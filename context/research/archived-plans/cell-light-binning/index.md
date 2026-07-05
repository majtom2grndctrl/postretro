> **⚠️ ARCHIVED 2026-07-05 — not an active plan. Do not implement from this.**
> This investigation's recommendation was "defer binning, ship `perf-forward-light-cull` as a point solution." That point solution is now being shelved (it optimized a non-bottleneck for the engine's static-lighting design), so the recommendation is moot. Kept for the research mapping of the engine's per-frame light-reach queries only. Live disposition: `context/plans/roadmap.md` Epic 17.

# Cell → Light Binning — Design Investigation

> **Status:** design investigation, NOT a ready-to-build spec. Maps the engine's
> several independent per-frame "does light L reach region R" queries, designs a
> single shared cell→light index they could all gather from, and weighs it
> honestly. **Verdict up front:** the raw CPU-scan consolidation is worth
> microseconds and is not the reason to build this; the real value is (1) a
> streaming residency substrate and (2) the forward per-fragment win — and the
> forward win is already captured standalone by `perf-forward-light-cull`. So the
> recommendation is **defer the binning, ship `perf-forward-light-cull` as a
> point solution**, with named triggers to revisit.
> **Related:** `context/lib/rendering_pipeline.md` §4 (lighting, promoted static
> lights), §7.1 (visibility/cull prepasses), §7.3 (forward light loop), §10
> (bind-group / per-stage storage budgets) · `context/lib/build_pipeline.md`
> (cells id 38, cell locator id 39, `ChunkLightList` id 23, affinity CSRs,
> `FogCellMasks` id 31) · `context/research/spatial-streaming.md` (cells are the
> one residency substrate) · sibling perf drafts
> `context/plans/ready/perf-forward-light-cull/`,
> `context/research/archived-plans/perf-shadow-caster-culling/`,
> `context/research/archived-plans/perf-promoted-static-light-load/`.

Anchors verified at HEAD `fd10d58`.

---

## 1. Problem

Several per-frame queries independently ask a variant of "which lights reach this
region?" Each scans a light set against a set of cell/bounds AABBs with a
sphere-vs-AABB test. They were built one at a time, share no structure, and each
re-derives the same spatial relationship. Cells (BSP leaves, `cell_id`) are the
natural common substrate — they are already the visibility unit and the
streaming residency unit (`spatial-streaming.md` §3). The question: is there a
single cell→light index all consumers should gather from, and is it worth
building now?

---

## 2. The query map (verified)

Every per-frame or load-time light-vs-region query in the engine. **Region**
column is the axis that matters — tight (drawn), wide (portal-reachable, empty
cells included), or an entity/bounds filter.

| # | Query | Anchor | Light set | Region (tight / wide / filter) | Cost class | When |
|---|-------|--------|-----------|-------------------------------|-----------|------|
| 1 | **Shadow-slot eligibility** — candidate influence sphere vs reachable cells | `shadow_candidate_reaches_visible_cell` (`renderer_lighting.rs:73-89`) → `light_reaches_visible_cell` (`lighting/src/lib.rs:139-155`); driven `renderer_light_slots.rs:79-127` | mixed: dynamic-tier + compiler-selected static candidates | **WIDE** — `reachable_cell_aabbs` = `fog_reachable`, empty `face_count==0` cells included (`main.rs:2219-2239`) | O(candidates × reachable cells) | per-frame |
| 2 | **Promoted-static promotion driver** — per selected-static state, linear `.position()` scan of the candidate selection list; slot ranking iterates full candidate list per pool | `update_promoted_static_weights_and_records` loop `renderer_light_slots.rs:772-776`; ranking `:896` | static (baked `EntityShadowLights`, id 40, length `N`) | consumes query 1's `visible_lights` gate; not itself a reach test | O(N × candidates) + sort | per-frame |
| 3 | **Mesh entity-shadow relevance** — per drawn/near entity, iterate all `N` selected-static lights, sphere vs the entity's model AABB | `selected_static_shadow_light_reaches_bounds` (`mesh_render.rs:351-364`), called per entity `:270` | static (`world.entity_shadow_lights`, `N`) | **entity model bounds** (not a cell) | O(entities × N) | per-frame |
| 4 | **Forward dynamic-light cull** (proposed, not built) — dynamic influence sphere vs drawn cells | `perf-forward-light-cull` spec; drawn set from `determine_visible_cells` (`visibility/src/visibility.rs:471`), drawable filter, frustum fallback `visible_cells_frustum_all:281-290` | dynamic-tier only | **TIGHT** — drawable `VisibleCells` | O(dynamic × drawn cells) | per-frame |
| 5 | **Static specular / SDF-light chunk list** (baked precedent) — per-chunk static light index list | `bake_chunk_light_list` (`chunk_light_list_bake.rs:75-100`); consumed `forward.wgsl:1030-1053` (specular), `sdf_light_select.wgsl` (per-fragment SDF-light select), `billboard.wgsl` (per-vertex specular) | static (compacted `!is_dynamic` spec-light slots) | **uniform 8 m voxel grid**, NOT cells — keyed `floor((p−origin)/cell_size)` (`render-cpu/src/chunk_list.rs`, `forward.wgsl:1030-1040`) | O(1) per fragment (grid lookup) | baked at compile; runtime lookup |

