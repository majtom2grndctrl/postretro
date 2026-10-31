// Skinned-mesh group-2 lighting: layout entries, params uniform, bind-group rebuild.
// See: context/lib/rendering_pipeline.md §9

use super::MeshPass;

/// Mesh-side group-2 params uniform (binding 4): runtime-light count, the frame's
/// render-clock time, and `light_term_mask`. `time` is the SAME render-clock
/// value the renderer uploads to forward `Uniforms.time` that frame (the renderer
/// caches it and threads it in), so the scripted-light animated curves the mesh
/// loop evaluates stay phase-coherent with the forward pass and the CPU light
/// bridge. `light_term_mask` is the per-frame snapshot that the renderer writes
/// to forward `Uniforms.light_term_mask`, so mesh ambient and runtime-direct
/// gates agree with the world path. `ambient_floor` is the SAME constant ambient fill the
/// renderer uploads to forward `Uniforms.ambient_floor` that frame; the mesh
/// fragment shader adds it once as an additive fill so shadowed mesh faces lift
/// with the diagnostics slider exactly as world surfaces do (see forward.wgsl's
/// ambient-floor term). Its second std140 row defines b0's three regions:
/// `[0, dynamic_light_count)` is the dynamic prefix,
/// `[dynamic_light_count, scripted_light_count)` is the raw animated-baked tail,
/// and `[scripted_light_count, light_count)` is the selected-static suffix.
/// `indirect_scale` is the renderer's SH-indirect multiplier captured with the
/// frame's forward group 0 (`Uniforms.indirect_scale`), so the diagnostics
/// slider reaches meshes on the same frame as world surfaces.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct MeshLightParams {
    pub(super) light_count: u32,
    pub(super) time: f32,
    pub(super) light_term_mask: u32,
    pub(super) ambient_floor: f32,
    pub(super) dynamic_light_count: u32,
    /// Descriptor-bearing prefix: dynamic tier plus the raw section-45
    /// animated-baked tail. Selected-static promotion records append after it.
    pub(super) scripted_light_count: u32,
    pub(super) indirect_scale: f32,
    pub(super) _pad2: u32,
}

/// Byte size of the group-2 params uniform (`MeshLightParams`, 32 B).
pub(super) const MESH_LIGHT_PARAMS_SIZE: u64 = std::mem::size_of::<MeshLightParams>() as u64;

/// Serialize `MeshLightParams` to its 32-byte std140 upload. The dynamic count
/// sits at bytes 16..20 and the descriptor-bearing count at 20..24.
/// Split out from
/// `write_light_params` so the byte layout can be asserted GPU-free in tests.
pub(super) fn build_light_params_bytes(params: MeshLightParams) -> Vec<u8> {
    [
        params.light_count.to_ne_bytes(),
        params.time.to_ne_bytes(),
        params.light_term_mask.to_ne_bytes(),
        params.ambient_floor.to_ne_bytes(),
        params.dynamic_light_count.to_ne_bytes(),
        params.scripted_light_count.to_ne_bytes(),
        params.indirect_scale.to_ne_bytes(),
        params._pad2.to_ne_bytes(),
    ]
    .concat()
}

