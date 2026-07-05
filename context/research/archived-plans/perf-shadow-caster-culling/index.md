> **⚠️ ARCHIVED 2026-07-05 — not an active plan. Do not implement from this.**
> The premise shifted after review: animated-light weight maps + the static-light shadowmask make *stationary* dynamic lights vestigial (author them static), so the primary depth-cache fix (a) targets a case good authoring eliminates. The idle-mover variant of that cache belongs to the kinematic movers epic (reuse `promoted_depth_cache.rs` + a mover at-rest signal). The parallel-BVH-walk technique (c) is measure-gated — resurrect only if a GPU capture shows a cull hot at production scale. Live disposition: `context/plans/roadmap.md` Epic 17 bullet E. Kept here for the research/reasoning only.

# Shadow Caster Culling

> **Status:** draft — REDESIGNED post-merge. The original premise (bound the *count* of
> occupied dynamic shadow slots by tightening the eligibility gate + screen-space ranking) was
> overtaken by the `static-light-shadowmask-world-receipt` / promoted-static-light merge, which landed
> the depth-cache and ranking infrastructure and — critically — locked in an invariant that **bars**
> the old gate-tightening fix. The remaining crawl driver is per-slot *cost* on the up-to-96+36
> DYNAMIC slots, which the merge left entirely on the uncached, serial per-slot path.
> **Related:** `context/lib/rendering_pipeline.md` §4 (dynamic direct, promoted static lights), §7.1
> steps 6–8 (shadow cone cull + depth passes) · `context/plans/done/perf-dynamic-light-pvs-cull/` (the
> origin-cell gate history) · `context/plans/ready/static-light-shadowmask-world-receipt/` (the merge
> that landed `promoted_depth_cache.rs` + `shadow_ranking.rs`) · `context/plans/drafts/perf-anti-penumbra-pvs/`
> (shrinks the drawable PVS — complementary, not folded in).

## Post-merge baseline

The shadow subsystem was reworked. Establish what already exists before adding scope.

**Promoted-static depth cache — the fix-(a) mechanism now exists in tree.**
`crates/renderer/src/render/promoted_depth_cache.rs` (`PromotedDepthCache`) is exactly the
"`Depth32Float` cache array + `COPY_SRC` + depth-to-depth copy + warm/skip counters" the old fix (a)
proposed reusing. Per frame `plan_frame` (`:197`) assigns each promoted-static record a stable cache
layer keyed on `(global_light_index, selection_index, slot)` (`CacheKey`, `:8`), tracks a `warm`
flag per layer, and emits `needs_world_render` (`:36`). Warm slots skip the world raster; their
depth is initialized in the live pool by `copy_spot_to_pool` / `copy_cube_face_to_pool` (`:225`,
`:260`). It also emits a **cull-side skip** (`should_dispatch_spot_cull` `:83`,
`should_dispatch_cube_cull` `:90`) so a warm slot contributes zero cone-cull work. **But the cache is
sized and keyed for PROMOTED STATIC lights only** — budget `MAX_PROMOTED_SPOT = 8` +
`MAX_PROMOTED_CUBE = 2` (`renderer_types.rs:371–372`), records are `PromotedStaticLightRecord`. It has
**no dirty test** (static lights never move) and does **not** cover dynamic-tier slots at all.

**Slot ranking + hysteresis — moved to a wgpu-free crate, position-only by design.**
Ranking left the renderer's `spot_shadow.rs` / `cube_shadow.rs` (the old `rank_lights` /
`assign_ranked_slots` anchors are gone) and now lives in `crates/lighting/src/shadow_ranking.rs`.
The score is unchanged: `slot_score = (falloff_range / max(distance, near_clip))²` (`:10`), a
**position-only** metric — `rank_spot_lights` / `rank_point_lights` (`:51`, `:82`) take
`camera_position` + `camera_near_clip` and **no** view-projection or frustum. The renderer drives them
through `assign_slots_with_hysteresis` (`:193`), which adds tier-neutral eviction hysteresis: a
dynamic light **keeps its slot frame-to-frame** unless a challenger out-scores its incumbent by
`EVICTION_MARGIN = 1.25` (`renderer_light_slots.rs:871`; incumbents built at `:949`). This gives
dynamic slots the **stable light-keyed binding** the old fix (a) said "does NOT exist for dynamic
lights today."