**Not additional CPU reach queries** (already region×light structures, listed so
the design does not double-count or reinvent them):

- **Animated-SH / direct-SH delta affinity CSR** — `affinity_offsets` /
  `affinity_lights` map each 4³-probe affinity cell to the lights overlapping it
  (`DeltaShVolumes` id 27, `DirectShDeltaVolumes` id 41; `rendering_pipeline.md`
  §4). Baked region→light binning, GPU-consumed in the SH compose pass. **This is
  already a baked cell/region→light index** — direct evidence the pattern is
  sound and the engine already carries one.
- **`FogCellMasks`** (id 31) — per-cell `u32` bitmask of overlapping fog volumes;
  runtime ORs every reachable cell's mask (`rendering_pipeline.md` §7.5). Baked
  cell→volume binning, gathered over the wide reachable set — structurally the
  same gather this design proposes, for volumes instead of lights.
- **Animated-lightmap compose cull** — GPU tile dispatch culled against the
  visible-cell bitmask (`rendering_pipeline.md` §7.1 step 4). A region gate, not a
  CPU reach scan.

Reflection-probe light lists: **do not exist** — `env_cubemap` marks a bake
position only; the cubemap bake tool is out of scope (`build_pipeline.md`
§Entity resolution). No consumer to fold in.

**Read of the map:** queries 1–4 are CPU sphere-vs-AABB scans that differ only in
*which light set* × *which cell set*. Query 5 is the one already-baked
light-list — but keyed on a voxel grid divorced from cells, so it does **not**
share cells as its substrate today. That mismatch is the most interesting finding:
the engine's existing per-cell light list is not per-*cell*.

---

## 3. Proposed binning

### 3.1 Data structure

One logical index: **cell → list of light indices whose influence sphere overlaps
that cell's bounds.** Cell = BSP leaf, keyed by `cell_id` (id 38), the id every
baked primitive already carries and the id that survives cell clustering
(`spatial-streaming.md` §3). Two tiers by bake participation — the same split the
lighting architecture already draws (`rendering_pipeline.md` §4):

| Tier | Built | Lives | Serves |
|------|-------|-------|--------|
| **Static** (positions fixed) | **Baked** — extends `chunk_light_list_bake.rs`, but re-keyed from the 8 m voxel grid to `cell_id` | new baked PRL section (or a cell-keyed re-issue of `ChunkLightList` id 23) | query 5 (specular/SDF), query 2/3 static-candidate reach, query 1 static-candidate half |
| **Dynamic / scripted** (small moving population) | **Runtime** — rebuilt per frame from dynamic influences × `world.cells` | app-side, `postretro-lighting` (wgpu-free, §4 ownership boundary) | query 4 (forward cull), query 1 dynamic half |

Hybrid by construction: a consumer's gather **unions the static (baked) and
dynamic (runtime) lists** over its cell set. Where it lives is **both**: the
static tier is a baked section over `world.cells`; the dynamic tier is an
app-side rebuild over `world.cells`. Neither crosses the wgpu boundary — the
reach math is CPU (`rendering_pipeline.md` §4 "light-reachability CPU math may
live in `postretro-lighting`").

Orientation: **cell → lights** (not light → cells) for the runtime tier, because
gathers iterate a cell set (drawn / reachable / entity-containing) and want each
cell's list directly. The baked tier is a CSR (`cell_span_offset[]` + flat index
list), mirroring `CellDrawIndex` (id 37) and the affinity CSRs.

### 3.2 Per-consumer gather

