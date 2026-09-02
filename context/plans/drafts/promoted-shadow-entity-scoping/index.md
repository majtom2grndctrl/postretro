# Promoted-Shadow Entity Scoping

## Goal

Promoted static lights make world receivers pay for static occlusion the lightmap
already holds. Every promoted slot re-copies its cached world depth into the pool
each frame whether or not anything draws on top of it, and every lit world
fragment a promoted light covers runs a 25-tap comparison kernel whose result is
then differenced against the bake. Scope both to where a dynamic occluder can
actually cast: skip the copy when no entity depth is in play, and skip the kernel
for fragments outside the occluder's shadow volume. Saves up to 44 MiB/frame of
depth-to-depth copy plus the per-fragment kernel, and removes the rake-angle
striping as a consequence rather than as a tuning exercise.

## Scope

### In scope

- Per-slot entity-occluder latch on `LayerState`
  (`crates/renderer/src/render/promoted_depth_cache.rs`) plus the derived
  `entity_drawn_last_frame` on `PromotedSpotCachePlan` / `PromotedCubeCachePlan`,
  making `copy_spot_to_pool` / `copy_cube_face_to_pool`
  (`crates/renderer/src/render/renderer_shadow_passes.rs`) conditional.
- Per-promoted-light world-space occluder bound — a conservative sphere over the
  mesh-instance and mover AABBs that intersect that light's influence — computed
  in the existing eligibility loop in `Renderer::update_dynamic_light_slots`
  (`crates/renderer/src/render/renderer_light_slots.rs`) and stored
  selection-indexed on `FullRenderer`.
- Forward-shadowmask metadata record grows from 2 to 3 `vec4<f32>`:
  `FORWARD_SHADOWMASK_META_VEC4S_PER_RECORD`
  (`crates/renderer/src/render/shadowmask.rs`) and the WGSL
  `SHADOWMASK_META_VEC4S_PER_RECORD` (`crates/renderer/src/shaders/forward.wgsl`),
  with `meta2 = (occluder_center.xyz, occluder_radius)`.
- Shadow-volume rejection in `shadowmask_union_subtraction` (`forward.wgsl`),
  placed before the kernel so a rejected fragment costs no texture fetch.
- A test pinning the Rust and WGSL per-record strides against each other. They
  agree today and nothing enforces it.
- Dev-tools diagnostics surface for the promoted-cache counters, which have three
  `pub` accessors on `Renderer` (`crates/renderer/src/render/renderer_state.rs`)
  and zero call sites.
- Amend `context/lib/rendering_pipeline.md` §8, which currently asserts the copy
  "replaces the per-frame clear and never leaves stale depth."

### Out of scope

- **Rake-angle depth bias.** `WORLD_RECEIVER_BIAS_SCALE` and the world depth
  pipeline's `DepthBiasState` are untouched. Entity scoping removes the striping
  outside occluder shadow volumes; residual acne inside them is a separate change,
  and its area is not knowable until this lands.
- **The static-only reference comparison** (sampling the promoted depth cache and
  differencing `static_pool_vis - combined_pool_vis`). Argued in `research.md`;
  it doubles taps in the region this plan is making cheaper.
- **Penumbra-model mismatch between the bake and the runtime compare.** Measured
  in `research.md`. Real, secondary, and bounded by `baked_vis` under linear
  falloff.
- **Right-sizing `SHADOW_POOL_SIZE`.** 96 slots × 1024² `Depth32Float` is 384 MiB
  against a ceiling near 19 occupied slots on `campaign-test`. Allocation is not
  per-frame traffic; this plan targets traffic.
- **`shadowmask_direct`'s hard-coded linear falloff** (`forward.wgsl`) against the
  compiler's three `FalloffModel` variants. Every light entity in `content/`
  authors `"delay" "0"` (Linear), so no shipped content reaches it.
- **Which lights promote.** `entity_shadow_select`, the ranker, `EVICTION_MARGIN`,
  and the promotion budgets are unchanged.
- **The shadow cone cull.** Owned by `shadow-cone-cull-parallel-dispatch`, which
  already declares the promoted depth cache out of its scope.

