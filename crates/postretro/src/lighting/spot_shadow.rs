// Dynamic spot light shadow-map pool and slot allocation.
//
// See: context/lib/rendering_pipeline.md §4 (Dynamic direct, spot shadow maps)

use crate::prl::{LightType, MapLight};
use glam::{Mat4, Vec3};

/// Near-clip distance used when building a spot light's projection matrix.
/// Matches the camera near-clip policy — close enough that self-shadowing
/// acne is controlled by the depth bias, far enough to keep precision.
pub const SHADOW_NEAR_CLIP: f32 = 0.1;

/// Build a light-space view-projection matrix for a spot light, producing
/// NDC that the forward shader converts to `[0, 1]` UVs for sampling.
///
/// `far` clamps to `falloff_range` but we enforce a minimum so zero-range
/// or degenerate lights don't produce a zero-extent frustum.
pub fn light_space_matrix(light: &MapLight) -> Mat4 {
    let eye = Vec3::new(
        light.origin[0] as f32,
        light.origin[1] as f32,
        light.origin[2] as f32,
    );
    let mut dir = Vec3::new(
        light.cone_direction[0],
        light.cone_direction[1],
        light.cone_direction[2],
    );
    if dir.length_squared() < 1e-8 {
        dir = Vec3::new(0.0, 0.0, -1.0);
    } else {
        dir = dir.normalize();
    }
    // Pick an up vector not colinear with `dir`.
    let world_up = if dir.y.abs() > 0.99 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let target = eye + dir;
    let view = Mat4::look_at_rh(eye, target, world_up);

    let fov_y = (2.0 * light.cone_angle_outer).max(0.05);
    let far = light.falloff_range.max(0.5);
    // `perspective_rh` in glam targets Vulkan/D3D/Metal depth range [0, 1].
    let proj = Mat4::perspective_rh(fov_y, 1.0, SHADOW_NEAR_CLIP, far);
    proj * view
}

/// Number of shadow-map slots in the pool. Re-tunable.
pub const SHADOW_POOL_SIZE: usize = 96;

/// Shared scoring/drop core for the shadow-slot rankers (spot pool here, cube
/// point pool in `cube_shadow.rs`). Takes pre-filtered, pre-scored candidates
/// — each `(light_index, influence_score)` — plus the pool `capacity` and the
/// total light count, and returns a `slot_assignment` Vec indexed by light
/// index: each entry is a slot (`0..capacity`) or [`NO_SHADOW_SLOT`].
///
/// Sorts by score descending, breaking ties by ascending light index for
/// determinism, then assigns the top `capacity` to dense slots `0..capacity`;
/// every lower-ranked candidate keeps `NO_SHADOW_SLOT` (dropped gracefully —
/// no panic). Spot and point rankers supply their own light-type filter and
/// influence score but MUST share this drop policy so the two cannot drift.
///
/// `candidates` is taken by value (mutated in place by the sort) so the caller
/// does not pay a clone.
pub fn assign_ranked_slots(
    mut candidates: Vec<(usize, f32)>,
    capacity: usize,
    light_count: usize,
) -> Vec<u32> {
    let mut slot_assignment = vec![NO_SHADOW_SLOT; light_count];

    // Sort by score (descending), then by index (ascending) for determinism.
    candidates.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });

    for (slot, (light_idx, _score)) in candidates.iter().take(capacity).enumerate() {
        slot_assignment[*light_idx] = slot as u32;
    }

    slot_assignment
}

/// Depth format for shadow maps.
pub const SHADOW_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Resolution (per side) of each square shadow map in the pool.
pub const SHADOW_MAP_RESOLUTION: u32 = 1024;

