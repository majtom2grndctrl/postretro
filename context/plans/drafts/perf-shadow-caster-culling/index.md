# Shadow Caster Culling

> **Status:** draft. Per-frame crawl on large lit maps — the dynamic shadow pool saturates and
> re-rasterizes the world (plus a whole-BVH cone cull) for ~100 slots every frame.
> **Related:** `context/lib/rendering_pipeline.md` §4 (shadow pool), §7.1 (pass ordering) ·
> `context/plans/done/perf-dynamic-light-pvs-cull/` (the origin-cell gate this tightens) ·
> `context/plans/ready/static-light-entity-shadows/` Task 6 (the depth-cache mechanism fix (a) reuses) ·
> `context/plans/drafts/perf-anti-penumbra-pvs/` (shrinks the drawable PVS this consumes — complementary,
> not folded in).

## Problem

Plain unlit `stress-warren.prl` runs at v-sync. Its lit variants crawl: `stress-warren-lit` (~157
lights) and `stress-warren-crates` (36 spot casters). VRAM and camera-side culling are fine — the
drain is entirely in the dynamic shadow pool.

**Saturation mechanism.** Baked lights never enter the pool; only dynamic-tier lights compete for
slots. For every occupied slot, every frame, the renderer does two expensive things:

1. **Re-rasterizes world depth** — one `begin_render_pass` per spot slot
   (`renderer_shadow_passes.rs:220`) and one per cube face (`:329`), each drawing the slot's culled
   world geometry.
2. **Runs a whole-BVH cone cull** — one `dispatch_workgroups(1, 1, 1)` per occupied slot
   (`shadow_cull.rs:286–289`), each a single workgroup walking the entire BVH serially.

In an open, densely-portaled warren, dozens of dynamic casters all legitimately reach the visible
set, so the pool fills to its caps — `SHADOW_POOL_SIZE = 96` spot slots (`spot_shadow.rs:13`) plus
`CUBE_COUNT = 6` cube slots × 6 faces = 36 cube faces (`cube_shadow.rs:36`). The result is ~100+
render passes and ~100+ serial single-workgroup whole-BVH walks per frame. That is the crawl.

**Root cause — the eligibility gate is too loose.** `update_dynamic_light_slots`
(`renderer_light_slots.rs:25`) gates each candidate through
`shadow_candidate_reaches_visible_cell` (`renderer_lighting.rs:62`), which tests the light's runtime
influence sphere against `reachable_cell_aabbs` — the **wide portal-reachable set**
(`renderer_light_slots.rs:11–18` documents this: same source as `light_reachable_cell_mask`, and it
deliberately includes empty `face_count == 0` cells). It is NOT the narrow drawable `VisibleCells`
PVS. In an open warren that reachable set is huge, so almost every caster clears the gate and enters
the pool.

The gate was widened deliberately (`perf-dynamic-light-pvs-cull` shipped the narrower origin-cell
gate; a later fix widened it back to the influence-vs-reachable-cell test because the own-cell-PVS
gate dropped a light whose cell left the shrinking PVS on pitch-down, making entity shadows vanish).
So the fix is not to revert — it is to gate against a set that is tight enough to bound the slot
count but still wide enough to keep any caster that lights a **visible receiver**.

**Ranking gives no slot stability.** `rank_lights` (`spot_shadow.rs:399`) scores each candidate by
`(falloff_range / max(distance, near_clip))²` (`:436`) and `assign_ranked_slots` (`:454`) re-sorts
by camera distance every frame. Slot identity is unstable frame-to-frame; only candidates past the
96/6 caps are dropped. The cube pool ranks through the same core (`rank_point_lights`,
`cube_shadow.rs:138`).

**The cost is invisible to profiling.** Both shadow passes set `timestamp_writes: None`
(`shadow_cull.rs:283`; spot depth `renderer_shadow_passes.rs:231`; cube depth `:340`), so
`POSTRETRO_GPU_TIMING` shows nothing for the dominant cost. Any before/after measurement is blind
until that is fixed.

## Goal

