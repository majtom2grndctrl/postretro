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
  the static SDF spec-light loop reconstruct with `light_eval_falloff`.
- Diagnostics: mode 5 renders the new subtraction; mode 6 renders entity-only
  pool visibility. Copy counters do not exist; the cache render-skip counter
  stays.
- `context/lib/rendering_pipeline.md` amendments (§4 promoted paragraph and
  receiver-bias paragraph; §7.1 steps 6–8).

### Out of scope

- **Deleting the cache texture.** It is the only world-only static depth the
  entity paths can sample; re-rendering world depth per frame is the shadow
  raster cost `context/plans/done/perf-forward-light-cull/` records as the
  frame's bottleneck. 44 MiB VRAM stays. Owner decision recorded in Open
  questions.
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
  (draft): superseded by this plan for correctness; optional tap-count
  follow-on.

## Direction

**Problem.** The world-receiver union subtracts the difference between two
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
moves into `postretro-lighting` because two packers there already need it.

**Foreclosures.** None material. Every surface this plan changes is
per-frame GPU state or a runtime buffer layout: two BGL entries, one metadata
lane, a 16→32 B uniform, one `SpecLight` lane. No PRL section changes.

## Acceptance criteria

- [ ] On `combat-demo.prl` with an enemy under a promoted light, mode 5 is
      black on every world surface outside the enemy's silhouette — raked
      floors, walls under a downward light, baked penumbrae, chart seams — at
      every camera angle; inside the silhouette it reads the light's direct
      term scaled by baked visibility.
- [ ] Mode 6 reads white on all world surfaces outside entity silhouettes
      under promoted lights on `combat-demo.prl` and `closet-reveal.prl`.
- [ ] On `closet-reveal.prl`, the wall beside the closed door shows no texel
      striping at close range under the static spotlight; the door's own
      appearance is unchanged from before this plan.
- [ ] An enemy standing in a static shadow (wall or pillar occluding a
      promoted light) at full promotion weight is as dark as before this plan,
      and an enemy crossing a static shadow edge shows the same crisp edge;
      manual A/B against a pre-change build on `combat-demo.prl`.
- [ ] After Task 1 alone, with the pool still holding merged depth, rendered
      output on `combat-demo.prl` and `stress-warren-lit.prl` is
      indistinguishable from before (dev A/B by toggling the cache compare).
- [ ] An enemy's shadow on floor and wall stays attached at the contact
      (no detachment, no gap) under a promoted spot and a promoted point light.
- [ ] A mover docked against a wall casts and receives under a promoted light
      as before; its contact seam shows no leak.
- [ ] No `copy_texture_to_texture` is issued for any promoted slot: the copy
      functions do not exist and a source-pin test asserts the cache module
      declares no `COPY_SRC` usage.
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
      (spot) or cube index (cube), and that a frame plan lacking a layer for a
      record removes that record and zeroes its weight for the frame.
- [ ] Skinned-mesh light-params upload is 32 bytes with `dynamic_light_count`
      at offset 16; byte-layout test updated; the WGSL mirror matches.
- [ ] `SpecLight.cone_cos.w` carries the falloff code (0 Linear,
      1 InverseDistance, 2 InverseSquared) — packer test; both forward
      reconstruction sites call `light_eval_falloff` with it — shader-source
      pin. Every shipped dev map renders unchanged (all lights author
      `"delay" "0"`).
- [ ] Shader pins: the dead-zone/kernel test is replaced by one pinning the
      attenuation expression, the zero union bias constant, and the absence of
      `shadowmask_sample_spot_shadow_wide`; the promoted-count/metadata-tail
      test and the multilayer-fallback test pass unchanged; the count-split
      loop-bound test passes unchanged.