## Direction

**Problem.** World receivers re-derive static occlusion at runtime that the
lightmap bake already encodes, in two places: the pool re-copies cached static
depth every frame regardless of whether entity depth will be layered on top, and
the forward pass runs a comparison kernel for every lit fragment a promoted light
covers, then subtracts the bake-minus-runtime difference. The runtime signal is
the noisier of the two — at rake angles the depth compare produces acne that the
one-tap dead zone cannot absorb — so the redundant evaluation does not merely cost
bandwidth, it injects error the bake did not have.

**Observation.** A mode-5 capture on `combat-demo` shows regular striping at the
shadow-texel pitch across open floor under a promoted light, far from any dynamic
occluder; the striping follows the surfaces each light rakes across maps. See
`research.md`.

**Placement.** The copy condition belongs in the promoted depth cache's frame
plan, which already owns the warm/cold decision and already keys on slot identity.
The union gate has to be per-fragment in the shader with a CPU-supplied bound:
dropping the promoted record is not available, because movers and skinned meshes
consume the same record through their own crossfade — only the world-receiver
consumption is redundant. The CPU computes one bound per light per frame; the
shader tests it per fragment.

**Prior commitments.**

- `rendering_pipeline.md` §4 states the world-surface path dead-zones the union
  subtraction so that "sub-threshold compare noise contributes zero, keeping
  runtime static→static shadowing exactly zero (the double-count invariant)." The
  capture is evidence that guarantee does not hold at rake angles. This plan does
  not diverge from the invariant; it supplies a mechanism that makes it hold.
- `rendering_pipeline.md` §8 states the promoted copy "is the occupied-face
  initialization baseline for promoted slots; it replaces the per-frame clear and
  never leaves stale depth." Task 3 makes the copy conditional, so that sentence
  must be amended to state the condition. This is a real divergence from a
  documented invariant, deliberately taken: the copy remains the initialization
  baseline, but it is issued only when something can have changed the layer.
- `rendering_pipeline.md` §4 states promotion reaches world receivers "only as the
  shadowmask union subtraction." This plan narrows *where* that reaches, not the
  mechanism.
- `shadowmask-no-drop-atlas` (draft) re-purposes the promoted-record `meta1.z`
  channel field to carry a `(block, channel)` slot. It does not change the record
  stride. Both plans edit `pack_forward_shadowmask_metadata` and the WGSL metadata
  decode; whichever lands second rebases on the other. No conflict in the fields
  each touches — this plan appends `meta2` and leaves `meta0`, `meta1.z` and the
  unread `meta1.w` alone.

**Alternatives rejected.** The strongest rival is the static-only reference: bind
the promoted depth cache and compute `static_pool_vis - combined_pool_vis` instead
of `baked_vis - pool_vis`. It is zero by construction with no occluder present and
it cancels acne, because both compares then share a kernel and a bias — strictly
more correct than a bound. Rejected as the first move because it doubles taps in
exactly the region this plan exists to make cheaper, needs `TEXTURE_BINDING` on a
cache declared `RENDER_ATTACHMENT | COPY_SRC`, needs an answer for adapters
without `CUBE_ARRAY_TEXTURES`, and does nothing for the copy. Entity scoping makes
it strictly cheaper if it is still wanted afterward, since it would then run only
inside occluder shadow volumes. Full reasoning and the two other rejected shapes
are in `research.md`.

**Foreclosures and reversibility.** The metadata stride is the least reversible
piece; it is a per-frame GPU buffer with no serialized form, so undoing it is the
constant plus the packer plus the decode. The latch establishes that a promoted
pool layer's contents are a function of the latch, so any future writer into a
promoted pool layer must participate in it — that constraint is stated in the
Invariants table rather than left implicit. Neither change forecloses the
static-only reference.

## Acceptance criteria

- [ ] Rendered output is unchanged when the metadata record grows and every light
      carries the permissive bound, on `combat-demo` and `stress-warren-lit`.
- [ ] With a promoted light holding a slot whose entity gate does not pass, no
      depth-to-depth copy is issued for that slot once its cache layer is warm and
      its latch has cleared.