Return `stress-warren-lit` and `stress-warren-crates` to the v-sync baseline of plain
`stress-warren` by bounding the number of occupied shadow slots — which multiplies BOTH the
render-pass count and the cone-cull dispatch count — without dropping any caster that shadows a
visible receiver. Make the shadow passes measurable first so the win is provable, parallelize the
surviving cone culls, and (optionally, last) skip world re-raster for static-in-place casters.

## Scope

### In scope

Four fixes, sequenced `(d) → (b) → (c) → (a)`:

- **(d) Shadow timestamp brackets (size S, measurement prerequisite).** Make the shadow cull and
  shadow depth passes visible to `POSTRETRO_GPU_TIMING`. Extend the `TIMING_PAIR_*` registry
  (`pipeline_layout.rs:131–138`, `TIMING_PAIR_COUNT` currently 7), add labels
  (`renderer_init_resources.rs:540–548`), and wire `timestamp_writes` at the three shadow sites via
  `frame_timing.rs` `compute_pass_writes` / `render_pass_writes` (`:136–155`).
- **(b) Tighter caster gate + screen-space ranking (size M–L, THE crawl fix).** Gate
  shadow-caster eligibility against the drawable PVS + one portal hop instead of the wide
  fog-reachable set, and rank by screen-space influence instead of raw camera distance. Cuts the
  occupied-slot count. Touches the `visible_lights` construction and
  `shadow_candidate_reaches_visible_cell` (`renderer_light_slots.rs:61–92`,
  `renderer_lighting.rs:62`), the caller that supplies `reachable_cell_aabbs`, and the scoring in
  `rank_lights` / `rank_point_lights`.
- **(c) Parallelize the per-slot cone culls (size M, independent win).** Replace the
  `dispatch_workgroups(1, 1, 1)`-per-slot loop in `shadow_cull.rs::dispatch_occupied_slots`
  (`:286–289`) with one dispatch over slot × leaf pairs, turning ~100 serial single-workgroup
  whole-BVH walks into one parallel pass. Helps whatever slots survive (b).
- **(a) Dynamic depth caching (size L, optional, last).** Cache a non-moving caster's world depth so
  a static-in-place dynamic light skips its per-frame world re-raster. Reuses the depth-cache
  mechanism designed in `static-light-entity-shadows` Task 6.

### Out of scope

- **Streaming / residency.** No change to what geometry is resident.
- **SDF / depth-moment device-limit guards.** `stress-warren-maze-crates` panics on load today for
  lack of these guards; adding them is a separate plan. See the interlock in Acceptance criteria.
- **Bake time.** No compiler-side work here.
- **The `static-light-entity-shadows` feature itself.** Fix (a) *reuses its Task 6 depth-cache
  mechanism* but does not implement or depend on the static-light promotion feature. That feature's
  cache covers only ≤10 static promoted lights (`MAX_PROMOTED_SPOT = 8` + `MAX_PROMOTED_CUBE = 2`)
  and explicitly leaves dynamic lights on the per-frame path; fix (a) extends the same mechanism to
  dynamic slots.
- **Compile-time PVS tightening.** `perf-anti-penumbra-pvs` shrinks the drawable PVS that fix (b)
  consumes as input; it is complementary and independent. Do not fold it in.

## Acceptance criteria

- [ ] `cargo build -p postretro` and the renderer crate compile clean with no warnings.
- [ ] `cargo test -p postretro-renderer` (or the crate holding the ranking/gate tests) passes,
  including the existing `shadow_candidate_reaches_visible_cell` tests
  (`render/tests/light_filter_tests.rs`) updated for the tightened gate, and new tests pinning the
  screen-space ranking order.
- [ ] **(d)** With `POSTRETRO_GPU_TIMING=1`, the per-frame timing log gains `shadow_cull` and
  `shadow_depth` lines. On `stress-warren-lit` / `stress-warren-crates` pre-(b), those lines
  dominate the frame; post-(b)+(c) they shrink sharply.
- [ ] **(b)** With `POSTRETRO_SHADOW_DEBUG=1` (`emit_shadow_debug`,
  `renderer_light_slots.rs:261`), the `spot=<used>/96 … cube=<used>/… spot_overflow=… cube_overflow=…`
  line shows `spot_used` / `cube_used` falling sharply and `spot_overflow` / `cube_overflow`
  collapsing toward 0 on both stress maps, versus the pre-fix saturated counts.