Each query becomes "**union the light lists over MY cell set**," preserving its
current filter exactly:

| Consumer | Cell set gathered over | Preserves |
|----------|------------------------|-----------|
| Forward cull (query 4) | **drawn** cells (drawable `VisibleCells`) — tight | contribution is per-drawn-fragment; tight is correct (`perf-forward-light-cull` Correctness) |
| Shadow eligibility (query 1) | **reachable** cells (`fog_reachable`, empty cells included) — **WIDE** | must stay wide — see §4 |
| Promoted selection (query 2/3) | cells the entity's bounds overlap (via `locate_cell` / bounds-vs-cell) | replaces per-entity O(N) scan with gather over the entity's few cells |
| Static specular / SDF (query 5) | the fragment's own cell (baked lookup) | same per-fragment cost, cell-keyed instead of voxel-keyed |

The forward and shadow gathers differ only in their cell-set input — the tight
drawn set vs the wide reachable set — exactly the asymmetry the two sibling specs
already enforce. The binning does not merge those inputs; it merges the
*mechanism* that answers them.

### 3.3 Building it

- **Static tier:** at bake, for each static light, rasterize its influence sphere
  against every cell AABB it overlaps and append its index to those cells' lists.
  Portal-aware clipping is already present in `chunk_light_list_bake.rs` (it takes
  `tree` / `portals` / `exterior_leaves`); the change is the **key** (cell id, not
  voxel index), not the reach algorithm. Fits the warm/cold cache as a new stage
  version (`build_pipeline.md` §Build Cache).
- **Dynamic tier:** per frame, for each dynamic/scripted light, clamp its
  influence center to each cell AABB and test squared distance ≤ r² — the same
  test queries 1/3/4 already run — appending to the overlapped cells' runtime
  lists. Caller-owned scratch, allocation-free steady state (the
  `visible_forward_light_indices` `out: &mut Vec<u32>` pattern from
  `perf-forward-light-cull` Task 1).

---

## 4. Correctness

The load-bearing invariant: **no consumer may drop a light it currently keeps.**

1. **Bin by influence-overlap, not home cell.** A light must be assigned to
   **every** cell whose AABB its influence sphere overlaps, not just its home
   cell. This is the exact lesson of the removed own-cell-PVS gate: the comment at
   `renderer_light_slots.rs:65-77` records that the prior gate "dropped a light
   whose cell left the shrinking PVS on pitch-down even though it still lit and
   shadowed geometry in view," and `light_reaches_visible_cell`'s doc
   (`lighting/src/lib.rs:106-138`) + the regression test
   `light_with_off_pvs_leaf_but_reachable_receiver_is_eligible` (`lib.rs:751`) pin
   the replacement. Overlap-binning reproduces the fixed behavior: a light whose
   home cell is not in the gather set but whose sphere reaches a gather-set cell is
   still in that cell's list, so the union keeps it.

2. **The gather is the AABB-lifted form of today's exact test.** Each consumer
   today computes `any(cell in my_set: sphere overlaps cell.aabb)`. The binning
   precomputes, per cell, `{lights: sphere overlaps this cell.aabb}`; the gather
   computes `union(cell in my_set: bin[cell])`. These are equal sets. So every
   consumer's output is bit-identical — the binning changes *when* the
   sphere-vs-AABB test runs (precompute vs inline), not its result.

3. **The WIDE shadow set stays wide.** Shadow eligibility (query 1) gathers over
   the **reachable** cell set (`fog_reachable`, empty `face_count==0` cells
   included), never the drawn set. This is non-negotiable: the merged shadow rework
   locked shadow-slot eligibility **orientation-invariant** — proptest
   `shadow_slot_set_invariant_under_camera_orientation`
   (`crates/lighting/src/shadow_ranking.rs:850`) and the regression
   `dynamic_spot_keeps_slot_when_cone_aabb_outside_pitched_camera_frustum`
   (`:817`). Narrowing the shadow gather to the (orientation-dependent) drawn set
   would resurrect the pitch-down "entity shadow vanished" bug. The binning's
   per-cell lists must therefore include lights reaching empty reachable cells —
   which they do, because empty cells still have bounds and the bake/rebin tests
   influence-vs-cell-AABB regardless of `face_count`. **The forward cull may be
   tighter; the shadow gather may not.** The binning enforces this by input cell
   set, not by the index structure.