- [ ] On the first frame after a slot's entity gate goes from passing to not
      passing, exactly one copy is issued for that slot, and the pool layer shows
      no residual occluder depth from the previous frame.
- [ ] A slot whose cache layer is freshly claimed, or whose occupant changed,
      issues a copy regardless of the latch.
- [ ] A cube slot marked warm only after all six faces render still issues per-face
      copies under the same condition as a spot slot.
- [ ] A world fragment outside every promoted light's occluder shadow volume
      produces exactly zero union subtraction, and mode 5 renders it black.
- [ ] An entity standing between a promoted light and a wall still casts its shadow
      on that wall, with the same extent as before this change, including when the
      entity is at the edge of the light's influence.
- [ ] Two occluders far apart under one light both keep their shadows; neither is
      clipped by the shared bound.
- [ ] A promoted light in its demote sticky window with no intersecting occluder
      behaves exactly as it does today — no visible change in the subtraction as
      the entity leaves.
- [ ] The rake-angle striping is absent from world surfaces outside occluder shadow
      volumes on `combat-demo` and `stress-warren-lit`.
- [ ] A test fails if the Rust per-record stride constant and the WGSL one diverge.
- [ ] The promoted-cache counters — records promoted, world renders skipped, copies
      issued, copies skipped — are readable in the dev-tools diagnostics UI.

## Tasks

### Task 1: Grow the shadowmask metadata record to three vec4s

Thin slice through the CPU→GPU metadata seam, output-identical to today.
`pack_forward_shadowmask_metadata` in
`crates/renderer/src/render/shadowmask.rs` writes 8 native-endian `f32` per
record today (`meta0 = (global_light_index, selection_index, spec_index, weight)`,
`meta1 = (pool_kind, slot, channel, 0.0)`). Raise
`FORWARD_SHADOWMASK_META_VEC4S_PER_RECORD` from 2 to 3 and append
`meta2 = (0.0, 0.0, 0.0, -1.0)` — the permissive sentinel, meaning "no bound
known, test nothing." `FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD` and
`influence_capacity_with_shadowmask_metadata` both already derive from that
constant, so buffer sizing follows without edits. In
`crates/renderer/src/shaders/forward.wgsl`, raise
`SHADOWMASK_META_VEC4S_PER_RECORD` to `3u`, widen the tail guard in
`shadowmask_union_subtraction` from `if meta_index + 1u >= influence_len` to
`+ 2u`, read `let meta2 = light_influence[meta_index + 2u];`, and apply the
shadow-volume test described in the Rough sketch — which no-ops under the
sentinel, so rendered output is unchanged. Retarget
`forward_shader_shadowmask_union_uses_promoted_count_and_safe_metadata_tail` in
`crates/renderer/src/render/tests/shader_tests.rs`, whose literal assertion on
`"if meta_index + 1u >= influence_len"` is the one hard break; leave `weight` on
`meta0.w`, which the same test pins, and add no new `sample_shadowmask_atlas(`
call site, whose count is pinned at 3 by
`forward_shader_shadowmask_fallback_clamps_multilayer_indices`. Add the
stride-agreement test required by the Invariants table: derive the expected WGSL
literal from the Rust constant and assert the shader source declares it, so the
two cannot drift. Do not add a field to `PromotedStaticLightRecord`; the bound
arrives in Task 2 as a separate selection-indexed slice argument, matching how
`selection_spec_light_indices` and `channels` are already passed.

### Task 2: Compute and pack the per-light occluder bound