- [ ] **(b) correctness** A caster whose own cell is not drawable but whose influence reaches a
  visible receiver through a portal still receives a slot (manual: place/keep a spot behind a portal
  that lights a wall the camera sees; confirm its shadow persists on pitch-down — the regression the
  wide gate originally fixed does not return).
- [ ] **(c)** The single slot × leaf dispatch produces the same per-slot cull output as the old
  per-slot loop (culled-leaf sets identical for a fixed camera/pool; shadow visuals unchanged).
- [ ] **Net** `stress-warren-lit` and `stress-warren-crates` frame time returns to the v-sync
  baseline of plain `stress-warren`. Verified manually via
  `POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run <map>.prl`.
- [ ] **(a), if landed** A static-in-place dynamic caster's per-frame world-depth draw AND its
  per-frame cone-cull dispatch both drop to zero after its cache warms (skip counters), while a
  caster whose cone matrix changed re-renders. No shadow visual change.

**Interlock (not scoped here):** full-scale validation on `stress-warren-maze-crates` is blocked
until the SDF / depth-moment device-limit guards land — that map panics on load today. Iterate and
validate on `stress-warren-lit` and `stress-warren-crates`, which load today.

## Tasks

### Task 1 — (d) Shadow timestamp brackets

Make the shadow passes measurable; this is the prerequisite for proving every later fix.

Add two pairs to the `TIMING_PAIR_*` registry (`pipeline_layout.rs:131–138`): `TIMING_PAIR_SHADOW_CULL`
and `TIMING_PAIR_SHADOW_DEPTH`, bumping `TIMING_PAIR_COUNT` (7 → 9). Add matching labels
(`"shadow_cull"`, `"shadow_depth"`) in the `pass_labels` vec (`renderer_init_resources.rs:540–548`).

Wire `timestamp_writes` at the three sites, replacing `None`:

- **Shadow cull compute pass** (`shadow_cull.rs:283`) — one compute pass, trivially bracketable.
  Pass `compute_pass_writes(TIMING_PAIR_SHADOW_CULL)` (`frame_timing.rs:147`).
- **Spot depth** (`renderer_shadow_passes.rs:220`/`:231`) and **cube depth** (`:329`/`:340`).

**Wrinkle (call out as a task caveat).** The compute cull is a single pass and brackets cleanly. The
spot and cube depth passes are ~100 SEPARATE `begin_render_pass` calls — they cannot each take a
timestamp pair, because `frame_timing` caps at 64 pairs (`mark_pair_written` guards `pair_idx < 64`,
`frame_timing.rs:157`) and there are only two allocated for shadows. Do NOT try to bracket each
depth pass individually. Options for the aggregate depth number, pick one in implementation:
- an encoder-level timestamp straddling the whole depth-pass block (requires the
  `TIMESTAMP_QUERY_INSIDE_ENCODERS` wgpu feature — check adapter support and gate on it), or
- a coarse begin/end pair around the first and last depth pass in the block (approximate, but zero
  new features), or
- accept cull-only bracketing for v1 and land depth aggregation as a follow-up.

Bracket the cull cleanly regardless; the depth aggregate is the only judgment call.

### Task 2 — (b) Tighter caster gate + screen-space ranking

The crawl fix. Cutting the occupied-slot count multiplies down BOTH the render-pass count and the
cone-cull dispatch count.

**Tighter gate.** Today `update_dynamic_light_slots` (`renderer_light_slots.rs:25`) builds
`visible_lights` by testing each candidate's influence sphere against `reachable_cell_aabbs` — the
wide portal-reachable set including empty `face_count == 0` cells. Replace the input set with the
**drawable PVS + one portal hop**: the cells that actually feed draws (the `VisibleCells` set the
camera renders), expanded by a single portal step so a caster one wall behind a visible cell still
qualifies. Concretely, change the AABB set the caller passes as `reachable_cell_aabbs` (and rename
if it no longer means "fog-reachable"), and update `shadow_candidate_reaches_visible_cell`
(`renderer_lighting.rs:62`) and its tests (`render/tests/light_filter_tests.rs:212`, `:223`)
accordingly. The `ALPHA_LIGHT_LEAF_UNASSIGNED` cull and the empty-slice DrawAll sentinel stay.