4. **Moving lights.** The dynamic tier rebuilds per frame, so a moved light lands
   in its new cells' lists automatically. Note scripted lights animate
   brightness/color/aim but **not position** — their influences are load-time
   static (`renderer_light_slots.rs:250-252`, cited in `perf-forward-light-cull`
   Decisions) — so the moving population that truly needs per-frame rebin is only
   genuinely-relocating gameplay lights, a small set. A conservative
   motion-dilated influence bound is the alternative if per-frame rebin ever shows
   in a trace; not needed at current scales.

5. **Uncullable `f32::MAX` fallback.** A light with a missing influence record
   degrades to radius `f32::MAX` (`uncullable_light_influence`,
   `renderer_lighting.rs:44-49`). Its sphere overlaps **every** cell AABB, so it
   is binned into every cell and kept by every gather — preserving the
   never-culled contract exactly (matching the `light_reaches_visible_cell`
   DrawAll-sentinel / missing-influence behavior).

6. **DrawAll sentinel.** An empty gather cell set (empty world / fallback
   visibility) must keep all lights, matching `light_reaches_visible_cell`'s
   empty-slice contract (`lib.rs:144-147`). The gather treats empty-cell-set as
   identity (every light), not empty (no light) — the same sentinel each consumer
   already honors.

---

## 5. Cost / benefit (honest)

### 5.1 The CPU scans are already cheap — quantify what consolidation saves

Order-of-magnitude arithmetic on `stress-warren-lit` (≈157 dynamic lights;
`perf-forward-light-cull` Problem) with a generous drawn/reachable cell estimate
of ~10² cells and the promoted-static findings' big-map estimate (`N=100`,
entities=50, candidates=250; `perf-promoted-static-light-load` §2):

- **Query 1** (shadow eligibility): ~250 candidates × ~10² reachable cells ≈
  2.5 × 10⁴ sphere-AABB tests/frame.
- **Query 3** (mesh reach): 50 entities × 100 lights = 5 × 10³ tests/frame.
- **Query 2** (promotion `.position()`): 100 × 250 = 2.5 × 10⁴ integer
  compares/frame.
- **Query 4** (forward cull, if built): ~157 × ~10² ≈ 1.6 × 10⁴ tests/frame.

Each is a few tens of thousands of clamp/dot/compare ops — **CPU microseconds**,
riding behind the per-frame portal walk they already share a cost class with.
Consolidating them into one binning saves the *duplicate* traversals but the
absolute figure is small: **the CPU-scan consolidation is a code-quality win, not
a frame-time win.** Do not sell it as a perf fix.

### 5.2 Build cost of the binning

- **Static tier:** baked, amortized to compile; warm cache hides re-bakes. Zero
  per-frame cost. Adds one PRL section + one cache stage.
- **Dynamic tier:** per frame, O(moving lights × cells each touches) to rebuild —
  same order as the scans it replaces (§5.1), plus the materialization write. So
  the runtime bin **does not beat** the sum of the scans on raw time; it wins only
  by being computed once and gathered many times. At current scales that is a wash
  in microseconds.

### 5.3 Where the real value is — separate the two

1. **The forward per-fragment win (valuable).** The forward loop today iterates
   the **map-wide** dynamic count for **every drawn fragment** with only a
   per-fragment influence early-out (`forward.wgsl:1093-1103`). Culling to
   lights-reaching-drawn-cells removes N−V iterations × every fragment. This is
   the one large win — and it is a **per-fragment shading** win, wholly captured
   by `perf-forward-light-cull` **standalone**, whose whole mechanism is one
   drawn-cell gather. The binning does not add to this win; it would merely be the
   structure the gather reads from.
2. **The CPU-scan consolidation (modest).** §5.1 — microseconds, code quality.
3. **The streaming foundation (deferred value).** §6 — real but only pays off once
   streaming lands.

So the binning's *marginal* value over shipping `perf-forward-light-cull` as a
point solution is (2) + (3): code quality now, streaming substrate later. Neither
is a per-frame bottleneck. **There is no crawl here to fix** — the promoted-static
findings already established the only unbounded quantity (`N`) drives cheap CPU
scans, not a per-fragment loop (`perf-promoted-static-light-load` §1, §2).

---

## 6. Streaming fit

Per-cell light lists are a **natural streaming residency unit** and align with
`spatial-streaming.md` rather than fighting it — provided one rule is kept:

- **Key on `cell_id`, never on a parallel voxel grid.** The streaming invariant is
  "one residency substrate = cells" (`spatial-streaming.md` Key invariant, §3).
  The static tier's re-key from the existing 8 m voxel grid (query 5) to `cell_id`
  is precisely the move that stops the light list from being a second, streaming-
  incompatible spatial query. A cell-keyed static light section becomes
  per-cluster-addressable exactly like lightmap layer ranges and SH probes — it
  slots into the epic's "generalize residency to remaining spatial sections"
  (`spatial-streaming.md` §8 slice 5) for free.
- **The dynamic tier rides the visible-cell signal already computed** — the same
  reason cells are the substrate (`spatial-streaming.md` §3). No new spatial query.
- **Do not build the static section before cell clustering exists.** Clustering is
  slice 1 of the streaming epic (`spatial-streaming.md` §8) and is not built. A
  cell→light section baked now, pre-clustering, would key on raw BSP leaves and
  need re-issuing once clusters land — so building it now paints a corner. The
  affinity CSRs (id 27/41) and `FogCellMasks` (id 31) are the precedents to mirror
  **when** that section is built, not before.

Net: the binning is streaming-aligned *if and only if* it is cell-keyed and built
alongside (not before) clustering. That timing is the core of the recommendation.

---

## 7. Recommendation

### 7.1 Build now, or later? — **Later.**

The consolidation is worth building **eventually**, driven by code-quality +
streaming-foundation, **not** by scan cost. It is **not** worth building **now**:

- The one large win (forward per-fragment) is fully captured by
  `perf-forward-light-cull` as a small point solution — no binning needed to get
  it.
- The CPU-scan consolidation saves microseconds (§5.1) — below the bar for a new
  PRL section + bake stage + per-frame runtime rebin.
- The streaming value (§6) only pays off after **cell clustering** (streaming
  slice 1) exists, and building the static section before clustering keys it wrong
  (§6). Streaming itself is pre-spec (`spatial-streaming.md` Status).

There is no measured bottleneck forcing it. Build it as part of the streaming
epic, not ahead of it.

### 7.2 Fate of `perf-forward-light-cull` — **(b) keep standalone; consolidate later.**

Of the three forks:

- **(a) Redirect it to consume the binning (binning as prerequisite)** —
  **rejected.** Blocks a cheap, fully-specced, independently-landable win behind a
  speculative substrate that shouldn't be built until clustering lands. Inverts the
  dependency: the point solution is ready; the substrate is not.
- **(b) Keep it standalone as a point solution, consolidate later** —
  **recommended.** Its committed shape (Methodology "(B)": keep full buffers, add a
  per-frame visible-light index list) already produces a **drawn-cell dynamic-light
  gather** (`visible_forward_light_indices`, Task 1) — which *is* the dynamic tier's
  forward gather for one cell set. Shipping it does not foreclose the binning; it
  pre-builds the exact predicate the binning would reuse. When the binning later
  lands, `visible_forward_light_indices` becomes "gather the dynamic bin over drawn
  cells" — a refactor, not a rewrite.
- **(c) Supersede it entirely with the binning spec** — **rejected.** Would delay a
  real per-fragment win for a substrate whose own value here is microseconds + a
  deferred streaming payoff.

**Reconciling the Fable rejection.** The Fable agent rejected **baked per-cell
dynamic light lists for the forward cull alone** — correctly: scripted lights
animate at runtime and the drawn set is per-frame, so a baked table still needs
the per-frame visible-cell join, which *is* the work (`perf-forward-light-cull`
Methodology, "Per-cell baked dynamic-light lists"). **Does serving multiple
consumers change that calculus? Only partially, and not in favor of baking
dynamic lists:**

- The dynamic tier stays **runtime** regardless of how many consumers it serves —
  Fable's rejection holds unchanged. Multi-consumer serving raises the value of a
  *runtime* materialized dynamic bin (compute once, gather thrice) over three
  independent scans, but that value is the §5.1 microseconds — real, modest, not
  decisive.
- What multi-consumer serving *does* legitimately add is value to the **static**
  tier (baked), which Fable never rejected — it rejected baking the *dynamic* tier.
  A cell-keyed **static** section (re-keyed query 5) could serve specular +
  SDF-select + the static-candidate half of shadow eligibility + the mesh
  entity-shadow scan. That is the defensible baked piece. But it is a streaming-era
  build (§6), not a forward-cull dependency.