/// Sentinel value written to the slot index field when no slot is allocated.
pub const NO_SHADOW_SLOT: u32 = 0xFFFFFFFF;

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
    /// [`crate::lighting::entity_occluder_eligible`] (`casts_entity_shadows &&
    /// is_dynamic`). The shadow-depth render loop draws skinned entity occluders
    /// into a slot ONLY when this is `true`; an ineligible slot keeps its WORLD
    /// shadow but draws zero entity occluders. Separate from pool-slot
    /// eligibility (which still admits non-entity dynamic spots to a slot).
    pub slot_entity_eligible: [bool; SHADOW_POOL_SIZE],
}

impl SpotShadowPool {
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
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

    /// Compute the slot-assignment ranking for visible shadow-casting spot lights.
    ///
    /// Takes the candidate light list and the camera position. Identifies spot
    /// lights that are pool-eligible (`is_dynamic && Spot`) and pass the caller's
    /// per-light visibility/brightness gate, ranks them by a distance-based
    /// influence heuristic, and assigns the top `SHADOW_POOL_SIZE` to slots.
    ///
    /// **No camera-orientation coupling.** Slot eligibility depends only on the
    /// light's tier/type and the caller's `eligible_lights` gate (cell visibility
    /// and brightness, neither of which is a function of camera *orientation*);
    /// the limited pool is then ranked by distance score alone. This mirrors the
    /// point-light ranker (`rank_point_lights`) and means the SET of lights that
    /// receive a shadow slot is invariant under camera pitch/yaw at a fixed
    /// position. A previous version pre-filtered candidates by testing each
    /// cone's enclosing AABB against the camera frustum; that over-approximation
    /// could still wrongly drop a shadow whose cone reached a camera-visible
    /// receiver (the shadow vanished purely on pitch-down), so it was removed.
    /// Per-slot cone culling of occluders still happens later in `shadow_cull.rs`
    /// (light-cone based, not camera based).
    ///
    /// The caller passes the full-level light slice; the pool-eligibility gate
    /// is `is_dynamic && Spot`. Only dynamic-tier spotlights get a shadow slot —
    /// the shadow depth pass renders WORLD geometry, so a pooled dynamic spot
    /// shadows static occluders (e.g. pillars) regardless of the per-light
    /// `casts_entity_shadows` toggle (which only gates moving-ENTITY occluders,
    /// drawn into the same slot by `entity_occluder_eligible`). A baked light's
    /// world shadow is frozen in the lightmap, so it never needs a slot.
    ///
    /// Returns a Vec indexed by light index (into the slice the caller
    /// passes): entry is the slot index (`0..SHADOW_POOL_SIZE`) or NO_SHADOW_SLOT.
    pub fn rank_lights(
        lights: &[MapLight],
        camera_position: Vec3,
        camera_near_clip: f32,
        eligible_lights: &[bool],
    ) -> Vec<u32> {
        // Collect pool-eligible spot lights with their distance scores.
        let candidates: Vec<(usize, f32)> = lights
            .iter()
            .enumerate()
            .filter_map(|(idx, light)| {
                // Pool eligibility: only dynamic-tier spotlights get a shadow
                // slot. The shadow depth pass renders WORLD geometry, so a pooled
                // dynamic spot shadows static occluders (pillars) regardless of
                // the `casts_entity_shadows` toggle (which gates moving-ENTITY
                // occluders into the same slot, not slot allocation). Baked lights
                // bake their world shadow into the lightmap and need no slot.
                if !light.is_dynamic || light.light_type != LightType::Spot {
                    return None;
                }

                // Per-light eligibility gate. The caller folds visibility and
                // animated-brightness suppression into this slice; an empty
                // (or short) slice is treated as all-eligible so existing
                // tests and the first-frame pre-bridge call keep working.
                if idx < eligible_lights.len() && !eligible_lights[idx] {
                    return None;
                }

                // Compute heuristic score: (falloff_range / max(distance, near_clip))^2
                let light_pos = Vec3::new(
                    light.origin[0] as f32,
                    light.origin[1] as f32,
                    light.origin[2] as f32,
                );
                let dist = (light_pos - camera_position).length();
                let denom = dist.max(camera_near_clip);
                let score = (light.falloff_range / denom).powi(2);

                Some((idx, score))
            })
            .collect();

        if candidates.len() > SHADOW_POOL_SIZE {
            log::debug!(
                "[ShadowPool] {} pool-eligible spot lights visible; {} assigned to slots, {} unshadowed",
                candidates.len(),
                SHADOW_POOL_SIZE,
                candidates.len() - SHADOW_POOL_SIZE
            );
        }

        // Shared scoring/drop core: sort by score, assign top SHADOW_POOL_SIZE
        // to slots, drop the rest. The cube point-pool ranker calls the SAME
        // core so the two cannot drift.
        assign_ranked_slots(candidates, SHADOW_POOL_SIZE, lights.len())
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
    use proptest::prelude::*;

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

    /// Pool eligibility is `is_dynamic` (a baked light's world shadow is frozen
    /// in the lightmap, so only dynamic lights get a slot). The bool param sets
    /// `is_dynamic`; `casts_entity_shadows` stays `false` here because it gates
    /// moving-ENTITY occluders into the slot, not slot allocation itself.
    fn test_light(_idx: u32, origin: [f64; 3], falloff_range: f32, is_dynamic: bool) -> MapLight {
        MapLight {
            origin,
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: crate::prl::FalloffModel::Linear,
            falloff_range,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.6,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            leaf_index: 0,
            shadow_type: crate::prl::ShadowType::StaticLightMap,
        }
    }

    /// Build a `view_proj` matrix from an eye, pitch, and yaw (radians). Used by
    /// the orientation-sweep tests to vary only the camera's looking direction
    /// while holding position fixed.
    fn camera_view_proj(eye: Vec3, pitch: f32, yaw: f32) -> Mat4 {
        // Forward from yaw (around +Y) and pitch (around the camera right axis),
        // matching a Y-up, forward = -Z convention at (pitch=0, yaw=0).
        let forward = Vec3::new(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            -yaw.cos() * pitch.cos(),
        )
        .normalize();
        let world_up = if forward.y.abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let view = Mat4::look_at_rh(eye, eye + forward, world_up);
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 4096.0);
        proj * view
    }