/// GPU-free builder for the mesh group-2 (runtime direct lighting + shadow
/// receipt) BGL entries. Single source of truth: `MeshPass::new` builds the layout
/// from this, and the headless `mesh_group2_bgl_matches_shader_bindings` test
/// re-derives the binding map and per-stage storage budget from the SAME entries —
/// so a drift in either the shader's group-2 declarations or the budget fails CI
/// before a real GPU would reject the pipeline. Pinned binding map (mirrors
/// `skinned_mesh.wgsl` group 2 and rendering_pipeline.md §9, §10):
///   b0 three regions — dynamic prefix, raw animated-baked tail, and
///   selected-static suffix — b1 per-light
///   influence volumes, b2 scripted-animation descriptors, b3 scripted-animation
///   curve samples, b4 the mesh-side params uniform (all FRAGMENT-only). The
///   dynamic-light loop runs in the fragment stage, so b0–b3 contribute FOUR
///   fragment-visible storage buffers — well under the per-stage ceiling of 8
///   (rendering_pipeline.md §10). b4 is a uniform (no storage-slot cost).
///   b5–b8 are the SHADOW-RECEIPT bindings on a MESH-SPECIFIC layout that omits
///   forward's SDF-factor + scene-depth entries the mesh must not sample. They
///   alias the SAME GPU resources the forward pass binds in its group 5 (NOT
///   forward's group-5 BGL):
///     b5 spot depth 2D-array (`spot_shadow_depth`, FRAGMENT),
///     b6 comparison sampler (`spot_shadow_compare`, FRAGMENT),
///     b7 light-space matrices UNIFORM (`light_space_matrices`, FRAGMENT) — a
///        uniform, NOT storage, so it adds NOTHING to the fragment storage-buffer
///        count (still 4); same `array<mat4x4<f32>, SHADOW_POOL_SIZE>` budget the
///        forward shader uses (well under the 16 KiB uniform cap),
///     b8 cube-array depth (`point_shadow_cube`, `texture_depth_cube_array`,
///        FRAGMENT) — present ONLY when `cube_array_supported`. A `CubeArray` BGL
///        entry requires `DownlevelFlags::CUBE_ARRAY_TEXTURES`, so on an adapter
///        without it the entry is omitted (and `render::strip_point_shadow_cube`
///        drops the matching shader declaration), exactly as forward's group-5 BGL
///        omits its binding 5. The cube view is passed `Some` to
///        `light_bind_groups` iff this entry is present (the
///        `Some`-iff-layout invariant — a single unconditional BGL crashes on a
///        no-cube adapter).
///
/// b5 + b8 are sampled depth textures (spot 2D-array always, cube array iff
/// supported): the mesh pipeline's group-2 sampled-texture count is ONE without
/// cube support and TWO with it.
pub(super) fn mesh_light_bind_group_layout_entries(
    cube_array_supported: bool,
) -> Vec<wgpu::BindGroupLayoutEntry> {
    let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let mut entries = vec![
        // b0: dynamic prefix, raw animated-baked tail, selected-static suffix.
        storage_entry(0),
        // b1: per-light influence volumes.
        storage_entry(1),
        // b2: scripted-animation descriptors (forward group-3 b13).
        storage_entry(2),
        // b3: scripted-animation curve samples (forward group-3 b12).
        storage_entry(3),
        // b4: mesh-side params uniform (light count, time, light-term mask).
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // b5: spot shadow depth 2D-array (`spot_shadow_depth`). SAME texture the
        // forward pass binds at group-5 b0 (the spot pool's `array_view`).
        wgpu::BindGroupLayoutEntry {
            binding: 5,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        // b6: comparison sampler (`spot_shadow_compare`). SAME sampler the forward
        // pass binds at group-5 b1; reused by the cube path too.
        wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
            count: None,
        },
        // b7: light-space matrices UNIFORM (`light_space_matrices`). SAME buffer
        // the forward pass binds at group-5 b2 — a uniform (NOT storage) to keep
        // the fragment storage-buffer count at 4 (rendering_pipeline.md §10).
        wgpu::BindGroupLayoutEntry {
            binding: 7,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(
                    crate::lighting::spot_shadow::LIGHT_SPACE_MATRICES_SIZE,
                ),
            },
            count: None,
        },
    ];
    // b8: dynamic POINT-light cube-array shadow depth (`point_shadow_cube`). SAME
    // `CubeArray` view the forward pass binds at group-5 b5 (the cube pool's
    // `sampling_view`). Present ONLY when `cube_array_supported`: a `CubeArray` BGL
    // entry requires `DownlevelFlags::CUBE_ARRAY_TEXTURES`, so omitting it lets the
    // mesh pipeline build on adapters without the feature (the no-cube shader
    // variant strips the matching declaration). The cube view is supplied `Some`
    // to `light_bind_groups` iff this entry is present.
    if cube_array_supported {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 8,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::CubeArray,
                multisampled: false,
            },
            count: None,
        });
    }
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 9,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    });
    if cube_array_supported {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 10,
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

/// Count BGL entries that consume a `max_storage_buffers_per_shader_stage` slot
/// for the FRAGMENT stage: read-only storage `Buffer` entries whose visibility
/// includes FRAGMENT. wgpu charges this limit against the BGL *entry* set of a
/// pipeline layout per stage, not against what a shader reads. Mirrors
/// `render::mod::vertex_storage_buffers` for the fragment stage; the mesh
/// dynamic-light loop is the mesh fragment stage's first storage-buffer use.
#[cfg(test)]
pub(super) fn fragment_storage_buffers(entries: &[wgpu::BindGroupLayoutEntry]) -> u32 {
    entries
        .iter()
        .filter(|e| {
            e.visibility.contains(wgpu::ShaderStages::FRAGMENT)
                && matches!(
                    e.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { .. },
                        ..
                    }
                )
        })
        .count() as u32
}