So: forward cull stays dynamic-runtime and standalone; the binning, when built,
absorbs its predicate as one gather and adds the baked static tier separately.

### 7.3 Shadow eligibility (query 1) and promoted selection (query 2/3) — migrate, or stay?

**Stay for now; migrate only alongside the binning if it is ever built.**

- **Shadow eligibility (query 1):** would migrate to "gather the dynamic bin +
  static bin over the **reachable** cell set" — same result, must stay wide (§4.3).
  The scan is cheap (§5.1) and the wide-set correctness is delicate; migrating it
  now buys nothing and risks the orientation-invariance guard. Leave
  `shadow_candidate_reaches_visible_cell` as-is.
- **Promoted selection (query 2/3):** this is the **cleanest consolidation
  beneficiary** — the per-entity O(entities × N) mesh scan
  (`selected_static_shadow_light_reaches_bounds`) and the O(N × candidates)
  promotion scan are exactly the "class 2" growth flagged in
  `perf-promoted-static-light-load` §2. A **static** cell→light bin would turn the
  mesh scan into a gather over the entity's few cells (O(entities ×
  lights-in-entity-cell)). **But** `perf-promoted-static-light-load` already
  prescribes a simpler point fix for that growth if a profile ever demands it: a
  compiler-side **top-K cap on `N`** in `entity_shadow_select.rs`. Prefer the cap
  as the cheap standalone fix; treat the static bin as the *consolidated* fix that
  supersedes the cap only once the streaming-era static section exists. Do not
  build either speculatively — the findings note both are latent, not measured.

---

## 8. If/when built — sketch and sequencing

Not a task list (this is an investigation), but the shape a future spec would take,
source-anchored, in streaming-epic order:

1. **Cell clustering** (streaming slice 1, `spatial-streaming.md` §8) — prerequisite.
   The static section must key on the residency unit, so it cannot precede this.
2. **Static cell→light section** — re-key `chunk_light_list_bake.rs` from the 8 m
   voxel grid to `cell_id` (or emit a sibling section), CSR like `CellDrawIndex`
   (id 37) / the affinity CSRs (id 27/41). New cache stage version
   (`build_pipeline.md` §Build Cache). Runtime lookup switches from
   `floor((p−origin)/cell_size)` to the fragment's `cell_id`.
3. **Runtime dynamic bin** — materialize the dynamic tier in `postretro-lighting`
   over `world.cells`; `perf-forward-light-cull`'s `visible_forward_light_indices`
   becomes "gather over drawn cells." Caller-owned scratch.
4. **Migrate consumers** — forward cull → drawn-cell gather; shadow eligibility →
   reachable-cell gather (WIDE, guarded by `shadow_ranking.rs:817`/`:850`); mesh
   entity-shadow scan → entity-cell gather. Each migration is behavior-preserving
   by §4.2 and must keep its existing pinning tests green.

### Triggers to revisit (revisit if ANY fires)

- **Cell clustering lands** (streaming slice 1) — build the static section
  cell-keyed and per-cluster-addressable as part of slice 5.
- **A profiled map shows the promoted-static CPU scans (class 2) in a trace** —
  first reach for the top-K cap (`perf-promoted-static-light-load` Recommendation);
  consider the static bin only if the cap is insufficient or streaming is already
  underway.
- **Clustered-forward becomes necessary** (a profiled map shows many lights *inside*
  the drawn set, not off-screen — the case `perf-forward-light-cull` Methodology
  defers) — the runtime dynamic bin is its natural input; build the binning as its
  prerequisite then.

Absent any trigger, the standing state is: `perf-forward-light-cull` ships
standalone; the binning stays a documented design the streaming epic will absorb.

---

## 9. Cross-references to apply (for the maintainer — not applied here)

Per task constraints, no other spec or engine file was modified. If desired:

- `perf-forward-light-cull/index.md` — add a "Related work" bullet noting this doc
  concludes fork **(b)**: keep it standalone; its `visible_forward_light_indices`
  predicate is the dynamic bin's forward gather when consolidation later lands.
- `spatial-streaming.md` §2 (streamable table) / §8 (decomposition) — add
  "per-cell light lists (static baked + dynamic runtime)" as a spatial section that
  rides the same cell-cluster residency, built in slice 5, keyed on `cell_id`.
- `perf-promoted-static-light-load/index.md` — note the static cell→light bin as
  the *consolidated* alternative to the top-K `N` cap for its class-2 scans, gated
  on the streaming-era static section.