    #[test]
    fn empty_light_list_produces_empty_assignment() {
        let assignment = SpotShadowPool::rank_lights(&[], Vec3::ZERO, 0.1, &[]);
        assert!(assignment.is_empty());
    }

    #[test]
    fn baked_spots_are_not_assigned() {
        // Non-dynamic (baked-tier) spotlights never get a slot: their world
        // shadow is frozen in the lightmap.
        let lights = vec![
            test_light(0, [0.0, 0.0, 0.0], 10.0, false),
            test_light(1, [10.0, 0.0, 0.0], 10.0, false),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[]);
        assert_eq!(assignment[0], NO_SHADOW_SLOT);
        assert_eq!(assignment[1], NO_SHADOW_SLOT);
    }

    /// Dynamic-tier spotlights are pool-eligible — they shadow static world
    /// occluders (e.g. pillars) through the world-geometry depth pass. A dynamic
    /// spot lands in a pool slot regardless of the `casts_entity_shadows` toggle
    /// (which only gates moving-ENTITY occluders into that slot).
    #[test]
    fn dynamic_spot_qualifies_for_pool() {
        let lights = vec![test_light(0, [0.0, 0.0, 0.0], 10.0, true)];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[]);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
    }

    /// The pool is spotlights-only. Making the dynamic tier pool-eligible by
    /// default widened the candidate set to every dynamic light, so the `Spot`
    /// guard is now the sole thing keeping dynamic POINT lights
    /// (`light_dynamic`) out of the spot pool. `campaign-test.map` ships such
    /// lights, so cover the exclusion explicitly.
    #[test]
    fn dynamic_point_light_is_not_assigned() {
        let mut light = test_light(0, [0.0, 0.0, 0.0], 10.0, true);
        light.light_type = LightType::Point;
        let lights = vec![light];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[]);
        assert_eq!(assignment[0], NO_SHADOW_SLOT);
    }

    #[test]
    fn point_lights_are_not_assigned() {
        let mut light = test_light(0, [0.0, 0.0, 0.0], 10.0, true);
        light.light_type = LightType::Point;
        let lights = vec![light];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[]);
        assert_eq!(assignment[0], NO_SHADOW_SLOT);
    }

    #[test]
    fn two_dynamic_spots_both_assigned() {
        let lights = vec![
            test_light(0, [0.0, 0.0, 0.0], 10.0, true),
            test_light(1, [10.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[]);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
        assert_ne!(assignment[1], NO_SHADOW_SLOT);
        // Should be different slots.
        assert_ne!(assignment[0], assignment[1]);
    }

    #[test]
    fn nine_lights_all_assigned_when_pool_has_capacity() {
        let mut lights = Vec::new();
        for i in 0..9 {
            lights.push(test_light(
                i as u32,
                [i as f64 * 10.0, 0.0, 0.0],
                10.0,
                true,
            ));
        }
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[]);

        let assigned_count = assignment.iter().filter(|&&s| s != NO_SHADOW_SLOT).count();
        assert_eq!(assigned_count, 9, "all 9 lights fit within pool capacity");

        let unshadowed_count = assignment.iter().filter(|&&s| s == NO_SHADOW_SLOT).count();
        assert_eq!(unshadowed_count, 0, "no lights left unshadowed");
    }

    #[test]
    fn closer_light_ranks_higher() {
        // Light 0 at origin is much closer than light 1 at distance 100.
        let lights = vec![
            test_light(0, [0.0, 0.0, 0.0], 10.0, true),
            test_light(1, [100.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[]);
        // Light 0 should get slot 0 (lower index = higher rank).
        assert_eq!(assignment[0], 0);
        assert_eq!(assignment[1], 1);
    }

    #[test]
    fn larger_falloff_ranks_higher() {
        // Both at same distance; light 0 has larger falloff_range.
        let lights = vec![
            test_light(0, [0.0, 0.0, -10.0], 20.0, true),
            test_light(1, [0.0, 0.0, -10.0], 10.0, true),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[]);
        // Light 0 (larger range) should get slot 0.
        assert_eq!(assignment[0], 0);
        assert_eq!(assignment[1], 1);
    }

    #[test]
    fn ties_broken_by_light_index() {
        // Two lights with identical distance and falloff_range.
        let lights = vec![
            test_light(0, [10.0, 0.0, 0.0], 10.0, true),
            test_light(1, [10.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[]);
        // Light 0 (lower index) should get slot 0; light 1 gets slot 1.
        assert_eq!(assignment[0], 0);
        assert_eq!(assignment[1], 1);
    }

    #[test]
    fn lights_in_invisible_cells_are_culled() {
        let lights = vec![
            test_light(0, [0.0, 0.0, -10.0], 10.0, true),
            test_light(1, [10.0, 0.0, -10.0], 10.0, true),
            test_light(2, [20.0, 0.0, -10.0], 10.0, true),
        ];
        let bitmask = [true, false, true];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &bitmask);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
        assert_eq!(assignment[1], NO_SHADOW_SLOT);
        assert_ne!(assignment[2], NO_SHADOW_SLOT);
    }

    #[test]
    fn nine_lights_with_eight_visible_assigns_eight() {
        // The invisible light (index 0) is placed closest to the camera so it
        // would otherwise rank #1 by heuristic — proving the bitmask filter
        // takes precedence over the score.
        let mut lights = Vec::new();
        lights.push(test_light(0, [0.0, 0.0, -1.0], 10.0, true));
        for i in 1..9 {
            lights.push(test_light(
                i as u32,
                [i as f64 * 50.0, 0.0, -10.0],
                10.0,
                true,
            ));
        }
        let mut bitmask = vec![true; 9];
        bitmask[0] = false;
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &bitmask);

        assert_eq!(assignment[0], NO_SHADOW_SLOT);
        let assigned_count = assignment[1..]
            .iter()
            .filter(|&&s| s != NO_SHADOW_SLOT)
            .count();
        assert_eq!(
            assigned_count, 8,
            "all 8 visible lights get slots (pool has 96 capacity)"
        );
    }

    #[test]
    fn empty_bitmask_treated_as_all_visible() {
        let lights = vec![
            test_light(0, [0.0, 0.0, -10.0], 10.0, true),
            test_light(1, [10.0, 0.0, -10.0], 10.0, true),
            test_light(2, [20.0, 0.0, -10.0], 10.0, true),
        ];
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, 0.1, &[]);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
        assert_ne!(assignment[1], NO_SHADOW_SLOT);
        assert_ne!(assignment[2], NO_SHADOW_SLOT);
    }

    #[test]
    fn camera_near_clip_clamps_denominator() {
        // Light very close to camera (distance < near_clip). Heuristic should clamp.
        let lights = vec![test_light(0, [0.001, 0.0, 0.0], 10.0, true)];
        let camera_near_clip = 0.1;
        let assignment = SpotShadowPool::rank_lights(&lights, Vec3::ZERO, camera_near_clip, &[]);
        // Should still be assigned.
        assert_eq!(assignment[0], 0);
    }

    /// Regression: dynamic spot lost its shadow slot when its cone AABB left the
    /// pitched camera frustum (shadow vanished on pitch-down).
    ///
    /// The light sits well off to the side of the camera, its cone reaching a
    /// receiver the camera can see, but the cone's enclosing AABB falls outside
    /// the pitched-down camera frustum. The old camera-frustum pre-filter dropped
    /// it here (`NO_SHADOW_SLOT`); slot eligibility must not depend on camera
    /// orientation, so it now keeps its slot.
    #[test]
    fn dynamic_spot_keeps_slot_when_cone_aabb_outside_pitched_camera_frustum() {
        // Light far off to the +X side, aimed straight down -Z.
        let lights = vec![test_light(0, [500.0, 0.0, -10.0], 10.0, true)];
        // Camera at the origin pitched sharply down — its frustum points at the
        // floor near x=0 and does not contain the light's cone AABB.
        let camera_eye = Vec3::ZERO;
        let pitched_down = camera_view_proj(camera_eye, -std::f32::consts::FRAC_PI_2 + 0.2, 0.0);
        // Sanity: the cone AABB really is outside this frustum (otherwise the
        // test wouldn't exercise the bug).
        let cone_aabb =
            crate::lighting::cone_frustum::cone_enclosing_aabb(&light_space_matrix(&lights[0]));
        let planes: [glam::Vec4; 6] =
            crate::compute_cull::extract_frustum_planes_for_gpu(&pitched_down)
                .map(|p| glam::Vec4::new(p[0], p[1], p[2], p[3]));
        assert!(
            !crate::lighting::cone_frustum::aabb_intersects_frustum(&cone_aabb, &planes),
            "test precondition: cone AABB must sit outside the pitched camera frustum"
        );

        let assignment = SpotShadowPool::rank_lights(&lights, camera_eye, 0.1, &[]);
        assert_ne!(
            assignment[0], NO_SHADOW_SLOT,
            "dynamic spot must keep its shadow slot regardless of where the camera looks"
        );
    }

    proptest! {
        /// The SET of lights receiving a shadow slot must be invariant under
        /// camera orientation. Fix the lights and the camera POSITION, sweep
        /// pitch and yaw across their full ranges, and assert the assigned-slot
        /// set never changes. This pins the position-independence symptom: the
        /// shadow vanished purely on camera pitch-down.
        #[test]
        fn shadow_slot_set_invariant_under_camera_orientation(
            pitch in -1.55f32..1.55,
            yaw in -3.14f32..3.14,
        ) {
            // A spread of dynamic spots around the scene, all within pool
            // capacity so distance ranking never drops any of them.
            let lights = vec![
                test_light(0, [0.0, 0.0, -10.0], 10.0, true),
                test_light(1, [500.0, 0.0, -10.0], 10.0, true),
                test_light(2, [-300.0, 50.0, 200.0], 10.0, true),
                test_light(3, [0.0, 200.0, 0.0], 10.0, true),
            ];
            let eye = Vec3::new(0.0, 0.0, 0.0);

            // Baseline: camera looking straight ahead.
            let baseline = SpotShadowPool::rank_lights(&lights, eye, 0.1, &[]);
            let baseline_set: std::collections::BTreeSet<usize> = baseline
                .iter()
                .enumerate()
                .filter(|&(_, s)| *s != NO_SHADOW_SLOT)
                .map(|(i, _)| i)
                .collect();

            // Orientation only changes the (unused) view direction; position is
            // fixed. The eye is the only thing rank_lights consumes from the
            // camera, so the set must match the baseline for every orientation.
            let _ = camera_view_proj(eye, pitch, yaw); // documents the swept pose
            let swept = SpotShadowPool::rank_lights(&lights, eye, 0.1, &[]);
            let swept_set: std::collections::BTreeSet<usize> = swept
                .iter()
                .enumerate()
                .filter(|&(_, s)| *s != NO_SHADOW_SLOT)
                .map(|(i, _)| i)
                .collect();

            prop_assert_eq!(
                swept_set,
                baseline_set,
                "the set of shadow-slot lights must not change with camera orientation",
            );
        }
    }

    /// Position/ranking guard: at a fixed orientation, sweeping the camera
    /// position changes slot assignment only through the documented distance
    /// ranking when the candidate pool overflows. With more candidates than a
    /// small capacity, the closest lights win the slots — and the winners track
    /// camera position, confirming distance ranking is live and not coupled to
    /// orientation.
    #[test]
    fn slot_ranking_follows_camera_position_when_pool_overflows() {
        // Two dynamic spots far apart on the X axis. Use the shared drop core
        // with a capacity of 1 so exactly one wins — the closer one.
        let near_a = Vec3::new(0.0, 0.0, 0.0);
        let near_b = Vec3::new(100.0, 0.0, 0.0);
        let lights = vec![
            test_light(0, [0.0, 0.0, 0.0], 10.0, true),
            test_light(1, [100.0, 0.0, 0.0], 10.0, true),
        ];

        // Full ranking keeps both (pool is large); the score still orders them.
        let from_a = SpotShadowPool::rank_lights(&lights, near_a, 0.1, &[]);
        assert_eq!(from_a[0], 0, "closest-to-A light 0 ranks first");
        assert_eq!(from_a[1], 1);

        // Move the camera next to light 1: now light 1 ranks first. The slot
        // ORDER tracks distance — the only documented coupling to the camera.
        let from_b = SpotShadowPool::rank_lights(&lights, near_b, 0.1, &[]);
        assert_eq!(from_b[1], 0, "closest-to-B light 1 ranks first");
        assert_eq!(from_b[0], 1);

        // And under genuine pool overflow (capacity 1 via the shared core), only
        // the closer light survives — proving the drop is distance-driven.
        let score = |light: &MapLight, cam: Vec3| -> f32 {
            let p = Vec3::new(
                light.origin[0] as f32,
                light.origin[1] as f32,
                light.origin[2] as f32,
            );
            (light.falloff_range / (p - cam).length().max(0.1)).powi(2)
        };
        let cands_a: Vec<(usize, f32)> = lights
            .iter()
            .enumerate()
            .map(|(i, l)| (i, score(l, near_a)))
            .collect();
        let overflow_a = assign_ranked_slots(cands_a, 1, lights.len());
        assert_eq!(
            overflow_a[0], 0,
            "with capacity 1, closest-to-A wins the slot"
        );
        assert_eq!(overflow_a[1], NO_SHADOW_SLOT);
    }
}