`Renderer::update_dynamic_light_slots` in
`crates/renderer/src/render/renderer_light_slots.rs` already runs a per-candidate
eligibility loop that iterates the planned mesh instances and the mover AABBs and
tests each against the light's influence via
`static_light_influence_intersects_aabb`, discarding everything but two booleans.
Extend that loop to also accumulate, per candidate, the union of the world-space
AABBs that intersected — `instance.bounds.transformed(&instance.transform)` for
mesh instances and `mover.world_aabb` for movers — then convert the union to a
bounding sphere and store it into a new `Vec<Option<OccluderBound>>` field on
`FullRenderer`, indexed by **selection** index (read
`full.shadow_candidate_selection_indices[candidate_index]` inside the loop to
convert), raw-length N and index-parallel to `entity_shadow_light_influences`, so
no candidate→selection bridge is needed at pack time. Resize and clear it wherever
`promoted_static_weights` is resized. Pass it to
`pack_forward_shadowmask_metadata` as a new slice parameter and write
`meta2 = (center.xyz, radius)` when the entry is `Some`, keeping the `-1.0`
permissive sentinel from Task 1 when it is `None`. `None` is the correct value
during a light's demote sticky window, when the record still exists but no
occluder intersects: the permissive sentinel preserves today's behavior for that
window rather than snapping the subtraction to zero and popping. The bound must be
conservative — a sphere enclosing the union of AABBs is never smaller than the
real occluder set, and a single sphere over several spread-out occluders degrades
toward the permissive case rather than clipping either shadow.

### Task 3: Make the promoted pool copy conditional

`copy_spot_to_pool` and `copy_cube_face_to_pool` in
`crates/renderer/src/render/renderer_shadow_passes.rs` issue a
`copy_texture_to_texture` for every occupied promoted slot (and every face of a
promoted cube slot) on every frame, outside the `plan.needs_world_render` branch
and before the entity-eligibility check. Add an `entity_drawn: bool` to
`LayerState` in `crates/renderer/src/render/promoted_depth_cache.rs` and surface
last frame's value as `entity_drawn_last_frame` on `PromotedSpotCachePlan` and
`PromotedCubeCachePlan`, read in `plan_frame_with_layers` alongside `warm`. In the
shadow passes, evaluate the existing entity gate —
`slot_entity_eligible[slot] && (mesh_frame_plan.is_some() || !mover_occluder_aabbs.is_empty())`
— into a local before the copy, and issue the copy only when
`plan.needs_world_render || gate_passes || plan.entity_drawn_last_frame`. After
the entity block, record this frame's gate result back onto the layer through a
new `set_spot_entity_drawn` / `set_cube_entity_drawn` method. Latch on the *gate*,
not on submitted draw counts: `record_skinned_depth` and `record_kinematic_movers`
can each submit zero draws after the gate passes, and latching on the gate
over-copies in that case rather than leaving stale depth. `retain_active_layers`
and `assign_layer` must clear `entity_drawn` wherever they clear `warm`, so a
tenancy change cannot inherit another occupant's latch; `reset_level` clears it
via `LayerState::default()`. Update the four tests in that file's `mod tests` that
construct `LayerState` or poke `warm` directly. Amend
`context/lib/rendering_pipeline.md` §8, which asserts the copy "replaces the
per-frame clear and never leaves stale depth," to state the condition under which
it is issued and why that still admits no stale depth.

### Task 4: Surface the promoted-cache counters in dev-tools

`Renderer` exposes `promoted_depth_cache_promoted_count`,
`promoted_depth_cache_world_render_skips` and
`promoted_depth_cache_cull_dispatch_skips` in
`crates/renderer/src/render/renderer_state.rs`; all three are `pub`, all three are
annotated `allow(dead_code)` outside `dev-tools`, and none has a call site
anywhere in the repo. Add two counters alongside them for copies issued and copies
skipped this frame, written where Task 3 evaluates the copy condition and reset
where the other per-frame promoted counters are reset in
`renderer_render_frame.rs`. Wire all five into the dev-tools diagnostics UI under
the existing Lighting panel that already hosts the `SdfShadowMode` selector, so
the acceptance criteria on copy counts are readable without a profiler. Note that
`promoted_count` counts records rather than successfully assigned cache layers —
`assign_layer` returns `None` when more records of a kind exist than the budget —
so label it as records, not layers, and do not present it as the denominator for
copies.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the stride, buffer-sizing
and shader-test assumptions before any bound math depends on them.
**Phase 2 (concurrent):** Task 2, Task 3 — disjoint files; Task 2 owns
`renderer_light_slots.rs` and `shadowmask.rs`, Task 3 owns
`promoted_depth_cache.rs` and `renderer_shadow_passes.rs`.
**Phase 3 (sequential):** Task 4 — consumes Task 3's copy-condition site for its
two new counters.