**Orientation-invariance is now a locked invariant.** Two tests pin that the assigned-slot SET must
not change with camera orientation (only eye position): the regression
`dynamic_spot_keeps_slot_when_cone_aabb_outside_pitched_camera_frustum` (`shadow_ranking.rs:817`) and
the proptest `shadow_slot_set_invariant_under_camera_orientation` (`:850`). This is the merge's
resolution of the pitch-down "entity shadow vanished" bug — and it directly **bars** the old fix (b)
(gate against the orientation-dependent drawable PVS, rank by frustum-position). See the disposition
table.

**Eligibility gate — unchanged (still wide).** `update_dynamic_light_slots`
(`renderer_light_slots.rs:30`) still builds `visible_lights` (`:79–127`) by testing each candidate's
runtime influence sphere against `reachable_cell_aabbs` — the WIDE portal-reachable set including
empty `face_count == 0` cells (`shadow_candidate_reaches_visible_cell`, `renderer_lighting.rs:73`,
documented `renderer_light_slots.rs:16–24`). The merge did NOT narrow it — deliberately, per the
invariant above.

**GPU timing — encoder-level brackets landed; the dynamic shadow passes are still dark.**
`frame_timing.rs` gained `write_encoder_start` / `write_encoder_end` (`:159–168`) for encoder-level
(non-pass) spans, and the registry gained `TIMING_PAIR_PROMOTED_DEPTH_CACHE = 7`
(`pipeline_layout.rs:138`, `TIMING_PAIR_COUNT = 9`), label `"promoted_depth_cache_upper"`
(`renderer_init_resources.rs:557`). That pair is a **coarse upper bound** that — by design — also
swallows the interleaved dynamic shadow-depth passes (`renderer_shadow_passes.rs:646–647`), **but only
opens when a promoted-static slot exists** (`:247`, `:474`). A pure-dynamic warren with zero promoted
lights (e.g. `stress-warren-lit`) gets **no** shadow-depth timing at all, and the shadow cone-cull
compute pass is still `timestamp_writes: None` (`shadow_cull.rs:288`) on every map.

## Post-merge disposition of the original four fixes

| Fix | Verdict | Deciding anchor |
| --- | --- | --- |
| **(d)** shadow timestamp brackets | **Partial → still needed** | Encoder-level mech + `promoted_depth_cache_upper` pair landed (`frame_timing.rs:159`, `pipeline_layout.rs:138`), but `shadow_cull.rs:288` is still `None` and the depth bracket only opens with a promoted slot (`renderer_shadow_passes.rs:247`). Dynamic-only maps stay dark. |
| **(b)** tighter gate + screen-space ranking | **Obsolete / contra-indicated → DROPPED** | The merge locked orientation-invariance of slot eligibility (`shadow_ranking.rs:817`, `:850`). Both halves of (b) — drawable-PVS gate and frustum-position ranking — are orientation-dependent and would reintroduce the exact pitch-down regression those tests guard. |
| **(c)** parallelize the per-slot cone culls | **Still needed → re-anchored** | `dispatch_occupied_slots_filtered` (`shadow_cull.rs:245`) still loops `dispatch_workgroups(1,1,1)` per occupied slot (`:291–294`); the `should_dispatch` closure only skips *warm promoted* slots. Shared-shader hazard still live (see Task 2). |
| **(a)** dynamic depth caching | **Infra delivered; dynamic extension still needed → now PRIMARY** | `promoted_depth_cache.rs` is the reusable mechanism, but covers only ≤8 spot + ≤2 cube *promoted-static* slots (`renderer_types.rs:371`). Dynamic slots re-raster world depth every frame (`renderer_shadow_passes.rs:344`, `:580`). Its missing prerequisites — stable slot binding (hysteresis) and the cache/cull-skip mechanism — now exist. |

## Problem

Plain unlit `stress-warren.prl` runs at v-sync. Its lit variants crawl: `stress-warren-lit`
(~157 lights, no promoted-static lights) and `stress-warren-crates` (36 spot casters over static
crates). VRAM and camera-side culling are fine — the drain is entirely in the **dynamic** shadow pool.

Dynamic-tier spot and point lights that clear the wide (unchanged) reachable gate saturate the pool to
its caps — `SHADOW_POOL_SIZE = 96` spot slots (`spot_shadow.rs:12`) plus `CUBE_COUNT = 6` cube slots ×
`CUBE_FACES = 6` = 36 cube faces (`cube_shadow.rs:36`, `:45`). For every occupied **dynamic** slot,
every frame, the renderer does two expensive things the merge left untouched:

1. **Re-rasterizes world depth** — one dedicated `begin_render_pass` per dynamic spot slot ("Spot
   Shadow Depth Pass", `renderer_shadow_passes.rs:345`) and one per dynamic cube face ("Cube Shadow
   Depth Pass", `:581`), each drawing the slot's cone-culled world geometry via `draw_slot_indirect`.
2. **Runs a whole-BVH cone cull** — one `dispatch_workgroups(1, 1, 1)` per occupied slot inside
   `dispatch_occupied_slots_filtered` (`shadow_cull.rs:291–294`), each a single workgroup walking the
   entire BVH serially. Two instances loop this: spot (`region_count = 96`) and cube
   (`region_count = 36`), called at `renderer_shadow_passes.rs:225` / `:446`.

Result on a saturated warren: ~100+ render passes and ~100+ serial single-workgroup whole-BVH walks
per frame. The merge's promoted-depth cache + cull-skip + hysteresis address only the ≤8 spot + ≤2
cube **promoted-static** slots; the up-to-96+36 **dynamic** slots remain fully on the uncached, serial
path. That is the crawl.

**Why the old lever (bound the count) is gone.** The original spec's primary fix — narrow the
eligibility gate to the drawable PVS and rank by screen-space footprint — is now contra-indicated:
the merge established that shadow-slot eligibility must be **orientation-independent** (the pitch-down
invariant, `shadow_ranking.rs:817` / `:850`). The drawable PVS shrinks on pitch-down; a frustum-position
ranking reorders on look direction. Either would resurrect the "entity shadow vanished" bug. The
orientation-independent count is already capped (96 spot + 6 cube via `assign_slots_with_hysteresis`
overflow), and lowering that cap is a quality knob, not this plan's concern. **So the only levers left
attack per-slot COST**, not count.

## Goal

Return `stress-warren-lit` and `stress-warren-crates` toward the v-sync baseline of plain
`stress-warren` by driving the per-frame cost of each occupied **dynamic** slot toward the
promoted-static cost profile — without narrowing the eligibility gate or reintroducing any
orientation dependence. Make the dynamic shadow passes measurable first (so the win is provable on a
promoted-light-free map), parallelize the surviving cone culls (a count-independent, VRAM-free win
that helps every occupied slot), then extend the existing promoted-depth cache to static-in-place
dynamic slots so the stationary majority skip their per-frame world re-raster and cull.

## Scope

### In scope

Three fixes, sequenced `(d) → (c) → (a)`:

- **(d) Dynamic shadow timestamp brackets (size S, measurement prerequisite).** Bracket the shadow
  cone-cull compute pass and the dynamic shadow-depth loop so `POSTRETRO_GPU_TIMING` shows them on
  every map — including promoted-light-free warrens where today's `promoted_depth_cache_upper` bracket
  never opens. Reuse the encoder-level `write_encoder_start` / `write_encoder_end`
  (`frame_timing.rs:159`).
- **(c) Parallelize the per-slot cone culls (size M, biggest count-independent win).** Replace the
  `dispatch_workgroups(1,1,1)`-per-slot loop with one dispatch over (slot, leaf) pairs. No VRAM cost,
  helps every occupied slot regardless of caching. In scope for BOTH the spot
  (`region_count = 96`) and cube (`region_count = 36`) instances. **Fork the shared shader — do not
  mutate `bvh_cull.wgsl` in place** (see Task 2).
- **(a) Extend the promoted-depth cache to static-in-place dynamic slots (size L, primary
  re-raster win).** Give dynamic slots a budgeted world-depth cache so a non-moving dynamic caster
  skips its per-frame world raster and cone cull, exactly as a warm promoted-static slot does today.
  Reuse `promoted_depth_cache.rs`'s array + copy + warm/skip machinery; add the one piece statics
  don't need — a **cone/position dirty test** so a *moving* dynamic light re-renders.

### Out of scope

- **Gate tightening / screen-space ranking (the old fix (b)).** Barred by the orientation-invariance
  invariant (`shadow_ranking.rs:817`, `:850`). Do NOT narrow `shadow_candidate_reaches_visible_cell`
  to the drawable PVS and do NOT add a frustum-position term to `slot_score`. If the pool cap itself
  is ever revisited, that is an orientation-independent quality knob in a separate plan, not here.
- **Lowering `SHADOW_POOL_SIZE` / `CUBE_COUNT`.** A quality trade, separate concern.
- **Moving-light world raster.** A genuinely moving dynamic light fails the dirty test and re-renders
  every frame; (c) still parallelizes its cull, but caching cannot help it. Not a gap to close here.
- **Streaming / residency, SDF / depth-moment device-limit guards, bake time, compile-time PVS
  tightening.** Unchanged from the original spec's non-goals. `stress-warren-maze-crates` still panics
  on load for lack of the SDF/moment guards — iterate on `stress-warren-lit` / `stress-warren-crates`,
  which load today.
- **The promoted-static feature itself.** (a) extends its cache machinery to dynamic slots; it does
  not modify promoted-static promotion, weights, or the SH-delta subtraction.

## Acceptance criteria

- [ ] `cargo build -p postretro` and `cargo build -p postretro-renderer` compile clean, no warnings.
- [ ] `cargo test -p postretro-renderer` and `cargo test -p postretro-lighting` pass, including the
  orientation-invariance guards (`shadow_ranking.rs:817`, `:850`) **unchanged** — this plan must not
  perturb them. The `promoted_depth_cache.rs` cache tests still pass and gain dynamic-tier analogues
  (a static-in-place dynamic slot warms and stops re-rendering; a moved dynamic slot re-renders).
- [ ] **(d)** With `POSTRETRO_GPU_TIMING=1` on `stress-warren-lit` (no promoted lights) the per-frame
  timing log gains a `shadow_cull` line and a `shadow_depth` line that are non-zero and dominate the
  frame pre-(c)/(a); both shrink sharply post-(c)+(a). The existing `promoted_depth_cache_upper` line
  is unchanged.
- [ ] **(c)** The parallel (slot, leaf) dispatch produces the same per-slot cull output as the old
  per-slot loop for both pools: for a fixed camera/pool, read back the indirect buffer's per-slot
  sub-regions (`region_stride_bytes`-spaced, `shadow_cull.rs:316`) before and after and diff the
  culled-leaf sets slot-by-slot. Shadow visuals unchanged. The camera cull path and its layout-pinning
  test (`candidate_shader_reuses_bvh_cull_struct_layouts`, `compute_cull.rs:1073`) are untouched.
- [ ] **(a)** A static-in-place dynamic caster's per-frame world-depth render pass drops to zero after
  its cache warms (skip counter), and its slot is excluded from the cone-cull work (via the same
  `should_dispatch_*` skip the promoted path uses), while a caster whose cone/position changed
  re-renders that frame. No shadow visual change. Cache VRAM stays O(dynamic budget), not O(pool size).
- [ ] **(b) diagnostics unchanged.** `POSTRETRO_SHADOW_DEBUG=1` (`emit_shadow_debug`,
  `renderer_light_slots.rs:392`) still logs the `spot=<used>/96 … cube=<used>/… spot_overflow=…
  cube_overflow=…` line (`:619–635`). This plan does **not** aim to collapse `spot_used`/`cube_used`
  (that was the dropped gate-tighten fix); the counts stay driven by the wide gate.
- [ ] **Net** `stress-warren-lit` and `stress-warren-crates` frame time returns toward the v-sync
  baseline of plain `stress-warren`, verified manually via
  `POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run <map>.prl`. Both maps' casters are static-in-place,
  so (a) removes their world raster and (c) their serial cull.

## Tasks

### Task 1 — (d) Dynamic shadow timestamp brackets

Make the dynamic shadow cull and depth passes measurable on **every** map, not just those with
promoted-static lights. Prerequisite for proving (c) and (a).

Add two pairs to the registry (`pipeline_layout.rs:131–140`): `TIMING_PAIR_SHADOW_CULL` and
`TIMING_PAIR_SHADOW_DEPTH`, bumping `TIMING_PAIR_COUNT` (9 → 11). Add matching labels
(`"shadow_cull"`, `"shadow_depth"`) to the `pass_labels` vec (`renderer_init_resources.rs:549–558`).
Both stay well under `frame_timing`'s 64-pair ceiling (`mark_pair_written`, `frame_timing.rs:171`).

- **Shadow cull** — one compute pass, trivially bracketable. Pass
  `compute_pass_writes(TIMING_PAIR_SHADOW_CULL)` (`frame_timing.rs:147`) into
  `dispatch_occupied_slots_filtered`, replacing `timestamp_writes: None` (`shadow_cull.rs:288`). Note
  both pools (spot at `renderer_shadow_passes.rs:225`, cube at `:446`) run this pipeline; bracket each
  under the same pair — the pass ran when either dispatched. (`render_pass_writes` /
  `compute_pass_writes` always write both `beginning`/`end` on one pass, so a single compute pass per
  call is clean; the two calls are separate passes — accept two same-pair spans across the frame,
  matching how `accumulate` sums a pair per frame, or bracket only the spot instance if cube is
  frequently empty. Implementor's call; the AC needs a non-zero `shadow_cull` line.)
- **Shadow depth** — the dynamic depth loops are ~100 separate `begin_render_pass` calls, so they
  cannot each carry a pair. Use the **encoder-level** bracket the merge already added
  (`write_encoder_start` / `write_encoder_end`, `frame_timing.rs:159–168`): open
  `TIMING_PAIR_SHADOW_DEPTH` at the top of `record_spot_shadow_depth` (`renderer_shadow_passes.rs:184`)
  and close it at the bottom of `record_cube_shadow_depth` (`:654`), spanning both loops
  **unconditionally** (do NOT gate on a promoted slot, unlike the existing
  `promoted_depth_cache_upper` open at `:247`/`:474`). This is a coarse aggregate that includes the
  promoted-cache work too; that overlap with `promoted_depth_cache_upper` is acceptable — `shadow_depth`
  is the whole-loop total, `promoted_depth_cache_upper` the promoted-only upper bound. Keep the
  existing `promoted_depth_cache_upper` pair as-is.

**Wrinkle.** `write_encoder_start`/`end` write raw encoder timestamps outside any pass; they need no
`TIMESTAMP_QUERY_INSIDE_ENCODERS` feature (that is only for timestamps *inside* a pass), so no adapter
gate is needed — the existing `promoted_depth_cache_upper` bracket already uses this path.

### Task 2 — (c) Parallelize the per-slot cone culls

The biggest count-independent win: turns ~130 serial single-workgroup whole-BVH walks into one
parallel pass, at zero VRAM cost, helping every occupied slot whether or not (a) caches it. In scope
for both instances built by `ShadowCullPipeline::new` (`shadow_cull.rs:73`): the spot instance
(`region_count = SHADOW_POOL_SIZE = 96`) and the cube instance (`region_count = CUBE_COUNT × CUBE_FACES
= 36`), both dispatched by `dispatch_occupied_slots_filtered` (`:245`).

`dispatch_occupied_slots_filtered` currently loops occupied slots and issues one
`dispatch_workgroups(1, 1, 1)` each (`:291–294`), every one a single workgroup walking the whole BVH.

**The cull shader is shared with the camera cull — fork, do not mutate.** `ShadowCullPipeline` builds
its module from `CULL_SHADER_SOURCE` = `bvh_cull.wgsl` (`shadow_cull.rs:9`, `:84`; the include lives
at `compute_cull.rs:36`) — the SAME source the camera `ComputeCullPipeline` uses, and
`shadow_cull.rs:88` notes the binding types must match `compute_cull.rs`. The layout-pinning test
`candidate_shader_reuses_bvh_cull_struct_layouts` (`compute_cull.rs:1073`) asserts its struct strides.
Changing binding-0's type or the dispatch model in `bvh_cull.wgsl` breaks the camera cull and trips
that test. **Fork a shadow-specific shader / entry point** (a cull variant module) for the
single-dispatch-over-(slot, leaf) model and its slot-indexed cone-plane buffer, and build
`ShadowCullPipeline` from the fork; `bvh_cull.wgsl` and the camera path stay untouched.

Replace with ONE dispatch parameterized over (slot, leaf) pairs so the BVH walk runs in parallel
across all occupied slots at once. A single dispatch cannot rebind per-slot uniforms, so the per-slot
cone planes (today one uniform buffer + bind group per slot, written at `shadow_cull.rs:263–279`) move
into ONE slot-indexed buffer indexed by a slot id derived from the (slot, leaf) pair. Drop the
per-slot uniform/bind-group entirely; do NOT preserve it. **Invariant that must hold:** each slot's
culled leaves land in the same indirect sub-region `draw_slot_indirect` reads
(`shadow_cull.rs:301–325`, keyed on `region_stride_bytes`) — the parallelization changes how cone
planes are bound and how the walk is dispatched, not the region layout or the draw side. The warm-slot
skip that (a) and the promoted path rely on (`should_dispatch_*`, threaded through the `should_dispatch`
closure at `:250`) must survive: a skipped slot contributes no (slot, leaf) pairs to the dispatch.

**Doc update, same change.** `context/lib/rendering_pipeline.md` §7.1 step 6 says "Shadow cone cull …
dispatches BVH traversal gated by that slot's cone frustum only" per-slot — correct it to describe the
unified (slot, leaf) dispatch.

### Task 3 — (a) Extend the promoted-depth cache to static-in-place dynamic slots

Now the primary re-raster win (with gate-tightening off the table, caching the stationary majority is
how the world-raster half of per-slot cost falls). Reuse `promoted_depth_cache.rs` rather than
building parallel machinery.

The mechanism already exists for promoted-static slots: a `Depth32Float` cache array sized to the
budget, `COPY_SRC` textures, a depth-to-depth `copy_*_to_pool` into the live slot, `warm` /
`needs_world_render` planning (`promoted_depth_cache.rs:36`, `:197`), and a cull-side skip
(`should_dispatch_spot_cull` `:83`). Two prerequisites the old fix (a) flagged as missing now exist:
**stable slot binding** (the merge's hysteresis keeps a dynamic light in its slot across frames unless
out-scored by `EVICTION_MARGIN`, `renderer_light_slots.rs:871`) and the cache/skip mechanism itself.

Extend the cache to dynamic slots:

- **Dynamic cache arrays, budgeted.** Add spot + cube dynamic-tier cache arrays alongside the existing
  promoted ones in `PromotedDepthCache` (or a sibling `DynamicDepthCache` reusing its helpers —
  implementor's call; prefer reuse of `assign_layer` `:314`, `retain_active_layers` `:304`, `copy_*`
  `:225`/`:260`). Size to a **small dynamic budget** `N`, NOT the 96+36 pool — cache VRAM stays
  O(budget). A 1024² `Depth32Float` spot layer is 4 MiB, so the budget is a deliberate VRAM draw-down
  (the live spot pool is already 96 × 4 MiB); pick a small `N` and treat caching as best-effort:
  slots beyond `N` stay on the per-frame path. Evict LRU by last-dirty frame.
- **Light-keyed cache key.** Key on the dynamic light's identity (its candidate/level-light index and
  slot), analogous to `CacheKey` (`:8`), so hysteresis-stable slots hold their cache entry across
  frames and a slot reassignment invalidates it (mirrors `slot_reassignment_invalidates_cache_layer`,
  `:475`).
- **Cone/position dirty test — the one new piece.** Statics never move, so the promoted cache has no
  dirty test. A dynamic light can move: compare the slot's current light-space cone matrix (the
  `slot_cone_matrices` / `face_matrices` already stashed for the cull, `renderer_light_slots.rs:322`,
  `spot_shadow.rs:60`) against the matrix cached when the depth was last rendered; `needs_world_render`
  only when it changed (beyond an epsilon). A clean slot skips both the world raster and — via the
  same `should_dispatch_*` path — the cone cull.
