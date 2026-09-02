# Promoted-Shadow Entity-Only Depth

## Goal

A promoted static light's pool slot holds dynamic-occluder depth only; the
static world is never rendered into it. World receivers attenuate the
reconstructed direct term by baked visibility × entity visibility instead of
differencing two occlusion estimates, which deletes the rake-angle striping
class, the dead zone, the wide kernel, and every per-frame depth copy. Movers
and skinned meshes keep crisp static-world shadows by sampling the promoted
depth cache directly. Derivations: `research.md`.

## Scope

### In scope

- Promoted spot and cube pool slots: `Clear(1.0)` plus entity occluder draws
  every frame; no world draw into the pool, no `copy_texture_to_texture`.
- The promoted depth cache stays as the world-only static depth source: gains
  `TEXTURE_BINDING`, sampled array views, loses `COPY_SRC` and both copy
  functions. Its warm/cold planning and cull-dispatch skips are unchanged.
- Mover and skinned promoted-record shadowing: per-tap `min` of the pool
  compare and the cache compare. Cache layer routed through the forward
  shadowmask metadata tail (`meta1.w`); `MeshLightParams` gains
  `dynamic_light_count`.
- World-receiver union: `direct × baked_vis × (1 − entity_vis) × w`, zero dead
  zone, zero receiver offset, shared 3×3 helpers; wide sampler and its
  constants deleted.
- Cache plan runs before the metadata pack.
- `strip_point_shadow_cube` handles every marker pair, not the first.
- Falloff model shipped in `SpecLight.cone_cos.w`; `shadowmask_direct` and
  the two static spec-light loops reconstruct with `light_eval_falloff`.
- Diagnostics: mode 5 renders the new subtraction; mode 6 renders entity-only
  pool visibility. Copy counters do not exist; the cache render-skip counter
  stays.
- `context/lib/rendering_pipeline.md` amendments (§4 promoted paragraph and
  receiver-bias paragraph; §7.1 steps 6–8).

### Out of scope

- **Deleting the cache texture.** It is the only world-only static depth the
  entity paths can sample; re-rendering world depth per frame is the shadow
  raster cost `context/plans/done/perf-forward-light-cull/` records as the
  frame's bottleneck. 44 MiB VRAM stays; the cache is the runtime's only
  source for world-onto-entity occlusion at near-tier resolution, and the
  owner's cost axis is per-frame bandwidth and ALU, not VRAM.
- Capsule or proxy occluders — a later shadow-quality tier.
- Dynamic (non-promoted) light slots and their world+entity depth;
  `WORLD_RECEIVER_BIAS_SCALE` on the forward dynamic loop;
  `MOVER_RECEIVER_BIAS_SCALE`, `SKINNED_SCALE`, `shadowBiasScale`.
- Promotion ranking, `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE`, the crossfade
  `w`, the shadowmask atlas and channel packing.
- Fog: `fog_volume.wgsl` keeps its private `sample_spot_shadow_pt`; fog
  excludes promoted slots.
- Billboard spec-light reconstruction (`billboard.wgsl` static loop hard-codes
  linear attenuation) — a separate consumer, not on the double-count path;
  same one-line fix later if wanted.
- `TIMING_PAIR_PROMOTED_DEPTH_CACHE` — its span now brackets cache fills and
  entity draws; left as is.
- The per-fragment occluder-cone gate of `promoted-shadow-entity-scoping`
  (archived): superseded by this plan for correctness; a tap-count follow-on
  only if playtesting warrants.

## Direction

**Problem.** Every occlusion fact should be computed once, by the source
that knows it best. The world-receiver union subtracts the difference between two
estimates of the same static occlusion — the bake and a runtime compare
against a pool map that contains the world — so every error in the runtime
estimate becomes darkening; at rake angles the estimate self-compares across a
±4-texel kernel and fails.

**Prior commitments.**

- `rendering_pipeline.md` §4 "Promoted static lights": entity receivers get
  `(1 − w) × baked direct SH + w × runtime term × pool shadow map`, the pool
  slot being "the near tier (true self-shadowing)". Preserved: the runtime
  term's shadow factor still carries world and entity occlusion, now from two
  maps combined per tap.
- Same paragraph: "A promoted slot's world depth is cached." Preserved; the
  cache becomes the sampled source rather than the copy source.
