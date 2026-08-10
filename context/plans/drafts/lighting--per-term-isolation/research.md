# Per-term lighting isolation — research notes

Grounding for `index.md`. Findings, not decisions.

## Today: two disjoint isolation instruments

| Instrument | Type | Carriers | Consumers |
|---|---|---|---|
| `LightingIsolation` | 10-variant enum (`frame_uniforms.rs:93-104`) | group-0 `FrameUniforms.lighting_isolation` (bytes 88..92); group-2 `MeshLightParams.lighting_isolation` (8..12); group-2 `KinematicLightParams.lighting_isolation` (8..12) | `forward.wgsl` gates 4 terms (`use_lightmap`/`use_indirect`/`use_specular`/`use_dynamic`, `:901-906`); `skinned_mesh.wgsl` + `kinematic_brush.wgsl` gate ONLY `use_dynamic` (`skinned_mesh.wgsl:607-608`, `kinematic_brush.wgsl:343-344`); `billboard.wgsl` ignores it (`billboard.wgsl:14`) |
| `DynamicDirectIsolation` | 3-variant enum (`frame_uniforms.rs:165-172`) | group-0 tail `dynamic_direct_isolation` (112..116, billboard); group-4 `DynamicDirectParams.isolation` (binding 16, mesh + kinematic) | `billboard.wgsl:278-282`, `skinned_mesh.wgsl:626-631`, `kinematic_brush.wgsl:358-363` — gates SH indirect vs baked-direct on the entity/mover/sprite paths only |

Root cause of the reported symptom: `LightingIsolation` is a forward/static-pass instrument. On the SH-lit paths (entity/mover/sprite) it gates only the small runtime dynamic-direct term; the dominant baked SH terms are gated by the separate, narrower `DynamicDirectIsolation`. No single per-term instrument spans all four paths, so toggling `LightingIsolation` transforms the world and leaves entities essentially unchanged.

## Separability of STATIC vs ANIMATED baked light

Whether a shader can gate static vs animated baked light independently, per family:

| Family | Static base resident? | Bound to render shader? | Animated folded in? | Verdict |
|---|---|---|---|---|
| Indirect SH (octahedral irradiance) | Yes (`base_atlas_view`, compact/BC6H, `sh_volume.rs:33-39`) | No — compose group 1 only | Yes → `sh_total_atlas` = `base + Σ(delta·scale)` (`sh_compose.wgsl:260-283`) | **NOT-SEPARABLE at shader** |
| Direct SH (binding 15) | Yes (`direct_base_atlas_view`, BC6H) | No when composed exists — compose only | Yes → composed = `(base − Σ promoted_static·w) + animated_direct_delta` (`direct_sh_compose.wgsl:201-218`, `animated_direct_sh_compose.rs:69-239`) | **NOT-SEPARABLE at shader** |
| World lightmap | Yes (`lightmap_irradiance`) | Yes (group 4 binding 0) | No — separate `animated_lm_atlas` (binding 3) | **SEPARABLE-TODAY (in-shader)** |

- World lightmap: `lm_irr` (static, `forward.wgsl:940`) and `lm_anim` (animated, `:952`) are separate variables summed only at `:1002` (`static_direct = lm_irr*scale + lm_anim*scale_anim`). Independent gating is a one-boolean-each change; the current `use_lightmap` treats them together as a policy choice, not a resource limit.
- Both SH families: the base is compact-slot/BC6H and bound only to the compose pass; the composed dense atlas is not texel-aligned with it, and (direct) promotion has already subtracted static lights. A shader-side `composed − base` is impossible. Isolating static vs animated for the SH families requires gating **at compose time**, where base and delta are still separate inputs.

## Sampled-texture ceiling forecloses the "bind base + composed" approach

`renderer_init_resources.rs:83` — `REQUIRED_SAMPLED_TEXTURES = 16`, "the WebGPU spec floor and Metal's hard ceiling." `pipeline_budget_tests.rs:112-207` derives the forward count from the BGL builders and asserts it is **exactly 16** with cube-array support (15 without) once emissive lands (`:169-176`, "emissive must raise the cube-array forward sampled-texture count to exactly 16"). The debug-assert at `renderer_init_resources.rs:92-99` says to switch to bindless rather than raise the limit. So there is **no headroom** to bind a static base atlas alongside the composed atlas — reinforcing the compose-time gate for the SH families.