- [ ] Fog shader source is byte-identical.
- [ ] `rendering_pipeline.md` §4 and §7.1 describe the entity-only slot, the
      attenuation form, the sampled cache, and the `Clear(1.0)` baseline; the
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
`cube_sampled_view() -> Option<&TextureView>`. Both constructors pass
`full.cube_shadow_pool.is_some()` — `renderer_full_init.rs` (the
`entity_shadow_indices.is_empty()` arm) and `renderer_resources.rs` (level
install re-creation, which precedes the bind-group rebuilds in the same
function — keep that order). Add group-2 entries in
`mesh_light_bind_group_layout_entries` (`mesh_pass.rs`) and
`light_bind_group_layout_entries` (`kinematic_brush.rs`): binding 9
`Depth`/`D2Array`/FRAGMENT unconditionally, binding 10 `Depth`/`CubeArray`
iff `cube_array_supported`, following the existing binding-8 pattern. Extend
`rebuild_light_bind_group` on both with `promoted_spot_cache: &TextureView`
and `promoted_cube_cache: Option<&TextureView>`, extending the Some-iff-layout
assert; update all four callers (`renderer_full_init.rs` and
`renderer_resources.rs`, mesh and kinematic). When `promoted_depth_cache` is
`None` — no entity-shadow selection, hence no promoted records — bind the
spot pool's array view and the cube pool's array view in its place; they are
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
loop as the shared helpers, each tap
`min(textureSampleCompare(spot_shadow_depth, …), textureSampleCompare(promoted_spot_depth_cache, …, cache_layer, …))`
when `cache_layer >= 0`, pool-only otherwise; the cube body sits between its
own `// CUBE_SHADOW_BODY_BEGIN` / `_END` markers. Change
`strip_point_shadow_cube` (`pipeline_layout.rs`) to replace every
BEGIN…END pair, keeping its mismatch panic, and extend its test with a
two-pair source. In `crates/renderer/src/render/shadowmask.rs`,
`pack_forward_shadowmask_metadata` gains `cache_layers: &[i32]`
(records-parallel; spot layer, or cube layer base divided by `CUBE_FACES`;
−1 when absent) and writes it as `meta1.w` in place of the `0.0`; update both
existing tests. In `renderer_light_slots.rs` `update_dynamic_light_slots`,
immediately after `update_promoted_static_weights_and_records` and before the
light upload and metadata pack: call `PromotedDepthCache::plan_frame` on
`full.promoted_depth_cache` (assert it is `Some` whenever records exist),
store the plan and its counters where `renderer_render_frame.rs` stores them
today (that block reduces to the `render_world == false` reset), build
`cache_layers` via `spot_for_slot` / `cube_for_slot`, and for a record with no
plan entry — `debug_assert!` unreachable, since the ranker's `promoted_cap`
equals the cache layer count — remove it from `promoted_static_records` and
write 0.0 to `promoted_static_weights[selection_index]` before the weight
buffer upload and the `total_light_count` recompute. Add
`dynamic_light_count: u32` (plus three pad words) to `MeshLightParams` in
`mesh_pass.rs` and its WGSL mirror, grow `light_params_buffer` to 32 bytes,
extend `MeshPass::write_light_params` and both callers in
`renderer_render_frame.rs` to pass `full.light_count`; update the byte-layout
test. In `accumulate_dynamic_direct` of both shaders, for
`i >= dynamic_light_count` (kinematic already has the field): `p = i −
dynamic_light_count`, `meta_index = light_count + p * 2u` where `light_count`
is the total the params carry, guard `meta_index + 1u <
arrayLength(&light_influence)`, read `cache = i32(light_influence[meta_index +
1u].w)`, and route the spot and cube shadow calls through the `_with_static`
helpers with it; dynamic-tier lights keep the shared helpers. Update
`mesh_group2_sampled_texture_count_recorded_for_both_cube_variants` (2/4).
Dev A/B: a `POSTRETRO_PROMOTED_CACHE_COMPARE=0` env toggle read once at init
that packs −1 for every record; with it on and off, output is identical while
the pool still holds merged depth.

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
`promoted_depth_cache.rs`. In `crates/renderer/src/shaders/forward.wgsl`:
delete `shadowmask_sample_spot_shadow_wide`, `SHADOWMASK_SPOT_KERNEL_RADIUS`,
`SHADOWMASK_SPOT_KERNEL_TEXELS`, `SHADOWMASK_SPOT_VISIBILITY_DEAD_ZONE`,
`SHADOWMASK_POINT_VISIBILITY_DEAD_ZONE`, `shadowmask_dead_zone`, and
`shadowmask_visibility_difference`; add
`const SHADOWMASK_UNION_RECEIVER_BIAS_SCALE: f32 = 0.0;` with a comment that
world receivers never appear in a promoted map; `shadowmask_shadow_visibility`
calls the shared `sample_spot_shadow` and `sample_point_shadow` with that
constant; in `shadowmask_union_subtraction` the skip becomes
`baked_vis <= 0.0`, and the accumulation becomes
`direct.value * (baked_vis * (1.0 - shadow_map_vis)) * weight` through a
`shadowmask_attenuation(baked_vis, entity_vis)` helper. Leave the promoted
count loop, metadata decode, `SHADOWMASK_META_VEC4S_PER_RECORD`, mode 5/6
plumbing, and the three `sample_shadowmask_atlas(` sites untouched. Replace
`forward_shader_shadowmask_dead_zone_matches_each_pool_kernel` in
`shader_tests.rs` with a pin on the attenuation expression, the zero constant,
and the absence of the deleted identifiers; add a `promoted_depth_cache.rs`
source pin that the module declares no `COPY_SRC`. Update the dev-tools label
for mode 6 to say entity-only pool visibility.

### Task 3: Falloff model in the static direct reconstruction