This is a gate on the light's **influence reach**, not on the caster's own cell — a caster in a
non-drawable cell that lights a drawable receiver still passes (see the correctness risk). One
portal hop is the tuning knob: wide enough to catch through-portal receivers, narrow enough to
starve the pool of casters that reach only empty/off-screen cells.

**Screen-space ranking.** Replace the raw camera-distance score in `rank_lights` (`spot_shadow.rs:436`,
`(falloff_range / distance)²`) with a screen-space influence estimate — how much of the frame the
light's shadowed region can plausibly cover (e.g. projected influence-sphere solid angle, or
projected radius / distance clamped to the frustum). Mirror the change in the cube ranker
(`rank_point_lights`, `cube_shadow.rs:138`) since both flow through the shared `assign_ranked_slots`
core. This makes slot occupancy track on-screen contribution rather than proximity, so the capped
slots go to the casters that matter and overflow drops the rest.

Note the complementary draft `perf-anti-penumbra-pvs` shrinks the drawable PVS this task consumes as
input — reference only; do not depend on or fold in.

### Task 3 — (c) Parallelize the per-slot cone culls

Independent of (b); helps whatever slots survive it.

`dispatch_occupied_slots` (`shadow_cull.rs:264–289`) currently loops occupied slots and issues one
`dispatch_workgroups(1, 1, 1)` per slot (`:286–289`), each a single workgroup walking the whole BVH
serially. Replace with ONE dispatch parameterized over (slot, leaf) pairs so the BVH walk runs in
parallel across all occupied slots at once. Preserve the per-slot uniform/bind-group binding
contract: each slot's cone planes are written to its own uniform buffer before the pass
(`:261–275`), and each slot's culled leaves must land in the same indirect sub-region the matching
`draw_slot_indirect` reads (`:295+`, keyed on `region_stride_bytes`). The parallelization changes
only how the walk is dispatched, not the region layout or the draw side.

### Task 4 — (a) Dynamic depth caching (optional, last)

Smallest marginal return; do after (b)+(c) or defer entirely. Caching alone does NOT bound
saturation — an uncached-but-cached-depth pool still runs ~100 live passes for the copy + entity
draw; (b) is what bounds the pass count. This only removes the world *re-raster* cost from slots
that survive (b).

Cache a non-moving caster's world depth so a static-in-place dynamic light skips its per-frame world
rasterization. This needs two pieces that do NOT exist for dynamic lights today:

- **Stable light-keyed slot binding.** `rank_lights` re-sorts by score every frame, so a light's
  slot identity is unstable — a cache keyed on slot index would thrash. Introduce a light-keyed slot
  binding so a light holds its cache across frames.
- **Cone-matrix dirty test.** Detect when a slot's cone/face matrix is unchanged since the cached
  depth was rendered; only then skip the world draw.

**Reuse the `static-light-entity-shadows` Task 6 mechanism** (reference, do not re-derive): a
dedicated `Depth32Float` cache array sized to the budget, pool + cache textures carrying
`COPY_SRC` / `COPY_DST` usage, a depth-to-depth `copy_texture_to_texture` into the live slot each
frame, the copy-replaces-clear invariant (`cube_face_needs_clear` — the copy stands in for the
Clear, never leaves stale depth), and skip counters. That feature's cache is sized for ≤10 static
promoted lights (`MAX_PROMOTED_SPOT = 8` + `MAX_PROMOTED_CUBE = 2`) and explicitly leaves dynamic
lights on the per-frame path; this task extends the same array/copy machinery to dynamic slots that
pass the dirty test. Cache VRAM stays O(budget), not O(pool size). Entity occluders still re-render
per frame on top of the copied world depth via `LoadOp::Load`.

## Sequencing

`(d) → (b) → (c) → (a)`.

- **(d) first.** Measurement prerequisite — without shadow-pass timestamps, no later fix is provable.
- **(b) next.** The crawl fix; it bounds the slot count and therefore both the pass count and the
  cull dispatch count. Everything downstream operates on the reduced pool.