/// Count BGL entries that consume a `max_sampled_textures_per_shader_stage` slot
/// for the FRAGMENT stage: `BindingType::Texture` entries whose visibility
/// includes FRAGMENT. wgpu charges this limit against the BGL *entry* set of a
/// pipeline layout per stage, not against how many textures a shader samples.
/// Mirrors `render::mod::fragment_sampled_textures` for the mesh group-2 budget
/// guard; the mesh group-2 shadow textures (spot depth array + the optional cube
/// array) are the mesh fragment stage's group-2 sampled-texture draw.
#[cfg(test)]
pub(super) fn fragment_sampled_textures(entries: &[wgpu::BindGroupLayoutEntry]) -> u32 {
    entries
        .iter()
        .filter(|e| {
            e.visibility.contains(wgpu::ShaderStages::FRAGMENT)
                && matches!(e.ty, wgpu::BindingType::Texture { .. })
        })
        .count() as u32
}

impl MeshPass {
    /// The group-2 layout and params buffer a level's mesh light groups
    /// build over ([`LightGroupLayout::light_bind_groups`]), cloned so a
    /// level build on any thread builds them.
    pub(in crate::render) fn light_group_layout(&self) -> LightGroupLayout {
        LightGroupLayout {
            pass: "Skinned Mesh",
            layout: self.light_bind_group_layout.clone(),
            light_params_buffer: self.light_params_buffer.clone(),
            cube_array_supported: self.cube_array_supported,
        }
    }
}

/// One dynamic-object pass's group-2 layout and its params buffer (b4),
/// cloned out of the pass so a level build on any thread builds that pass's
/// light groups. `pass` names the groups.
#[derive(Clone)]
pub(in crate::render) struct LightGroupLayout {
    pub(in crate::render) pass: &'static str,
    pub(in crate::render) layout: wgpu::BindGroupLayout,
    pub(in crate::render) light_params_buffer: wgpu::Buffer,
    pub(in crate::render) cube_array_supported: bool,
}

/// One set's buffers and shadow-tier views a dynamic-object pass's group 2
/// binds ([`LightGroupLayout::light_bind_groups`] documents each binding).
pub(in crate::render) struct LightGroupInputs<'a> {
    pub(in crate::render) lights: &'a wgpu::Buffer,
    pub(in crate::render) mirror_lights: &'a wgpu::Buffer,
    pub(in crate::render) influence: &'a wgpu::Buffer,
    pub(in crate::render) scripted_descriptors: &'a wgpu::Buffer,
    pub(in crate::render) anim_samples: &'a wgpu::Buffer,
    pub(in crate::render) spot_shadow_depth: &'a wgpu::TextureView,
    pub(in crate::render) spot_shadow_compare: &'a wgpu::Sampler,
    pub(in crate::render) light_space_matrices: &'a wgpu::Buffer,
    pub(in crate::render) point_shadow_cube: Option<&'a wgpu::TextureView>,
    pub(in crate::render) promoted_spot_cache: &'a wgpu::TextureView,
    pub(in crate::render) promoted_cube_cache: Option<&'a wgpu::TextureView>,
}