In `crates/lighting/src/lib.rs`, extract the `FalloffModel → u32` match the
`GpuLight` packer uses into `pub fn falloff_model_code(FalloffModel) -> u32`
and use it there. In `crates/lighting/src/spec_buffer.rs` `pack_spec_lights`,
write `falloff_model_code(light.falloff_model) as f32` at byte 60 in place of
`0.0`; update the packer's layout doc and add a test for byte 60. In
`forward.wgsl`, update the `SpecLight.cone_cos` comment, and in
`shadowmask_direct` and the static SDF spec-light loop replace the linear
`max(1.0 - dist / range, 0.0)` with
`light_eval_falloff(dist, range, u32(round(sl.cone_cos.w)))`, keeping each
site's existing `range <= 0.0` handling. Model 0 in `light_eval_falloff`
(`light_eval.wgsl`) is the same linear expression, so shipped content — every
light in `content/dev/maps` authors `"delay" "0"`, and the translator defaults
absent `delay` to Linear — renders unchanged. Add a shader-source pin that
both sites call `light_eval_falloff(` with `cone_cos.w`.

### Task 4: Pipeline documentation

In `context/lib/rendering_pipeline.md`: §4 "Promoted static lights" — replace
the union sentence with the attenuation form and state that promoted slots
hold entity depth only while the cached world depth is sampled by entity
receivers; §4 "Pool-shadow receiver bias" — the world class applies to the
forward dynamic loop, the union path offsets by zero; §4 lighting
architecture map, the one-source-per-receiver sentence — add the clause that
two depth maps compared under one technique (the pool slot and the cache,
Nearest compare per tap) are one source; §7.1 step 6 — warm
promoted slots skip cull dispatch because the cache needs no re-render; steps
7 and 8 — promoted slots clear to the far plane and draw entity occluders
only, the cache fills once per assignment and is sampled, not copied; delete
the "copy is the occupied-face initialization baseline" sentence. One clause
each; no new section.

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
if baked_vis <= 0.0 { continue; }
let entity_vis = shadowmask_shadow_visibility(pool_kind, slot, sl, world_pos, mesh_n);
out.raw_pool_visibility = min(out.raw_pool_visibility, entity_vis);
out.subtraction = out.subtraction + direct.value * shadowmask_attenuation(baked_vis, entity_vis) * weight;
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
| Record with no cache layer | Unreachable by cap; defensive branch | Record dropped for the frame, weight 0.0, compose keeps baked SH — never brighter than baked |
| No entity-shadow selection | `promoted_depth_cache == None` | Pool views bound at b9/b10; no records; no sample |
| Level install | Cache re-created, then mesh and kinematic bind groups rebuilt | Bind groups reference the new cache views |
| `render_world == false` | No slot update, no pack | Plan reset; nothing sampled |
| Two promoted lights on one entity fragment | Loop over both records | Each record combines its own slot with its own cache layer |
| Cube slot, six faces | All faces of an occupied slot render in one frame (`face_matrices` set per slot) | Cache marked warm at face 6; sampled complete |
| Adapter without cube arrays | Both cube bodies stripped | Spot path unaffected; promoted point lights read as unshadowed on entities and world, as today |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A promoted pool slot never holds static world depth | Task 2 | Any future world draw or copy into a promoted slot | AC 2, 8, 9 |
| Entity receivers see world+entity occlusion per tap identical to a merged map | Task 1 | Any change to sampler filter mode (Nearest) or to cache/pool resolution parity | AC 4, 5 |
| World subtraction is exactly zero where no dynamic occluder shadows the fragment | Task 2 | Any nonzero receiver offset that moves a world fragment into an entity silhouette; any content in the slot other than entity depth | AC 1, 3 |
| Every packed promoted record carries a valid cache layer | Task 1 (plan before pack) | Any writer of `promoted_static_records` after `plan_frame`; any change to `promoted_cap` vs cache layer counts | AC 12 |
| The cache holds world depth only | Existing (movers never enter the cache); preserved by Task 2 leaving the fill pass unchanged | Any entity draw targeting a cache view | AC 4 |
| Static direct reconstruction uses the authored falloff model | Task 3 | Billboard static loop (out of scope) | AC 14 |

## Open questions

- **Owner decision:** keep the 44 MiB cache (this plan) or accept
  probe-resolution static occlusion on entity receivers and delete it. The
  plan commits to keeping it; `research.md` states the trade.
- Whether entity shadows on world should be softer than the shared 3×3. If so,
  raise `SPOT_SHADOW_PCF_RADIUS` for every consumer rather than reintroducing a
  union-only kernel.
- Whether `promoted-shadow-entity-scoping` should be archived at promotion or
  reshaped into a tap-count follow-on over this plan.