- **(c) is independent of (b).** It can land in parallel with or before (b); it parallelizes whatever
  slots exist. Sequenced after (b) only so its win is measured against the already-reduced pool.
- **(a) is optional and last.** Smallest marginal return once (b) has bounded the pool; skippable for
  v1. It depends on the stable-slot-binding + dirty-test infrastructure it introduces, and reuses the
  Task 6 depth cache.

## Decisions

- **Gate-tightening is the primary fix, not caching.** The crawl is a *count* problem: ~100 occupied
  slots × (one render pass + one whole-BVH cull dispatch) each. Caching depth (fix a) removes only
  the world-raster half of each surviving slot's cost and still leaves ~100 live passes doing the
  copy + entity draw + (uncached) cull. Only tightening the gate (fix b) reduces the slot count
  itself, which is the single multiplier on every per-slot cost. So (b) is sized M–L and load-bearing;
  (a) is L but optional and marginal.
- **Timestamps first.** The dominant cost is currently invisible to `POSTRETRO_GPU_TIMING`
  (`timestamp_writes: None` at all three shadow sites). Landing (d) first — a size-S change — makes
  the crawl measurable so (b) and (c) can be proven rather than asserted, and so regressions are
  caught by the same tool.
- **Reach gate, not own-cell gate.** The gate stays a test on the light's influence reaching a
  drawable-relevant cell, not on the caster's own cell being drawable. The narrower own-cell gate was
  already tried (`perf-dynamic-light-pvs-cull`) and reverted because it dropped through-portal
  casters on pitch-down. Drawable-PVS-plus-one-hop is the middle ground.

## Risks

- **Tighter-gate correctness (highest).** The gate must NOT drop a caster whose light reaches a
  visible RECEIVER through a portal even when the caster's own cell is not drawable. This is exactly
  the regression the wide gate was widened to fix (entity shadows vanishing on pitch-down as the PVS
  shrank). The one-portal-hop expansion is the mitigation; if one hop proves too narrow on real maps,
  the hop count is the tuning knob. Pin with the pitch-down through-portal test in Acceptance.
- **Depth-timestamp aggregation wrinkle.** The ~100 separate depth `begin_render_pass` calls cannot
  each carry a timestamp pair (64-pair ceiling, two pairs allocated). The aggregate/encoder-level
  approach (Task 1) may need the `TIMESTAMP_QUERY_INSIDE_ENCODERS` feature; if the target adapter
  lacks it, the depth number is coarse (first-to-last pair) or deferred. The cull number is exact
  regardless.
- **Screen-space ranking heuristic tuning.** "Screen-space influence" is an estimate (projected
  solid angle / clamped projected radius). A bad estimate could starve a genuinely dominant caster or
  favor a large-but-occluded one. Keep the heuristic simple and tune against the two stress maps;
  ranking only decides *which* casters fill the capped slots, so a mis-rank degrades quality, not
  correctness.
- **(c) region-contract drift.** The single slot × leaf dispatch must write each slot's culled leaves
  into the same indirect sub-region `draw_slot_indirect` reads. A mismatch silently corrupts shadow
  draws. Pin with the per-slot cull-output equivalence check in Acceptance.

## Related work

- **`context/plans/done/perf-dynamic-light-pvs-cull/`** — shipped the origin-cell shadow gate that
  fix (b) tightens. Its own-cell-PVS gate was later widened to the influence-vs-reachable-cell test
  (the current loose gate); (b) re-narrows the *input set* without reverting to own-cell semantics.
- **`context/plans/ready/static-light-entity-shadows/` Task 6** — the depth-cache mechanism fix (a)
  reuses: dedicated `Depth32Float` cache array, `COPY_SRC`/`COPY_DST`, depth-to-depth copy,
  copy-replaces-clear invariant, skip counters. Covers only ≤10 static promoted lights today and
  leaves dynamic lights on the per-frame path; (a) extends it to dynamic slots.
- **`context/plans/drafts/perf-anti-penumbra-pvs/`** — shrinks the drawable PVS that fix (b) consumes
  as its input set. Complementary and independent; the two compound (a tighter PVS makes the
  drawable-plus-one-hop gate tighter still) but neither depends on the other.