## Compose dispatch conditions

- `sh_compose` (indirect): runs unconditionally every frame (`rendering_pipeline.md §7.1` step 5). Gating its accumulation by a mask adds no dispatch logic.
- `direct_sh_compose` / `animated_direct_sh_compose`: dispatch on level-load, while any promotion weight is nonzero, and once when weights return to zero. A mask change must dirty a re-dispatch.

## Four-path plumbing map

| Path | Isolation-carrying uniform | Per-frame writer call-site | Byte-layout assertion test |
|---|---|---|---|
| Forward / world | group-0 `FrameUniforms.lighting_isolation` (88..92) | `build_uniform_data(&FrameUniforms{..})` — `renderer_frame.rs:270` | `frame_uniforms.rs:434` (CPU offsets) + `shader_tests.rs:135` (WGSL stride == `UNIFORM_SIZE` 128) |
| skinned_mesh | group-2 `MeshLightParams.lighting_isolation` (8..12) + group-4 `DynamicDirectParams` (b16) | `MeshPass::write_light_params` — `renderer_render_frame.rs:551`/`:786`; `ShVolumeResources::write_dynamic_direct_params` — `renderer_frame.rs:300` | `mesh_pass.rs:2106` (MeshLightParams 8..12); `sh_volume.rs:1775` (b16 layout) |
| kinematic_brush | group-2 `KinematicLightParams.lighting_isolation` (8..12) + group-4 `DynamicDirectParams` (b16, shared `mesh_bind_group`) | `KinematicBrushPass::write_light_params` — `renderer_render_frame.rs:487`; shares `write_dynamic_direct_params` | `kinematic_brush.rs:1120` (bytes) + `:1140` (WGSL layout) |
| billboard | group-0 tail: `dynamic_direct_isolation` (112..116), `dynamic_direct_scale` (108..112), `has_direct` (116..120); does NOT read 88..92 | same `build_uniform_data` — `renderer_frame.rs:270` | `frame_uniforms.rs:405` (tail offsets) + `shader_tests.rs:135` (stride); behavioral `shader_tests.rs:152` |

CPU mirror owner: `postretro-render-cpu` (`crates/render-cpu/src/frame_uniforms.rs`). `build_uniform_data` serializes group-0 (128 B). `DynamicDirectParams` bytes: `render-cpu/src/sh_volume.rs:46` (`build_dynamic_direct_params_bytes(scale, isolation, has_direct)`, 16 B).

Byte-layout tests to update: `frame_uniforms.rs:277-505` (CPU offsets), `shader_tests.rs:135` (group-0 stride) + `:152` (billboard loop-bound), `mesh_pass.rs:2093`/`:2106`, `kinematic_brush.rs:1120`/`:1140`, `sh_volume.rs:1775` (DynamicDirectParams layout).

## Debug UI

Both ComboBoxes live in `DiagnosticsTab::Lighting` → `CollapsingHeader "Lighting systems"` (`debug_ui/mod.rs:320-323`), inside `draw_lighting_tab`. Order: Ambient Floor + Indirect Scale sliders; Dynamic Direct Scale slider → "Dynamic Direct Isolation" ComboBox (`:356`); Direct SH Delta / Animated Direct SH Delta override blocks; Probe Occlusion checkbox; "Lighting Isolation" ComboBox (`:460`). Renderer state: `renderer_state.rs:48-57` (`set/get lighting_isolation`), `:221-228` (`set/get dynamic_direct_isolation`); fields at `renderer_types.rs:812`, `:817`.

## Overlap: in-progress `emissive-surfaces-bloom`

`context/plans/in-progress/emissive-surfaces-bloom` concurrently edits `forward.wgsl` and `kinematic_brush.wgsl` (adds an additive emissive term), moves scene targets to `Rgba16Float` + tonemap, and consumes the final (16th) forward sampled-texture slot. Emissive is not a lighting term that exists in shader source on any path until that plan lands.