- **Wire into the depth loops.** In `record_spot_shadow_depth` / `record_cube_shadow_depth`
  (`renderer_shadow_passes.rs:184`, `:421`), the dynamic branch (the `else` after
  `promoted_plan`, `:344`, `:580`) currently always opens a full world-depth pass. Give it the same
  warm/copy/entity structure the promoted branch uses (`:253–341`): warm + clean → copy cache→pool +
  optional entity occluders, no world pass; dirty or unbudgeted → render world into the cache (or
  straight into the pool if unbudgeted) and mark warm. Entity occluders still re-render per frame via
  `LoadOp::Load` (`:318`), unchanged.

Keep the copy-replaces-clear invariant (`cube_face_needs_clear`, `cube_shadow.rs`) — a warm copy
stands in for the `Clear(1.0)` baseline and never leaves stale depth.

## Sequencing

`(d) → (c) → (a)`.

- **(d) first.** Without dynamic shadow-pass timestamps on a promoted-light-free map, no later fix is
  provable. Size S.
- **(c) next.** Count-independent, VRAM-free, helps every occupied slot; the parallel-cull win is
  visible on both stress maps immediately and does not depend on (a).
- **(a) last.** Highest value on static-in-place warrens (both stress maps qualify) but the largest
  change and the only one with a VRAM budget to tune. Reuses (c)'s warm-skip path for the cull side.

