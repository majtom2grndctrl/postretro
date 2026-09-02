// Dynamic spot light shadow-map pool and slot allocation.
//
// See: context/lib/rendering_pipeline.md §4 (Dynamic direct, spot shadow maps)

use glam::Mat4;

#[cfg(test)]
use postretro_lighting::light_space_matrix;
pub use postretro_lighting::{NO_SHADOW_SLOT, SHADOW_NEAR_CLIP};

/// Number of shadow-map slots in the pool. Re-tunable.
pub const SHADOW_POOL_SIZE: usize = 96;

/// Depth format for shadow maps.
pub const SHADOW_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Resolution (per side) of each square shadow map in the pool.
pub const SHADOW_MAP_RESOLUTION: u32 = 1024;

/// Size of the `array<mat4x4<f32>, SHADOW_POOL_SIZE>` storage buffer consumed
/// by the forward shader at `@group(5) @binding(2)`.
pub const LIGHT_SPACE_MATRICES_SIZE: u64 = (SHADOW_POOL_SIZE * 16 * 4) as u64;

/// Pool of shadow-map texture slots, one per dynamic spot light that
/// passes visibility culling. Ranked by projected influence area each frame.
///
/// Owns the group 5 resources the forward shader binds: the shadow depth
/// array (as a D2Array view), the comparison sampler, and the light-space
/// matrix storage buffer. `matrices` is sized for all `SHADOW_POOL_SIZE` slots;
/// slots that aren't assigned in a given frame are left at whatever was last written
/// (the fragment shader gates on the per-light slot sentinel so those
/// stale entries are never sampled).
pub struct SpotShadowPool {
    /// Array texture with SHADOW_POOL_SIZE layers, each SHADOW_MAP_RESOLUTION×SHADOW_MAP_RESOLUTION.
    /// Held for ownership — actual access goes through `views` and `bind_group`.
    #[allow(dead_code)]
    pub array_texture: wgpu::Texture,
    /// Texture views for each slot (2D views for render attachments).
    pub views: Vec<wgpu::TextureView>,
    /// D2Array view of `array_texture`, bound at `@group(5) @binding(0)` for sampling.
    /// Held for ownership — `bind_group` references it.
    #[allow(dead_code)]
    pub array_view: wgpu::TextureView,
    /// Comparison sampler bound at `@group(5) @binding(1)`.
    /// Held for ownership — `bind_group` references it.
    #[allow(dead_code)]
    pub compare_sampler: wgpu::Sampler,
    /// Uniform buffer of `SHADOW_POOL_SIZE` `mat4x4<f32>` bound at `@group(5) @binding(2)`.
    /// Contains light-space view-projection matrices per slot.
    pub matrices_buffer: wgpu::Buffer,
    /// Bind group for group 5 — lives alongside the resources above.
    pub bind_group: wgpu::BindGroup,
    /// Per-frame slot assignment: slot_assignment[light_index] = slot (0..SHADOW_POOL_SIZE) or NO_SHADOW_SLOT.
    pub slot_assignment: Vec<u32>,
    /// Per-slot light-space matrix for the occupant of each shadow slot, written
    /// during `update_dynamic_light_slots`. This is the SAME
    /// `light_space_matrix(candidate)` value uploaded to bind-group-5's matrices
    /// buffer — one source of truth, read by the shadow-depth render loop to
    /// build the slot's GPU cone-cull frustum planes. `None` = slot unoccupied.
    pub slot_cone_matrices: [Option<Mat4>; SHADOW_POOL_SIZE],
    /// Per-slot entity-occluder gate, written alongside `slot_cone_matrices` in
    /// `update_dynamic_light_slots`. `true` only when the slot's occupant passes
    /// [`postretro_lighting::entity_occluder_eligible`] (`casts_entity_shadows &&
    /// is_dynamic`). The shadow-depth render loop draws skinned entity occluders
    /// into a slot ONLY when this is `true`; an ineligible slot keeps its WORLD
    /// shadow but draws zero entity occluders. Separate from pool-slot
    /// eligibility (which still admits non-entity dynamic spots to a slot).
    pub slot_entity_eligible: [bool; SHADOW_POOL_SIZE],
}