impl LightGroupLayout {
    /// (Re)build the group-2 runtime-direct light bind group over the renderer's
    /// runtime light buffers. Called once after geometry installs and again on any
    /// reallocation of these buffers (level load), mirroring how the renderer
    /// rebuilds its forward `lighting_bind_group`. The buffers are owned by the
    /// renderer and bound here by reference; b4 is this pass's own
    /// `light_params_buffer` (cloned into this layout).
    ///
    /// b0 uses three semantic intervals:
    /// - `[0, dynamic_light_count)`: dynamic prefix from `filter_dynamic_lights`;
    /// - `[dynamic_light_count, scripted_light_count)`: raw animated-baked tail;
    /// - `[scripted_light_count, light_count)`: selected-static suffix.
    ///
    /// The params uniform loop-bounds all three regions. Do not bind the raw
    /// shadow-candidate set here. `influence` is the matching per-light
    /// influence-volume buffer. `scripted_descriptors` is forward's group-3 b13
    /// `scripted_light_descriptors`; `anim_samples` is forward's group-3 b12
    /// `anim_samples` — the SAME GPU buffers, rebound at mesh group 2 b2/b3.
    ///
    /// b5–b8 are the SHADOW-RECEIPT bindings, on a mesh-specific layout that
    /// OMITS forward's SDF-factor + scene-depth entries the mesh must not sample.
    /// They alias the SAME pool-owned GPU resources the forward pass binds in its
    /// group 5 (NOT forward's group-5 BGL):
    /// `spot_shadow_depth` is the spot pool's D2-array `array_view` (b5),
    /// `spot_shadow_compare` is the pool's comparison sampler (b6),
    /// `light_space_matrices` is the pool's `matrices_buffer` UNIFORM (b7), and
    /// `point_shadow_cube` is the cube pool's `CubeArray` `sampling_view` (b8).
    ///
    /// `point_shadow_cube` MUST be `Some` IFF the layout carries the b8 entry — i.e.
    /// iff `self.cube_array_supported` (the `Some`-iff-layout invariant). Passing
    /// `Some` on a no-cube layout (or `None` on a cube layout) is a bind-group /
    /// layout mismatch wgpu rejects; the assert below pins the invariant before the
    /// GPU sees it. A level install may reallocate the spot pool for a pending
    /// shadow-quality change, so rebuild this group against the current pool resources
    /// after each level install/reallocation, alongside the b0–b4 rebind.
    pub(in crate::render) fn light_bind_groups(
        &self,
        device: &wgpu::Device,
        inputs: &LightGroupInputs<'_>,
    ) -> crate::render::LevelLightGroups {
        use crate::render::gpu_create::GpuCreate;
        let pass = self.pass;
        assert_eq!(
            inputs.point_shadow_cube.is_some(),
            self.cube_array_supported,
            "{pass} group-2 cube view must be Some iff the BGL carries the b8 cube entry \
             (cube_array_supported) — the Some-iff-layout invariant",
        );
        assert_eq!(
            inputs.promoted_cube_cache.is_some(),
            self.cube_array_supported,
            "{pass} group-2 promoted cube cache must be Some iff the BGL carries the b10 cube entry",
        );
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: inputs.lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: inputs.influence.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: inputs.scripted_descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: inputs.anim_samples.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: self.light_params_buffer.as_entire_binding(),
            },
            // b5: spot shadow depth 2D-array (pool `array_view`).
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(inputs.spot_shadow_depth),
            },
            // b6: comparison sampler (pool `compare_sampler`).
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(inputs.spot_shadow_compare),
            },
            // b7: light-space matrices uniform (pool `matrices_buffer`).
            wgpu::BindGroupEntry {
                binding: 7,
                resource: inputs.light_space_matrices.as_entire_binding(),
            },
        ];
        // b8: cube-array depth — present IFF the BGL carries it (cube support).
        if let Some(cube_view) = inputs.point_shadow_cube {
            entries.push(wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(cube_view),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 9,
            resource: wgpu::BindingResource::TextureView(inputs.promoted_spot_cache),
        });
        if let Some(cube_view) = inputs.promoted_cube_cache {
            entries.push(wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(cube_view),
            });
        }
        let light = device.new_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{pass} Light Bind Group (group 2)")),
            layout: &self.layout,
            entries: &entries,
        });
        // The mirror's group differs only in b0.
        entries[0].resource = inputs.mirror_lights.as_entire_binding();
        let mirror = device.new_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{pass} Mirror Light Bind Group (group 2)")),
            layout: &self.layout,
            entries: &entries,
        });
        crate::render::LevelLightGroups { light, mirror }
    }
}

impl MeshPass {
    /// Whether this frame's mirror draws read the mirror's light records.
    pub(crate) fn set_mirror_light_records(&mut self, mirror: bool) {
        self.mirror_light_records = mirror;
    }

    #[cfg(test)]
    pub(in crate::render) fn light_params_buffer_for_test(&self) -> &wgpu::Buffer {
        &self.light_params_buffer
    }

    /// Write this frame's group-2 params uniform (binding 4): the dynamic-light
    /// `light_count`, the frame's render-clock `time`, and `light_term_mask`.
    /// `time` MUST be the SAME value the renderer wrote to forward `Uniforms.time`
    /// this frame (the renderer caches it in `update_per_frame_uniforms` and
    /// threads it here), so the scripted-light curves the mesh loop evaluates stay
    /// phase-coherent with the forward pass. `light_term_mask` MUST be the SAME
    /// captured snapshot the renderer writes to forward `Uniforms.light_term_mask`,
    /// so the mesh ambient and dynamic-direct gates land with the world path.
    /// `ambient_floor` MUST be the SAME value the renderer writes to forward
    /// `Uniforms.ambient_floor` this frame, so shadowed mesh faces lift with the
    /// diagnostics ambient-floor slider exactly as world surfaces do.
    /// `indirect_scale` is the snapshot captured with forward group 0, so a
    /// diagnostics change reaches all SH surface consumers on the same frame.
    #[allow(clippy::too_many_arguments)] // Mirrors the fixed group-2 light uniform fields.
    pub fn write_light_params(
        &self,
        queue: &crate::render::uploads::UploadQueue,
        light_count: u32,
        dynamic_light_count: u32,
        scripted_light_count: u32,
        time: f32,
        light_term_mask: u32,
        ambient_floor: f32,
        indirect_scale: f32,
    ) {
        let bytes = build_light_params_bytes(MeshLightParams {
            light_count,
            dynamic_light_count,
            scripted_light_count,
            time,
            light_term_mask,
            ambient_floor,
            indirect_scale,
            _pad2: 0,
        });
        queue.write_buffer(&self.light_params_buffer, 0, &bytes);
    }
}