## Decisions

- **Attack cost, not count.** The merge barred gate-tightening (orientation invariance,
  `shadow_ranking.rs:817` / `:850`), and lowering the pool cap is a quality knob. So the levers are
  per-slot: parallelize the cull (c, count-independent) and cache the world raster of stationary slots
  (a). The old spec's "gate-tightening is the primary fix" is inverted — it is now out of scope.
- **Reuse the merged cache, don't fork it.** `promoted_depth_cache.rs` already implements the exact
  array/copy/warm/skip mechanism the old fix (a) planned to import from `static-light-entity-shadows`
  Task 6. Extend it (or a sibling sharing its helpers) rather than building a second cache.
- **Budget, not full-pool.** A 96-spot + 36-cube-face full cache roughly doubles shadow VRAM (spot
  pool alone is 96 × 1024² × 4 B). Keep the dynamic cache O(budget) with LRU eviction; overflow slots
  stay on the per-frame path and are the parallel cull's (c) job to keep cheap.
- **Timestamps first, measured on a promoted-free map.** `stress-warren-lit` has no promoted-static
  lights, so today's `promoted_depth_cache_upper` bracket never opens there and the crawl is invisible.
  The new `shadow_cull` + `shadow_depth` pairs must open unconditionally.

## Risks

- **(c) region-contract drift (highest for c).** The single (slot, leaf) dispatch must write each
  slot's culled leaves into the same indirect sub-region `draw_slot_indirect` reads
  (`shadow_cull.rs:301–325`, `region_stride_bytes`-keyed). A mismatch silently corrupts shadow draws.
  Pin with the per-slot cull-output equivalence check in Acceptance.