impl SpotShadowPool {
    /// Clear all per-frame occupancy: cone matrices, entity gates, assignment.
    /// Called when a frame ranks zero candidates (a level with no dynamic
    /// lights following one that had them) so no stale slot keeps its depth
    /// pass rasterizing world geometry — the shader never samples those slots
    /// (every light packs the `NO_SHADOW_SLOT` sentinel), so a survivor is
    /// pure wasted GPU work.
    pub fn clear_occupancy(&mut self) {
        self.slot_cone_matrices = [None; SHADOW_POOL_SIZE];
        self.slot_entity_eligible = [false; SHADOW_POOL_SIZE];
        self.slot_assignment.clear();
    }

    /// Build the bind group layout for `@group(5)` of the forward shader.
    ///
    /// Group 5 has five or six entries depending on `cube_array_supported`:
    ///   0 = shadow depth array (D2Array Depth32Float; FRAGMENT | COMPUTE)
    ///   1 = comparison sampler (FRAGMENT | COMPUTE) — reused by the cube path
    ///   2 = light-space matrix uniform buffer (FRAGMENT | COMPUTE)
    ///   3 = half-res SDF shadow factor target (Rgba8Unorm; R = static, G = animated; FRAGMENT)
    ///   4 = full-res scene depth (Depth32Float; sampled via `textureLoad`; FRAGMENT)
    ///   5 = dynamic point-light cube-array shadow depth (CubeArray Depth32Float;
    ///       FRAGMENT) — present ONLY when `cube_array_supported`. A `CubeArray`
    ///       view requires `DownlevelFlags::CUBE_ARRAY_TEXTURES`, so on an adapter
    ///       without it this entry is omitted and the forward/fog pipelines build
    ///       from the no-cube shader variants (point shadows cleanly off).
    ///
    /// Bindings 3, 4, and 5 are owned outside the pool — the SDF shadow pass owns
    /// the factor target, the renderer owns the scene depth view, and the cube
    /// shadow pool owns the cube sampling view. All are supplied at construction
    /// time and must be re-supplied on resize via `rebuild_bind_group`. The fog
    /// volume compute pass also binds group 5 but does not reference slots 3, 4,
    /// or 5 — unused BGL entries are valid.
    pub fn bind_group_layout(
        device: &wgpu::Device,
        cube_array_supported: bool,
    ) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spot Shadow BGL"),
            entries: &Self::bind_group_layout_entries(cube_array_supported),
        })
    }

    /// CPU-only entry list backing `bind_group_layout`. Split out so the forward
    /// pipeline's sampled-texture budget can be re-derived from the real BGL
    /// definitions without a GPU device (see `render::mod.rs`).
    ///
    /// Binding 5 (the `CubeArray` point-shadow depth) is included only when
    /// `cube_array_supported` — both forward and fog share this BGL, so each
    /// variant stays layout-identical between the two pipelines.
    pub fn bind_group_layout_entries(
        cube_array_supported: bool,
    ) -> Vec<wgpu::BindGroupLayoutEntry> {
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(LIGHT_SPACE_MATRICES_SIZE),
                },
                count: None,
            },
            // Binding 3: SDF shadow factor (half-res Rgba8Unorm).
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // Binding 4: full-res scene depth, read via `textureLoad`.
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ];
        // Binding 5: dynamic POINT-light cube-array shadow depth
        // (`CubeShadowPool::sampling_view`). Sampled by the forward pass via
        // `textureSampleCompareLevel` (reusing the binding-1 comparison
        // sampler); BOUND but not sampled by the fog pass. FRAGMENT only —
        // the COMPUTE-visible shadow consumers (cone cull) never read it.
        //
        // Present ONLY when `cube_array_supported`: a `CubeArray` BGL entry
        // requires `DownlevelFlags::CUBE_ARRAY_TEXTURES`, so omitting it lets the
        // forward + fog pipelines build on adapters without the feature (point
        // shadows then cleanly off; the no-cube shader variants omit the matching
        // declaration). When present, the inventory is identical to before.
        if cube_array_supported {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::CubeArray,
                    multisampled: false,
                },
                count: None,
            });
        }
        entries
    }

    /// Allocate the shadow-map pool at renderer init.
    ///
    /// Creates a single array texture with `SHADOW_POOL_SIZE` layers,
    /// each `SHADOW_MAP_RESOLUTION × SHADOW_MAP_RESOLUTION` Depth32Float,
    /// along with the sampler, matrix buffer, and bind group that the
    /// forward shader's `@group(5)` layout expects.
    ///
    /// Bindings 3 (SDF shadow factor) and 4 (scene depth) are owned outside
    /// the pool — the SDF shadow pass owns the half-res factor target and the
    /// renderer owns the scene depth view. Both are passed in here so the pool
    /// can build a complete bind group at construction time. Both views must be
    /// re-supplied on resize via `rebuild_bind_group` since they are
    /// re-created when the surface changes size.
    ///
    /// `point_cube_view` is `Some` only when the adapter supports
    /// `CUBE_ARRAY_TEXTURES` — it must be `Some` iff `layout` was built with
    /// `cube_array_supported = true`, since the bind group's entry set must match
    /// the BGL exactly. `None` omits binding 5 (point shadows off).
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sdf_shadow_factor_view: &wgpu::TextureView,
        scene_depth_view: &wgpu::TextureView,
        point_cube_view: Option<&wgpu::TextureView>,
    ) -> Self {
        let array_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Spot Shadow Map Array"),
            size: wgpu::Extent3d {
                width: SHADOW_MAP_RESOLUTION,
                height: SHADOW_MAP_RESOLUTION,
                depth_or_array_layers: SHADOW_POOL_SIZE as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH_FORMAT,
            // Copy destination only: promoted-slot depth is copied IN from the
            // promoted depth cache. The pool is never a copy source.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Per-layer 2D views used as render attachments in the shadow pass.
        let views: Vec<wgpu::TextureView> = (0..SHADOW_POOL_SIZE)
            .map(|i| {
                array_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Spot Shadow Map View {}", i)),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i as u32,
                    array_layer_count: Some(1u32),
                    ..Default::default()
                })
            })
            .collect();

        // D2Array view used by the forward shader for sampling.
        let array_view = array_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Spot Shadow Array View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            base_array_layer: 0,
            array_layer_count: Some(SHADOW_POOL_SIZE as u32),
            ..Default::default()
        });

        // `CompareFunction::Less`: textureSampleCompare returns 1.0 (lit)
        // when the fragment's depth is less than the stored (light-nearest)
        // depth — i.e. the fragment is closer than the shadow caster, so
        // it's not occluded.
        let compare_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Spot Shadow Compare Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::Less),
            ..Default::default()
        });

        let matrices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spot Shadow Light-Space Matrices"),
            size: LIGHT_SPACE_MATRICES_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = build_bind_group(
            device,
            layout,
            &array_view,
            &compare_sampler,
            &matrices_buffer,
            sdf_shadow_factor_view,
            scene_depth_view,
            point_cube_view,
        );

        Self {
            array_texture,
            views,
            array_view,
            compare_sampler,
            matrices_buffer,
            bind_group,
            slot_assignment: Vec::new(),
            slot_cone_matrices: [None; SHADOW_POOL_SIZE],
            slot_entity_eligible: [false; SHADOW_POOL_SIZE],
        }
    }

    /// Rebuild the group-5 bind group after one of the external views
    /// (SDF shadow factor target or scene depth) has been re-created — both
    /// flip on a surface resize. The pool-owned resources (array view,
    /// sampler, matrix buffer) are stable across resizes.
    pub fn rebuild_bind_group(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sdf_shadow_factor_view: &wgpu::TextureView,
        scene_depth_view: &wgpu::TextureView,
        point_cube_view: Option<&wgpu::TextureView>,
    ) {
        self.bind_group = build_bind_group(
            device,
            layout,
            &self.array_view,
            &self.compare_sampler,
            &self.matrices_buffer,
            sdf_shadow_factor_view,
            scene_depth_view,
            point_cube_view,
        );
    }
}