## Rough sketch

The shader-side rejection, placed immediately after `let sl = spec_lights[spec_idx];`
in `shadowmask_union_subtraction` so a rejected fragment costs neither
`shadowmask_direct` nor the kernel:

```wgsl
// Proposed design
let occ_radius = meta2.w;
let light_pos = sl.position_and_range.xyz;
let to_frag = world_pos - light_pos;
let frag_dist = length(to_frag);
if occ_radius >= 0.0 && frag_dist > SHADOWMASK_EPS {
    let dir = to_frag / frag_dist;
    let oc = meta2.xyz - light_pos;
    let along = dot(oc, dir);
    // Occluder behind the light, or fragment nearer than the occluder.
    if along <= 0.0 || frag_dist < along - occ_radius { continue; }
    // The silhouette widens with distance from a point emitter.
    let reach = occ_radius * frag_dist / along;
    if dot(oc, oc) - along * along > reach * reach { continue; }
}
```

`SHADOWMASK_EPS` already exists in `forward.wgsl`. The test is a diverging cone,
not a cylinder: scaling the silhouette radius by `frag_dist / along` is what keeps
it conservative at range.

## Orderings

| Scenario | Ordering | Expected |
|---|---|---|
| Entity leaves a promoted light's influence | Gate passes on frame N, fails on N+1, N+2… | Copy issued on N and N+1 (latch), skipped from N+2. No residual occluder depth. |
| Entity gate passes, zero draws submitted | `record_skinned_depth` and `record_kinematic_movers` both early-return | Latch set from the gate, so a copy is issued next frame regardless. Over-copies; never stale. |
| Slot reassigned to a different light | `CacheKey` changes, `assign_layer` claims a fresh layer | `warm` and `entity_drawn` both false; `needs_world_render` forces the copy. |
| Promoted → demoted → dynamic tenant → re-promoted | Dynamic pass `Clear(1.0)`s the layer between promotions | `retain_active_layers` freed the `LayerState` on the record-absent frame, so the re-promotion gets a fresh layer and a forced copy. |
| Cube slot with five of six faces rendered | Partial fill within one frame | Never marked warm (`face + 1 == CUBE_FACES` gate); next frame re-renders. |
| Demote sticky window, no occluder intersects | Record exists, `weight > 0`, bound is `None` | Permissive sentinel; subtraction identical to today for up to `STICKY_SECONDS`. |
| Two occluders spread across one light's influence | Union AABB spans both | One enclosing sphere; both shadows preserved, gate degrades toward permissive. |
| Occluder positioned behind the light | `along <= 0.0` | Rejected; no shadow can reach any fragment. |
| Fragment exactly on the silhouette tangent | `dot(oc,oc) - along² == reach²` | Passes (strict `>` rejects), so the boundary case keeps its shadow. |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A promoted pool layer never carries entity depth from a previous frame | Task 3 | The frame after the gate goes false; any tenancy change; any future writer into a promoted pool layer must participate in the latch | AC 2, 3, 4, 5 |
| The occluder bound is conservative — it never clips a real entity shadow | Task 2 | Multi-occluder union; occluder behind the light; tangent case | AC 7, 8 |
| World-receiver subtraction is zero wherever no dynamic occluder can cast | Task 1 (test), Task 2 (bound) | Demote sticky window packs the permissive sentinel by design | AC 6, 9, 10 |
| Rust and WGSL agree on the per-record metadata stride | Task 1 | Nothing enforces it today; `shadowmask-no-drop-atlas` also edits both sides | AC 1, 11 |

## Open questions

- The bound is a sphere over a union of AABBs, which is loose for a tall thin
  occluder near a wide light. Whether that looseness matters is a measurement to
  take after Task 2, not a reason to build a tighter bound first.
- Residual acne inside occluder shadow volumes is expected to remain. Its area,
  and therefore whether the bias change is worth its own spec, is not knowable
  until this lands.