- **Shared-shader breakage (c).** Mutating `bvh_cull.wgsl` in place would break the camera cull and
  trip `candidate_shader_reuses_bvh_cull_struct_layouts` (`compute_cull.rs:1073`). Fork the shader; the
  AC requires the camera path and that test untouched.
- **Dirty-test correctness (a).** A too-loose epsilon leaves a moved light showing stale shadow; too
  tight defeats the cache. A dynamic light whose *world occluders* changed but whose cone matrix did
  not (rare — world geometry is static) is safe because world depth is a function of the cone matrix
  alone. Entity occluders always re-render (`LoadOp::Load`), so entity motion never needs invalidation.
- **VRAM budget (a).** The dynamic cache is a fixed draw-down on top of the live pool. Keep `N` small
  and LRU-evicted; validate residency on the compatibility-floor GPU (rendering_pipeline.md §10).
- **Overlapping timing spans (d).** `shadow_depth` (whole loop) and `promoted_depth_cache_upper`
  (promoted-only, opened inside the loop) overlap by design — they measure different scopes, not a
  double-count of one span. Document it in the pass-label comment.

## Related work

- **`context/plans/ready/static-light-shadowmask-world-receipt/`** — the merge that landed
  `promoted_depth_cache.rs`, `shadow_ranking.rs::assign_slots_with_hysteresis`, and the
  orientation-invariance invariant. (a) extends its cache to dynamic slots; (b)'s premise died with its
  invariant.
- **`context/plans/done/perf-dynamic-light-pvs-cull/`** — shipped the origin-cell gate, later widened
  to the influence-vs-reachable-cell test. That widening is now protected by the orientation-invariance
  tests; do not re-narrow it.
- **`context/plans/drafts/perf-anti-penumbra-pvs/`** — shrinks the drawable PVS. Complementary and
  independent; it does not interact with this plan now that the drawable-PVS gate is out of scope.
- **`context/plans/ready/perf-forward-light-cull/`** — sibling forward-shading-loop cull for the same
  large-map dynamic-light cost. Independent (no build-order dependency); its tight drawn-cell cull is
  contribution-only and does not touch this plan's wide eligibility gate. Its perf AC reuses the existing
  `forward` timing pair; this plan's Task 1 brackets remain the shadow-side measurement substrate.
- **`context/research/archived-plans/perf-promoted-static-light-load/`** — findings note: the promoted-static
  *forward-entity* per-fragment load is bounded at ≤ 10 by the same `MAX_PROMOTED_SPOT + CUBE` budget this
  plan caps. The only growth is the CPU selection/promotion scan over the uncapped baked
  `EntityShadowLights` set; a top-K selection cap in `entity_shadow_select.rs` is the companion lever if
  ever needed.