// Thin GPU plumbing: one positional arg per group-5 binding resource. Splitting
// into a struct would only rename the same resources. `point_cube_view` is
// `Some` iff the layout carries binding 5 (cube-array support present); the bind
// group's entry set must match the BGL exactly.
#[allow(clippy::too_many_arguments)]
fn build_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    array_view: &wgpu::TextureView,
    compare_sampler: &wgpu::Sampler,
    matrices_buffer: &wgpu::Buffer,
    sdf_shadow_factor_view: &wgpu::TextureView,
    scene_depth_view: &wgpu::TextureView,
    point_cube_view: Option<&wgpu::TextureView>,
) -> wgpu::BindGroup {
    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(array_view),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::Sampler(compare_sampler),
        },
        wgpu::BindGroupEntry {
            binding: 2,
            resource: matrices_buffer.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 3,
            resource: wgpu::BindingResource::TextureView(sdf_shadow_factor_view),
        },
        wgpu::BindGroupEntry {
            binding: 4,
            resource: wgpu::BindingResource::TextureView(scene_depth_view),
        },
    ];
    // Binding 5 only when the BGL carries it (cube-array support present).
    if let Some(cube_view) = point_cube_view {
        entries.push(wgpu::BindGroupEntry {
            binding: 5,
            resource: wgpu::BindingResource::TextureView(cube_view),
        });
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Spot Shadow Bind Group"),
        layout,
        entries: &entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_level_loader::MapLight;
    use postretro_render_data::cone_frustum::{
        Aabb, aabb_intersects_frustum, cone_enclosing_aabb, cone_frustum_planes,
    };

    /// Scan a WGSL source for the `LightSpaceMatrices` array length, i.e. the
    /// `N` in `array<mat4x4<f32>, N>`. Returns `None` if the declaration is
    /// absent or unparseable so the test fails loudly rather than silently
    /// passing on a renamed/removed array.
    fn light_space_matrices_array_len(shader_src: &str) -> Option<usize> {
        let marker = "array<mat4x4<f32>,";
        let start = shader_src.find(marker)? + marker.len();
        let close = shader_src[start..].find('>')? + start;
        shader_src[start..close].trim().parse().ok()
    }

    /// Regression: the WGSL `LightSpaceMatrices` array was hard-coded to 12
    /// while the Rust pool was 64, so any slot ≥ 12 indexed the light-space
    /// matrix array out of bounds. Pin every shader that declares the array to
    /// `LIGHT_SPACE_MATRICES_SIZE` so none can silently drift from the pool.
    ///
    /// The skinned-mesh fragment shader (M10 mesh shadow receipt) declares the
    /// SAME `array<mat4x4<f32>, SHADOW_POOL_SIZE>` at its group-2 b7, sampling the
    /// pool's `matrices_buffer`, so it is scanned here alongside forward + fog — a
    /// mesh-side drift would index the light-space matrices out of bounds exactly
    /// as the forward bug did.
    #[test]
    fn light_space_matrices_array_len_matches_pool() {
        const FORWARD_SRC: &str = include_str!("../shaders/forward.wgsl");
        const FOG_SRC: &str = include_str!("../shaders/fog_volume.wgsl");
        const MESH_SRC: &str = include_str!("../shaders/skinned_mesh.wgsl");

        // `LIGHT_SPACE_MATRICES_SIZE` is the byte size of an
        // `array<mat4x4<f32>, SHADOW_POOL_SIZE>`: each mat4 is 16 f32 × 4 B.
        let expected_len = (LIGHT_SPACE_MATRICES_SIZE / (16 * 4)) as usize;
        assert_eq!(
            expected_len, SHADOW_POOL_SIZE,
            "LIGHT_SPACE_MATRICES_SIZE must encode exactly SHADOW_POOL_SIZE mat4x4s"
        );

        assert_eq!(
            light_space_matrices_array_len(FORWARD_SRC),
            Some(expected_len),
            "forward.wgsl LightSpaceMatrices array length must equal the Rust pool size"
        );
        assert_eq!(
            light_space_matrices_array_len(FOG_SRC),
            Some(expected_len),
            "fog_volume.wgsl LightSpaceMatrices array length must equal the Rust pool size"
        );
        assert_eq!(
            light_space_matrices_array_len(MESH_SRC),
            Some(expected_len),
            "skinned_mesh.wgsl LightSpaceMatrices array length must equal the Rust pool size"
        );
    }

    /// Tunable PCF radius wiring (AC, mechanical half): `sample_spot_shadow` must
    /// carry a single non-zero `SPOT_SHADOW_PCF_RADIUS` const and a multi-tap
    /// kernel scaled by it. Pins the shared radius constant already used by both
    /// `sample_spot_shadow` and `sample_point_shadow`, and guards against a silent
    /// revert to a single-texel (radius-zero / one-tap) sample.
    #[test]
    fn forward_spot_shadow_has_nonzero_pcf_radius_and_multitap_kernel() {
        // `SPOT_SHADOW_PCF_RADIUS` and the `sample_spot_shadow` kernel live in the
        // shared `shadow_sample.wgsl` snippet (extracted from forward.wgsl so the
        // skinned-mesh pass can reuse them), concatenated into the forward module
        // at pipeline build.
        const SHADOW_SRC: &str = include_str!("../shaders/shadow_sample.wgsl");

        // The shared radius parameter exists, is a const, and parses to non-zero.
        let marker = "const SPOT_SHADOW_PCF_RADIUS: f32 =";
        let start = SHADOW_SRC
            .find(marker)
            .expect("shadow_sample.wgsl must declare SPOT_SHADOW_PCF_RADIUS")
            + marker.len();
        let end = SHADOW_SRC[start..]
            .find(';')
            .expect("SPOT_SHADOW_PCF_RADIUS declaration must terminate with ';'")
            + start;
        let value: f32 = SHADOW_SRC[start..end]
            .trim()
            .parse()
            .expect("SPOT_SHADOW_PCF_RADIUS must be a float literal");
        assert!(
            value > 0.0,
            "PCF radius must be non-zero so the kernel samples more than one texel"
        );

        // The kernel scales its tap offsets by the radius and averages multiple
        // comparison samples (3×3 box → 9 taps), so it is not a single-texel
        // sample. Both the radius use and the 9-tap normalization must be present.
        assert!(
            SHADOW_SRC.contains("SPOT_SHADOW_PCF_RADIUS") && SHADOW_SRC.contains("/ 9.0"),
            "sample_spot_shadow must use the radius and average a multi-tap kernel"
        );
    }

    /// The skinned per-instance lane added by the follow-up task must scale the
    /// complete receiver normal offset. Scaling only a packed input would leave
    /// a non-zero offset at factor 0 and fail the pre-change sampling guarantee.
    #[test]
    fn receiver_bias_factor_scales_the_entire_shared_normal_offset() {
        const SHADOW_SRC: &str = include_str!("../shaders/shadow_sample.wgsl");
        const MESH_SRC: &str = include_str!("../shaders/skinned_mesh.wgsl");
        const OFFSET: &str =
            "let receiver_offset = receiver_normal * (texel_world_footprint * bias_scale);";

        assert_eq!(
            SHADOW_SRC.matches(OFFSET).count(),
            2,
            "spot and point receivers must multiply their complete normal offset by bias_scale"
        );
        assert_eq!(
            MESH_SRC.matches("SKINNED_SCALE * bias_factor").count(),
            4,
            "skinned pool/cache spot and point calls must apply the authorable factor to the shared offset scale"
        );
        assert!(
            MESH_SRC.contains("out.shadow_bias_scale = bitcast<f32>(instance.base_and_pad.y);")
                && MESH_SRC.contains("in.shadow_bias_scale,"),
            "the Task 3 instance lane must reach the skinned receiver-bias factor"
        );
    }

    /// Spotlight at the origin aimed down -Z, used by light-space matrix tests.
    fn spot_down_neg_z() -> MapLight {
        MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: postretro_level_loader::LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: postretro_level_loader::FalloffModel::Linear,
            falloff_range: 20.0,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.4,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic: true,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
        }
    }

    /// AC#2: a world AABB inside the cone is classified inside; one fully
    /// outside the cone (behind the light, opposite the aim) is classified
    /// outside. Same predicate the GPU per-slot cull mirrors.
    #[test]
    fn cone_frustum_classifies_inside_and_outside_aabbs() {
        let light = spot_down_neg_z();
        let m = light_space_matrix(&light);
        let planes = cone_frustum_planes(&m);

        let inside = Aabb {
            min: Vec3::new(-0.5, -0.5, -10.5),
            max: Vec3::new(0.5, 0.5, -9.5),
        };
        assert!(
            aabb_intersects_frustum(&inside, &planes),
            "on-axis box inside the cone must classify as inside"
        );

        let behind = Aabb {
            min: Vec3::new(-0.5, -0.5, 9.5),
            max: Vec3::new(0.5, 0.5, 10.5),
        };
        assert!(
            !aabb_intersects_frustum(&behind, &planes),
            "box behind the light must classify as outside the cone"
        );

        let off_axis = Aabb {
            min: Vec3::new(49.5, -0.5, -10.5),
            max: Vec3::new(50.5, 0.5, -9.5),
        };
        assert!(
            !aabb_intersects_frustum(&off_axis, &planes),
            "box outside the cone's angular spread must classify as outside"
        );
    }

    /// The enclosing AABB derived from the light-space matrix must contain the
    /// cone: it spans the aim direction and stays bounded near the apex.
    #[test]
    fn cone_enclosing_aabb_spans_aim_direction() {
        let light = spot_down_neg_z();
        let m = light_space_matrix(&light);
        let aabb = cone_enclosing_aabb(&m);

        assert!(
            aabb.min.z < -19.0,
            "enclosing AABB should reach the far plane (~-20), got min.z = {}",
            aabb.min.z
        );
        assert!(
            aabb.max.z > -0.5,
            "enclosing AABB should include the apex near the origin, got max.z = {}",
            aabb.max.z
        );
        assert!(
            aabb.min.x.is_finite() && aabb.max.x.is_finite(),
            "enclosing AABB lateral extent must be finite"
        );
    }

    /// A point inside the enclosing AABB and on the cone axis must also pass the
    /// plane predicate — the two representations agree on the obvious interior.
    #[test]
    fn enclosing_aabb_interior_point_passes_planes() {
        let light = spot_down_neg_z();
        let m = light_space_matrix(&light);
        let planes = cone_frustum_planes(&m);

        let center = Aabb {
            min: Vec3::new(-0.1, -0.1, -10.1),
            max: Vec3::new(0.1, 0.1, -9.9),
        };
        assert!(aabb_intersects_frustum(&center, &planes));
    }

    /// An AABB straddling the cone apex must be classified as intersecting.
    #[test]
    fn cone_frustum_apex_straddling_aabb_classifies_as_intersecting() {
        let light = spot_down_neg_z();
        let m = light_space_matrix(&light);
        let planes = cone_frustum_planes(&m);

        let apex_box = Aabb {
            min: Vec3::new(-0.2, -0.2, -2.0),
            max: Vec3::new(0.2, 0.2, 0.0),
        };
        assert!(
            aabb_intersects_frustum(&apex_box, &planes),
            "AABB straddling the cone apex must be classified as intersecting"
        );
    }

    /// An AABB that grazes the cone's right side plane from just inside must be
    /// classified as intersecting; one clearly past the side boundary must not.
    #[test]
    fn cone_frustum_grazing_side_plane_aabb_classified_correctly() {
        let light = spot_down_neg_z();
        let m = light_space_matrix(&light);
        let planes = cone_frustum_planes(&m);

        let just_inside = Aabb {
            min: Vec3::new(3.0, -0.5, -10.5),
            max: Vec3::new(4.0, 0.5, -9.5),
        };
        assert!(
            aabb_intersects_frustum(&just_inside, &planes),
            "AABB with positive vertex inside the cone side plane must intersect"
        );

        let clearly_outside = Aabb {
            min: Vec3::new(9.5, -0.5, -10.5),
            max: Vec3::new(10.5, 0.5, -9.5),
        };
        assert!(
            !aabb_intersects_frustum(&clearly_outside, &planes),
            "AABB well outside the cone side plane must not intersect"
        );
    }
}