- Same paragraph: the union is "the baked-visibility-minus-pool-visibility
  delta, dead-zoned and renormalized". Replaced by the attenuation form. This
  is the divergence: the double-count invariant ("runtime static→static
  shadowing stays exactly zero") is met by construction — no world in the map
  — rather than by threshold.
- §7.1 steps 7–8: "a `Depth32Float` texture copy initializes the live pool
  layer before every … occluder draws into the live pool with `LoadOp::Load`"
  and "The copy is the occupied-face initialization baseline". Replaced: the
  baseline is `Clear(1.0)`, as for dynamic slots.
- `context/plans/done/static-light-shadowmask-world-receipt/` Task 3 widened
  the union kernel to 5×5 at 2 texels to approach the baked ramp of static
  occluders in the pool map. That warrant is gone with the static occluders;
  the union uses the shared 3×3 so an entity's shadow softens identically on
  the floor and on the mover beside it (`research.md`, "Wide kernel retired").
- `context/plans/done/pool-shadow-receiver-bias/`: the world receiver class
  keeps its bias on the forward dynamic loop, which still samples world depth;
  the union path passes zero because the world is never in the promoted map.
- `context/plans/drafts/shadowmask-no-drop-atlas/` claims `meta1.z`; this plan
  claims `meta1.w`. Whichever lands second rebases the packer and decode.
- `context/plans/drafts/animated-light-shadow-promotion/` re-renders world
  depth per frame for direction-animated promoted lights. After this plan the
  target of that render is the light's cache layer, never the pool slot; the
  draft carries the cross-reference.

**Alternatives rejected.** Static visibility for entity receivers from the
per-light direct-SH delta tiles: the subtraction is a compute compose
(`direct_sh_compose.wgsl`), the fragment stage binds only the composed atlas,
and the tiles are 1 m probe resolution — the far-LOD blur promotion exists to
replace. A bias formula change (`δ = t·(reach·sinθ + c)`) fixes the stripes in
four ALU but keeps the copy, the 25-tap kernel, and a threshold-based
invariant. Full reasoning in `research.md`.

**Placement.** Renderer-owned throughout (all wgpu). The falloff code mapping
is a shared `pub fn` in `postretro-lighting`, where both packers live. Non-renderer edits: the falloff code in `postretro-lighting` and the mode-6
label in `postretro-render-cpu` (`frame_uniforms.rs`).

**Foreclosures.** None material. Every surface this plan changes is
per-frame GPU state or a runtime buffer layout: two BGL entries, one metadata
lane, a 16→32 B uniform, one `SpecLight` lane. No PRL section changes.

## Acceptance criteria

- [ ] On `combat-demo.prl` with an enemy under a promoted light, mode 5 is
      black on every world surface outside the enemy's silhouette — raked
      floors, walls under a downward light, baked penumbrae, chart seams — from
      four vantage points (light-side, opposite, grazing along the floor, and
      overhead); inside the silhouette, at full promotion weight, it reads the
      light's direct term scaled by baked visibility. Dev maps compile with
      `prl-build`; modes 5 and 6 need `--features dev-tools`.
- [ ] Mode 6 reads white on all world surfaces outside entity silhouettes
      under promoted lights on `combat-demo.prl` and `kinematic-platform.prl`
      (`closet-reveal` has no promotable light: its bright spot is a script
      target and the selector requires no animation). The mode-6 dev label
      reads entity-only pool visibility.
- [ ] On `combat-demo.prl`, the raked floor and the wall under each promoted
      spot show no texel striping at close range with the enemy in the
      light's influence; a mover on `kinematic-platform.prl` under a promoted
      spot looks as before this plan.
- [ ] An enemy standing in a static shadow (wall or pillar occluding a
      promoted light) at full promotion weight is as dark as before this plan,
      and an enemy crossing a static shadow edge shows the same crisp edge;
      manual A/B against a pre-change build on `combat-demo.prl`.
- [ ] After Task 1 alone, with the pool still holding merged depth, rendered
      output on `combat-demo.prl` and `campaign-test.prl` (movers and skinned
      meshes under promoted lights) is indistinguishable from before (dev A/B
      by toggling the cache compare).
      Task 2 deletes the toggle.
- [ ] An enemy's shadow on floor and wall stays attached at the contact
      (no detachment, no gap) under a promoted spot (`combat-demo.prl`) and a
      promoted point light (`campaign-test.prl`, which needs
      `CUBE_ARRAY_TEXTURES`).
- [ ] A mover at rest against world geometry on `kinematic-platform.prl`
      casts and receives under a promoted light as before; its contact seam
      shows no leak.
- [ ] No `copy_texture_to_texture` is issued for any promoted slot: the copy
      functions do not exist and a source-pin test asserts the cache module
      source contains no `COPY_SRC` token (comments included, so Task 2
      rewords them). Both halves are review gates.
- [ ] A warm promoted slot issues no world draw into the pool and no cull
      dispatch; the existing cache tests
      (`warm_promoted_spot_skips_world_render_and_cull_dispatch`,
      `cube_warm_skip_counts_each_face_sub_region`,
      `slot_reassignment_invalidates_cache_layer`) stay green.
- [ ] On an adapter without `CUBE_ARRAY_TEXTURES`, the skinned and kinematic
      shaders validate with both cube-sampling bodies stripped; the strip test
      covers a source with two marker pairs.
- [ ] Mesh and kinematic group-2 fragment sampled-texture counts are 2 without
      cube support and 4 with; pinned.
- [ ] A metadata-pack test asserts `meta1.w` equals the record's cache layer
      (spot) or cube index (cube); a test on the GPU-free cache-layer helper
      asserts that a plan lacking a layer for a record removes that record,
      zeroes its weight, and packs no tail entry for it.
- [ ] Skinned-mesh light-params upload is 32 bytes with `dynamic_light_count`
      at offset 16; byte-layout test updated; a naga span test on `MeshLightParams` (sibling of
      `kinematic_light_params_wgsl_layout_matches_rust_upload`) pins 32 bytes
      with `dynamic_light_count` at offset 16.
- [ ] `SpecLight.cone_cos.w` carries the falloff code (0 Linear,
      1 InverseDistance, 2 InverseSquared) — packer test; all three forward
      reconstruction sites call `light_eval_falloff` with it — shader-source
      pin. Every shipped dev map renders unchanged — a review gate argued
      from `grep`: every light authors `delay 0` or omits it, and the
      translator defaults absent `delay` to Linear.
- [ ] Shader pins: the dead-zone/kernel test is replaced by one pinning the
      attenuation expression, the zero union bias constant, and the absence of
      `shadowmask_sample_spot_shadow_wide`;
      `forward_shader_shadowmask_visualization_mode_is_wired`, the
      promoted-count/metadata-tail test, the multilayer-fallback test and the
      count-split loop-bound test pass unchanged; the four
      `pack_forward_shadowmask_metadata` tests are updated for `meta1.w`.
- [ ] Fog shader source is byte-identical (`git diff` shows no change to
      `fog_volume.wgsl`).
- [ ] `rendering_pipeline.md` carries no "Decided, not yet built" clause for
      this plan; §4, §7.1 and the §9 group-2 row read as current; the
      one-source-per-receiver sentence counts the pool slot and the cache as
      one source.

## Tasks

### Task 1: Sampleable cache and per-tap combined entity shadowing

Thin slice; output-identical. In `crates/renderer/src/render/promoted_depth_cache.rs`,
`PromotedDepthCache::new` gains `cube_array_supported: bool`, creates both
textures with `RENDER_ATTACHMENT | TEXTURE_BINDING` (drop `COPY_SRC`), and adds
a `D2Array` sampled view over all `MAX_PROMOTED_SPOT` spot layers and, iff
`cube_array_supported`, a `CubeArray` sampled view over the
`MAX_PROMOTED_CUBE × CUBE_FACES` cube layers; expose `spot_sampled_view()` and
`cube_sampled_view() -> Option<&TextureView>`. Both construction sites pass cube-array support — `renderer_full_init.rs`
(the `!entity_shadow_indices.is_empty()` arm, passing the local
`cube_array_supported`; move the cache construction above both
`rebuild_light_bind_group` calls, which today precede it, keeping the
`FullRenderer` field initializer pointed at the local) and
`renderer_resources.rs` (the empty→non-empty re-creation, passing
`full.cube_shadow_pool.is_some()`; it precedes the bind-group rebuilds in the
same function — keep that order; the non-empty→non-empty path is
`reset_level` and keeps the texture). Add group-2 entries in
`mesh_light_bind_group_layout_entries` (`mesh_pass.rs`) and
`light_bind_group_layout_entries` (`kinematic_brush.rs`): binding 9
`Depth`/`D2Array`/FRAGMENT unconditionally, binding 10 `Depth`/`CubeArray`
iff `cube_array_supported`, following the existing binding-8 pattern. Extend
`rebuild_light_bind_group` on both with `promoted_spot_cache: &TextureView`
and `promoted_cube_cache: Option<&TextureView>`, extending the Some-iff-layout
assert; update all four callers (`renderer_full_init.rs` and
`renderer_resources.rs`, mesh and kinematic). When `promoted_depth_cache` is
`None` — no entity-shadow selection, hence no promoted records — bind the
spot pool's array view and the cube pool's `sampling_view` in its place; they are
never sampled because no record carries a cache layer. Declare in
`skinned_mesh.wgsl` and `kinematic_brush.wgsl`:
`@group(2) @binding(9) var promoted_spot_depth_cache: texture_depth_2d_array;`
and `@group(2) @binding(10) var promoted_cube_depth_cache: texture_depth_cube_array; // CUBE_SHADOW_BINDING`.
Add `crates/renderer/src/shaders/shadow_sample_static_cache.wgsl`, appended
after `shadow_sample.wgsl` in `SKINNED_MESH_SHADER_SOURCE` (`mesh_pass.rs`
`concat!`) and in the kinematic source composition, defining
`sample_spot_shadow_with_static(slot, cache_layer: i32, light_pos, world_pos,
receiver_normal, bias_scale, light_proj)` and
`sample_point_shadow_with_static(slot, cache_index: i32, light_pos, world_pos,
receiver_normal, bias_scale, far_range)`: same projection, offset, and 3×3
loop as the shared helpers — copied, not factored out of `shadow_sample.wgsl`,
whose `receiver_offset` count
`receiver_bias_factor_scales_the_entire_shared_normal_offset` pins — each tap
`min(textureSampleCompare(spot_shadow_depth, …), textureSampleCompare(promoted_spot_depth_cache, …, cache_layer, …))`
when `cache_layer >= 0`, pool-only otherwise; the cube body sits between its
own `// CUBE_SHADOW_BODY_BEGIN` / `_END` markers, and no comment in the file
spells either marker token outside that one pair
(`forward_wgsl_no_cube_variant_strips_binding_and_validates` asserts neither
survives the strip). Change
`strip_point_shadow_cube` (`pipeline_layout.rs`) to replace every
BEGIN…END pair, keeping its mismatch panic, and extend its test with a
two-pair source. In `crates/renderer/src/render/shadowmask.rs`,
`pack_forward_shadowmask_metadata` gains `cache_layers: &[i32]`
(records-parallel; spot layer, or cube layer base divided by `CUBE_FACES`;
−1 when absent) and writes it as `meta1.w` in place of the `0.0`; update the
four existing tests that call it. In `renderer_light_slots.rs` `update_promoted_static_weights_and_records`,
after the record loop and before the weight-scratch upload and the
`total_light_count` recompute that close the function: call
`PromotedDepthCache::plan_frame` on `full.promoted_depth_cache` (assert it is
`Some` whenever records exist),
store the plan and its counters where `record_scene_passes` in
`renderer_render_frame.rs` stores them today — the per-frame zeroing of
`promoted_depth_cache_cull_dispatch_skips` and
`promoted_entity_occluders_submitted` moves with the plan call, since the
shadow passes accumulate both (that block reduces to the
`render_world == false` reset; the
`render_world && cache.is_none()` reset arm moves with the plan call; the
early return when `shadow_candidate_lights` is empty leaves the plan at its
install defaults), build
`cache_layers` via `spot_for_slot` / `cube_for_slot`, and for a record with no
plan entry — handled here so the zeroed weight reaches the upload, the
recompute counts only the remaining records, and the influence and metadata
pack in `update_dynamic_light_slots` sees the final list; unreachable because `MAX_PROMOTED_SPOT` / `MAX_PROMOTED_CUBE` are
both the ranker's `promoted_cap` and the cache layer counts; `log::warn!`
once, never assert, so AC 12 can drive the branch with a fabricated plan
(a `bool` latch on `FullRenderer`, reset in `install_level_geometry`, keeps
it to one warning per level; the light's weight state is untouched, so the
ranker re-seeds it and the branch repeats each frame). Removal, zeroing, and
−1 packing live in a GPU-free helper taking the records, the weights, the
plan, and the compare flag and returning `cache_layers: Vec<i32>` with the
plan's `promoted_count` set from the surviving records, following
the `plan_frame_with_layers` seam in `promoted_depth_cache.rs`; the method
calls it and uploads —
remove it from `promoted_static_records` and
write 0.0 to `promoted_static_weights[selection_index]` before the weight
buffer upload and the `total_light_count` recompute. Add
`dynamic_light_count: u32` (plus three pad words) to `MeshLightParams` in
`mesh_pass.rs` and its WGSL mirror, grow `light_params_buffer` to 32 bytes,
extend `MeshPass::write_light_params` and both callers in
`renderer_render_frame.rs` to pass `full.light_count`; update the byte-layout
test. In `accumulate_dynamic_direct` of both shaders, for
`i >= dynamic_light_count` (kinematic already has the field): `p = i −
dynamic_light_count`, `meta_index = mesh_light_params.light_count + p *
SHADOWMASK_META_VEC4S_PER_RECORD` (kinematic: `kinematic_light_params.light_count`)
— that field is `full.total_light_count`, the same base
`shadowmask_union_subtraction` decodes from `uniforms.total_light_count`; the
loop bound `select(0u, mesh_light_params.light_count, use_dynamic)` stays
verbatim for `count_split_shader_consumers_use_expected_loop_bounds` — guard `meta_index + 1u < arrayLength(&light_influence)` (on failure
`cache = -1`), read `cache = i32(light_influence[meta_index + 1u].w)` —
`meta1.w` carries the spot layer or the cube index and the helpers take it
unchanged; both entity shaders declare `SHADOWMASK_META_VEC4S_PER_RECORD` and
the stride is that constant, not a literal — and route the spot and cube
shadow calls through the `_with_static`
helpers with it; dynamic-tier lights keep the shared helpers. Update the pins Task 1 breaks:
`mesh_group2_sampled_texture_count_recorded_for_both_cube_variants` (2/4),
`mesh_group2_shadow_bindings_match_both_cube_variants` and
`mesh_group2_bgl_matches_shader_bindings` (binding ranges grow to 9 / 10),
`skinned_mesh_pipeline_fragment_texture_budget_includes_shared_emissive_binding`
and `kinematic_pipeline_fragment_texture_budget_includes_emissive` (group-2
count 2 → 4 with cube support),
`receiver_bias_factor_scales_the_entire_shared_normal_offset` in
`lighting/spot_shadow.rs` (the `SKINNED_SCALE * bias_factor` occurrence count
doubles), `mesh_light_params_is_sixteen_bytes` and
`write_light_params_places_ambient_floor_at_bytes_twelve_to_sixteen` (32
bytes). The strip test is
`forward_wgsl_no_cube_variant_strips_binding_and_validates`
(`render/tests/shader_pipeline_tests.rs`) and its fog twin in `fog_pass.rs`;
extend the former with a two-pair source. The two
`*_fragment_texture_budget_*` tests also pin the no-cube group-2 count (1 → 2)
and the totals (9 → 11 with cube support, 8 → 9 without);
`mesh_group2_bgl_matches_shader_bindings` expects `[0..=8, 9, 10]` with cube
support and `[0..=7, 9]` without. Add a kinematic no-cube naga validation test
beside the existing skinned one.
Dev A/B: a `POSTRETRO_PROMOTED_CACHE_COMPARE=0` env toggle read once at init
into a `bool` on the renderer alongside the cache, consulted by the
`cache_layers` build to pack −1 for every record; with it on and off, output
is identical while the pool still holds merged depth. Task 2 deletes the
toggle — after it, packing −1 drops world occlusion from entity receivers.

### Task 2: Entity-only promoted slots and the attenuation union

In `crates/renderer/src/render/renderer_shadow_passes.rs`, promoted spot
branch: keep the `plan.needs_world_render` cache-fill pass unchanged; delete
the `copy_spot_to_pool` call; make the "Promoted Spot Entity Shadow Depth
Pass" run for every occupied promoted slot with `LoadOp::Clear(1.0)`, keeping
the entity draws inside it gated exactly as today
(`slot_entity_eligible[slot] && (mesh_frame_plan.is_some() || !mover_occluder_aabbs.is_empty())`,
then per-occluder cone cull) — the clear is never gated, mirroring
`cube_face_needs_clear`. Promoted cube branch: same per face; delete the
`copy_cube_face_to_pool` call. Delete both copy functions from
`promoted_depth_cache.rs`, the two texture fields they were the only readers
of (the views own the textures), and every comment mention of `COPY_SRC`. In `crates/renderer/src/shaders/forward.wgsl`:
delete `shadowmask_sample_spot_shadow_wide`, `SHADOWMASK_SPOT_KERNEL_RADIUS`,
`SHADOWMASK_SPOT_KERNEL_TEXELS`, `SHADOWMASK_SPOT_VISIBILITY_DEAD_ZONE`,
`SHADOWMASK_POINT_VISIBILITY_DEAD_ZONE`, `shadowmask_dead_zone`, and
`shadowmask_visibility_difference`; add
`const SHADOWMASK_UNION_RECEIVER_BIAS_SCALE: f32 = 0.0;` with a comment that
world receivers never appear in a promoted map; `shadowmask_shadow_visibility`
calls the shared `sample_spot_shadow` and `sample_point_shadow` with that
constant, keeping its two slot-range guards verbatim (pinned by
`forward_shader_shadowmask_union_uses_promoted_count_and_safe_metadata_tail`); in `shadowmask_union_subtraction` the skip keeps its mode-6 exemption and
becomes `uniforms.sdf_shadow_mode != SHADOWMASK_RAW_POOL_VISIBILITY_MODE &&
baked_vis <= 0.0`, and the accumulation becomes
`direct.value * (baked_vis * (1.0 - shadow_map_vis)) * weight` through a
`shadowmask_attenuation(baked_vis, entity_vis)` helper. Leave the promoted
count loop, metadata decode, `SHADOWMASK_META_VEC4S_PER_RECORD`, mode 5/6
plumbing, and the three `sample_shadowmask_atlas(` sites untouched. Replace
`forward_shader_shadowmask_dead_zone_matches_each_pool_kernel` in
`shader_tests.rs` with a pin on the attenuation expression, the zero constant,
and the absence of the deleted identifiers; add a `promoted_depth_cache.rs`
source pin that the module declares no `COPY_SRC`. Update the mode-6 label
(`SdfShadowMode::ShadowmaskRawPoolVisibility` in
`crates/render-cpu/src/frame_uniforms.rs`) to say entity-only pool
visibility. Delete the `POSTRETRO_PROMOTED_CACHE_COMPARE` toggle and its
field.

### Task 3: Falloff model in the static direct reconstruction

In `crates/lighting/src/lib.rs`, make the private `falloff_model_u32` the
`GpuLight` packer uses `pub` as `falloff_model_code(FalloffModel) -> u32` and
call it from both packers. In `crates/lighting/src/spec_buffer.rs` `pack_spec_lights`,
write `falloff_model_code(light.falloff_model) as f32` at byte 60 in place of
`0.0`; update the packer's layout doc and add a test for byte 60. In
`forward.wgsl`, update the `SpecLight.cone_cos` comment, and in
`shadowmask_direct`, the static SDF spec-light loop, and the static-specular
chunk loop (both loops in `fs_main`) replace the linear attenuation —
`max(1.0 - dist / max(range, 0.001), 0.0)` in `shadowmask_direct`,
`select(1.0, max(1.0 - dist / max(range, 0.001), 0.0), range > 0.0)` in each
loop — with
`light_eval_falloff(dist, range, u32(round(sl.cone_cos.w)))`, keeping each
site's existing `range <= 0.0` handling. Model 0 in `light_eval_falloff`
(`light_eval.wgsl`) is the same linear expression, so shipped content — every
light in `content/dev/maps` authors `delay 0` or omits it, and the translator
(`quake_map.rs`, `Some(0) | None => Linear`) defaults absent `delay` to
Linear — renders unchanged. Add a shader-source pin that
all three sites call `light_eval_falloff(` with `cone_cos.w` and that the
hard-coded linear expression is absent from `forward.wgsl`.

### Task 4: Pipeline documentation

`context/lib/rendering_pipeline.md` already states this plan's contract —
the compute-once thesis, the attenuation union, entity-only slots, the
sampled cache, the zero union offset, the §7.1 clear-and-draw steps, and the
§9 group-2 row — each followed by a **Decided, not yet built** clause
describing the shipped path. Delete those clauses (two in §4, two in §7.1,
one in the §9 row) so the paragraphs read as current. No other change.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice, falsifies the binding budget,
the strip transform, the plan-before-pack ordering, and the pixel-identical
warrant while the pool still holds merged depth.
**Phase 2 (sequential):** Task 2 — consumes Task 1's cache compare; only then
can the copy and the pool-side world depth go.
**Phase 3 (concurrent):** Task 3 (`spec_buffer.rs`, `lighting/lib.rs`,
`forward.wgsl` after Task 2's edits land), Task 4 (`context/lib` only).

## Rough sketch

```wgsl
// Proposed design — shadow_sample_static_cache.wgsl (entity consumers only)
fn sample_spot_shadow_with_static(
    slot_index: u32, cache_layer: i32, light_pos: vec3<f32>, world_pos: vec3<f32>,
    receiver_normal: vec3<f32>, bias_scale: f32, light_proj: mat4x4<f32>,
) -> f32 {
    // … identical projection/offset/uv/early-outs to sample_spot_shadow …
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let o = uv + vec2<f32>(f32(dx), f32(dy)) * step;
            var tap = textureSampleCompare(spot_shadow_depth, spot_shadow_compare, o, i32(slot_index), light_ndc.z);
            if cache_layer >= 0 {
                tap = min(tap, textureSampleCompare(promoted_spot_depth_cache, spot_shadow_compare, o, cache_layer, light_ndc.z));
            }
            lit = lit + tap;
        }
    }
    return lit / 9.0;
}
```

```wgsl
// Proposed design — forward.wgsl union
fn shadowmask_attenuation(baked_vis: f32, entity_vis: f32) -> f32 {
    return baked_vis * (1.0 - entity_vis);
}
// … in shadowmask_union_subtraction:
if uniforms.sdf_shadow_mode != SHADOWMASK_RAW_POOL_VISIBILITY_MODE && baked_vis <= 0.0 { continue; }
let shadow_map_vis = shadowmask_shadow_visibility(pool_kind, slot, sl, world_pos, mesh_n);
out.raw_pool_visibility = min(out.raw_pool_visibility, shadow_map_vis);
out.subtraction = out.subtraction + direct.value * shadowmask_attenuation(baked_vis, shadow_map_vis) * weight;
```

Nearest comparison sampling makes each tap 0 or 1, so the per-tap `min` is
the compare against the merged depth. The union's zero offset is safe because
the only depths in a promoted slot belong to occluders that are not the
receiver.

## Orderings

| Scenario | Ordering | Expected |
|---|---|---|
| Cold cache layer | plan says `needs_world_render`; cache fill in shadow passes; forward/mesh sample later the same frame | Entity receivers read the freshly filled layer; world receivers never read the cache |
| Slot reassigned to another light | `assign_layer` claims a fresh layer, `warm = false` | Cache refilled before sampling; `meta1.w` carries the new layer |
| Promoted slot, entity gate fails | No mesh plan and no movers | `Clear(1.0)` still runs; entity term unshadowed by the pool, world subtraction exactly zero, cache still applies to entity receivers |
| Demote sticky window | Record with `w > 0`, occluder gone | Pool slot cleared each frame; subtraction zero; entity receivers keep world occlusion from the cache while `w` fades |
| Record with no cache layer | Unreachable by cap; defensive branch inside `update_promoted_static_weights_and_records` before the weight upload | Record dropped for the frame, weight 0.0 uploaded, `total_light_count` excludes it, metadata tail packed without it; `clear_zero_weight_promoted_assignments` then releases its slot, so no depth pass runs for it; compose keeps baked SH — never brighter than baked. The ranker re-seeds the light next frame and the branch repeats; one warning per level |
| No entity-shadow selection | `promoted_depth_cache == None` | Pool views bound at b9/b10; no records; no sample |
| Level install | Cache re-created, then mesh and kinematic bind groups rebuilt | Bind groups reference the new cache views |
| `render_world == false` | No slot update, no pack | Plan reset; nothing sampled |
| Two promoted lights on one entity fragment | Loop over both records | Each record combines its own slot with its own cache layer |
| Cube slot, six faces | All faces of an occupied slot render in one frame (`face_matrices` set per slot) | Cache marked warm at face 6; sampled complete |
| Adapter without cube arrays | Both cube bodies stripped | Spot path unaffected; promoted point candidates are never assigned (`update_cube_light_slots` returns empty), so no cube record, no cube plan, no `meta1.w`; entity receivers use baked SH only, as today |
| Init-time bind-group build | `PromotedDepthCache::new` precedes both `rebuild_light_bind_group` calls in `renderer_full_init.rs` | First frame's mesh and kinematic group-2 bind groups reference cache views (or pool views when `None`) |
| Level with no shadow candidates | `update_dynamic_light_slots` early return every frame | Plan and counters are the install defaults; no promoted branch runs; `LayerState` untouched |
| `render_world == false` then `true` | Frame A: no slot update, plan reset to default, no passes. Frame B: `plan_frame` against the untouched `LayerState` | Warm layers stay warm across the gap; no refill; `meta1.w` re-derived in frame B |
| Layer index reused within one `plan_frame` | `retain_active_layers` frees layer k (light A gone) → `assign_layer` gives k to light B with `warm = false` | B's plan is cold; k is refilled this frame before sampling; A's stale depth is never sampled |
| Cap-boundary eviction and return | Frame N: R evicted → layer freed. Frame N+1: R re-promoted → new layer, cold | One world render per flip; `w` continues from its decayed value (existing ranker behavior) |
| Records non-empty → empty | Influence/metadata upload skipped (`records.is_empty()`); buffer retains last frame's tail | `total_light_count == light_count`, so forward and entity loops never read the stale tail |
| Same-selection level reload | `reset_level` clears `LayerState`; texture kept; bind groups rebuilt | Every layer cold on the first frame; full refill; no stale sampling |
| No-geometry level (`draw_world == false`) | Cache-fill pass runs with `Clear(1.0)` and no draw; `mark_*_world_rendered` still called | Layer warm with far-plane depth; entity receivers compare against 1.0 |
| Dev toggle lifetime | `POSTRETRO_PROMOTED_CACHE_COMPARE=0` read at init | Exists only between Task 1 and Task 2; no release path packs −1 |
| Mode 6 on a baked-dark receiver | Mode 6, `baked_vis == 0`, entity on the ray | Skip exempted; `raw_pool_visibility` 0 inside the silhouette, 1 outside; subtraction exactly 0 |
| Per-frame counter resets | Frame N tallies occluders and cull skips in the shadow passes; frame N+1 enters `update_promoted_static_weights_and_records` | Both counters read 0 before N+1's shadow passes |
| Runtime light spawn shifts the tail | `upload_bridge_lights` grows `light_count`; same-frame re-pack at the new `total_light_count`; `write_light_params` carries the new total | `meta_index` resolves to the fresh record in all three shaders |
| Toggle off, cold layer (Task 1 window) | Toggle packs −1; plan is cold → fill still recorded | Fill and copy run; helpers take the pool-only branch; output identical |
| Capture path | `capture_frame_indirect` → `record_scene_passes` | Same plan, pack, fill, sample order; no second plan site |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A promoted pool slot with a live record never holds static world depth | Task 2 | Any future world draw or copy into a promoted slot; the unreachable defensive drop releases its slot through the zero-weight clear the same frame | AC 2, 8, 9 |
| Entity receivers see world+entity occlusion per tap identical to a merged map | Task 1 | Any change to sampler filter mode (Nearest) or to cache/pool resolution parity | AC 4, 5 |
| World subtraction is exactly zero where no dynamic occluder shadows the fragment | Task 2 | Any nonzero receiver offset that moves a world fragment into an entity silhouette; any content in the slot other than entity depth | AC 1, 3 |
| Every packed promoted record carries a valid cache layer | Task 1 (plan before pack) | Any writer of `promoted_static_records` after `plan_frame`; any change to `promoted_cap` vs cache layer counts; the Task 1 dev toggle while it exists | AC 12 |
| The cache holds world depth only | Existing (movers never enter the cache); preserved by Task 2 leaving the fill pass unchanged | Any entity draw targeting a cache view | AC 4 |
| Static direct reconstruction uses the authored falloff model | Task 3 | Billboard static loop (out of scope) | AC 14 |

## Decisions

- **The cache stays.** It is the light's-eye static world depth the entity
  paths sample for world-onto-entity occlusion at the near tier; the bake
  cannot supply that fact for bodies it never saw, and the probes are the far
  tier. Deleting it would collapse promotion's near tier into the tier it
  exists to replace. VRAM is not the cost axis.
- **Entity shadows on world use the shared 3×3.** Softness belongs to the
  shadow, not the receiver: one silhouette softens identically on the floor
  and on the mover beside it. Kernel radius is a quality knob for the later
  player-facing shadow-quality slider, applied to every consumer at once
  through `SPOT_SHADOW_PCF_RADIUS`; no union-only kernel returns.
- **`promoted-shadow-entity-scoping` is archived** under `done/` with a
  superseded banner. Its copy-elision half is void (promotion requires an
  occluder in the influence) and its per-fragment cone gate is a tap-count
  optimization unmeasurable on the owner's hardware, with
  `perf-forward-light-cull` as a shelved prior of the same shape. It can be
  re-drafted against this plan's 9-tap baseline if playtesting ever warrants.
