# Shadowmask Bake Scaling — Bounded-Memory Composite, Governed Parallelism, Progress

## Goal

Make the compiler's shadowmask atlas bake finish on warren-class maps instead of
OOM-killing. Peak resident memory in the shadowmask stage today scales with
`selected_lights × atlas_covered_texels` because it holds every selected light's
per-light `LightmapLayer` in memory before compositing; on `stress-warren-lit` (157
lights) that exceeds 16 GB and SIGKILLs. Bound the resident set of full 48-byte
per-light `LightmapLayer`s to a fixed window, saturate worker cores under the existing
governor, and give the stage determinate progress. Output stays byte-for-byte
identical — this is an allocation-lifecycle, concurrency, and reporting change, not a
format or quality change.

This is a **large constant-factor reduction on the dominant term, not an asymptotic
bound.** The non-additive composite (per-texel overlap graph + one global channel per
light) needs every covering-light record resident before it can assign channels, so a
compact membership — per covering (texel, light): the texel's global position, the
light's compact index, and its full-precision `raw_visibility` — remains
`Σ_lights covered_texels`. Still light-scaling, but roughly a third of the 48-byte
`LayerTexel` (it drops the irradiance, weighted direction, and fallback normal the
shadowmask never reads). On this fixture that clears the 16 GB wall (~14 GB → order
1 GB); finer density or many more lights re-consume the headroom. The full-`LightmapLayer`
set is what this plan windows; the membership is the irreducible working set of the
global-per-light coloring (see Alternatives rejected — tiling). Keeping full-precision
`raw_visibility` in the membership lets the fill quantize once, identically to today, so
no layer is re-read and bytes stay identical (see Sequencing, Rough sketch).

## Scope

### In scope

- Restructure the shadowmask composite (`bake_shadowmask_atlas`,
  `bake_shadowmask_atlas_cached`, `build_shadowmask_from_layers`) so it never holds
  all selected per-light layers at once: stream lights, dropping each layer after its
  contribution is recorded, bounding resident per-light layers to a fixed window.
- Governed light-axis parallelism: saturate worker cores up to the `-j` / TUI permit
  cap across the selected-light work, entering the shared governor once per work item
  at its outermost boundary, without nested-governor deadlock and without
  re-unbounding memory.
- Determinate progress for the shadowmask stage: publish a real total and advance it,
  so the stage renders a percentage + ETA instead of a bare indeterminate spinner.
- Byte-identity preserved on both the cached (warm) and `--no-cache` (cold) paths,
  including the existing >4-overlap channel-drop behavior.

### Out of scope / non-goals

- **The lightmap warm section-miss materialization.** The cached lightmap path holds
  all N `LightmapLayer`s the same way (`pipeline.rs` builds
  `Vec<LightmapLayer>` then `composite_layers`); that is
  `lighting-scale--lightmap-bake-incremental-flush` Task 2, which folds each light in
  and drops it. This plan does not touch the lightmap composite. The two together
  cover every path that OOMs (see Direction).
- **The atlas-size output buffer** (`width × height × layer_count × 4`). Independent of
  light count; a function of lightmap density, bounded by `--lightmap-density` and owned
  by `lighting-scale--lightmap-bake-scaling`. This plan shrinks the dominant
  lights-scaling term by a large constant factor (48-byte `LayerTexel` → a ~third-size entry)
  but does not make it light-count-independent — see Goal.
- **Any change to `.prl` output bytes**, the `ShadowmaskAtlasSection` format, the
  >4-overlap drop policy, or the runtime. Lifting the drop is
  `shadowmask-no-drop-atlas` (the sibling format expansion). The byte-identity gate is
  this plan's contract.
- **The shadowmask stage cache key / memo semantics.** The `"shadowmask_atlas"` and
  `"lightmap_layer"` cache keys and versions are unchanged (subject to the
  cross-plan coupling in Sequencing).
- **Selection budgeting / ranking.** No cap on selected shadow lights — capping would
  drop shadows.

## Direction

**Problem.** The shadowmask composite materializes every selected light's per-light
`LightmapLayer` (48 B × covered texels each) before it can build the cross-light
overlap graph and assign RGBA channels, so peak resident memory scales with
light-count and OOMs on warren-scale maps. Secondary: the light axis is serial
(only charts within one light parallelize) and the stage publishes no progress total,
so a slow bake also reads as a hang.

**Prior commitments.** `lighting-scale--lightmap-bake-incremental-flush` establishes
the bake-scaling posture: bound resident memory by streaming per-light contributions
and dropping each, behind a byte-identity gate. This plan is the shadowmask analogue
and adopts the same posture. The governor contract holds: parallel ray work enters
the shared `Governor` at its outermost boundary (never bare `checkpoint`, which
honors pause but not the `-j` permit cap), so `-j` and TUI throttle keystrokes —
which call `set_permits`/`set_paused` on the shared `Arc<Governor>` — take effect at
each work item's next admission. `lightmap-bake-throughput` (done) established the
shadowmask stage's `BakeControl`/governor-`Arc` threading this plan builds on, and
deliberately gave the stage "its own `StageProgress` or an unregistered indeterminate
one." This plan **revises** that to a published total so the stage renders a percentage
— a witting divergence: the throughput plan left progress out because the composite was
a single opaque call; this plan restructures it into countable (light, chart) units, so
a total is now meaningful. The shadowmask cache memo (`stage_id "shadowmask_atlas"`,
keyed through per-light layer hashes and `--soft-shadow-samples`) is preserved.

**Placement.** The fix lives in the shadowmask composite and its pipeline stage, not
in the shared lightmap composite. The shadowmask composite is structurally distinct
from the lightmap's additive `composite_layers`: it does not sum contributions across
lights, it builds a per-texel cross-light overlap graph, 4-colors it, and writes each
light's visibility into its assigned RGBA channel. That non-additive structure cannot
fold into incremental-flush's additive accumulator, so it is its own bounded-memory
restructure at its own site.

**Alternatives rejected.** (a) **Tile/region the atlas and color each tile's lights
locally**, dropping membership per tile — the only shape that would make the bound
*actually* light-count-independent (asymptotic, not just the constant-factor reprieve).
Rejected because the shipped id-42 contract stores **one channel per light, global
across all atlas texels** (`SpecLight` byte 56, from `static-light-shadowmask-world-receipt`):
a light's channel must be consistent everywhere it appears, so channel assignment must
see the light's whole overlap set at once. Per-tile coloring cannot provide that without
a per-texel channel store — which is exactly the format change `shadowmask-no-drop-atlas`
owns. This is *why* the membership term is irreducible here (the constant-factor bound in
Goal), not an asymptotic one. (b) Cap or rank selected shadow lights to bound N —
rejected: drops shadows (violates the project directive). (c) Fold the shadowmask into
incremental-flush's lightmap streaming — rejected: the shadowmask composite is
non-additive, so it cannot reuse the additive accumulator; coupling entangles a
byte-identical lightmap change with the shadowmask's distinct structure. (d) Add only
progress + parallelism, leave materialization — rejected: does not remove the OOM, which
is the blocker.

**Forecloses.** Nothing material. Byte-identity keeps the format free for the sibling
no-drop plan; the streaming/window mechanism is tunable without output impact.

## Acceptance criteria

- [ ] The baked `ShadowmaskAtlasSection` is byte-identical to the pre-change bake on a
  multi-selected-light, multi-layer fixture, on both the cached and `--no-cache`
  paths — including a fixture with a texel covered by >4 selected lights (the same
  masks are dropped as before). The reference is a **committed golden snapshot** of the
  pre-change section bytes for these fixtures (captured before the restructure): because
  `build_shadowmask_from_layers` — the current reference implementation — is itself
  restructured, a self-diff of two co-changing current paths (as
  `preloaded_layer_bake_matches_uncached_…` does) is not a valid anchor.
- [ ] The shadowmask stage's resident per-light `LightmapLayer` set is bounded to a
  fixed window W independent of selected-light count (not all N), verified two ways:
  (i) a `#[cfg(test)]` resident-layer high-water counter asserts the concurrently-held
  `LightmapLayer` count never exceeds W — a **runnable** check of Invariant 2; (ii) peak
  process RSS on `stress-warren-lit` is measured **out-of-band** (e.g. `/proc/self/status`
  `VmHWM` or `/usr/bin/time -v`) and reported — a manual measurement gate, not a unit test
  (no RSS instrumentation exists in the compiler) — and the lights-scaling share drops
  materially versus pre-change.
- [ ] A `stress-warren-lit` bake at `--no-cache --lightmap-density 0.25`, which
  previously SIGKILL-ed during the shadowmask stage (~405 s, ~16 GB), completes the
  shadowmask stage. This is a **manual, multi-minute full-map acceptance run**, not a CI
  unit test.
- [ ] The shadowmask stage renders an advancing percentage + ETA (a published total),
  not a bare spinner; a TUI pause and a `-j` permit change take effect mid-stage. The
  bar reaches 100% on a fully warm rebake (all `"lightmap_layer"` cache hits), not only
  on a cold bake (Orderings 8). The total/100%-on-warm-rebake core is **unit-testable**
  via a test that owns the `StageProgress` passed to `BakeControl` (assert
  `completed() == total()`); the mid-stage TUI-pause / `-j`-change clause is an
  interaction check, effectively manual.
- [ ] The shadowmask stage saturates worker threads up to the governor permit cap; a
  bake at `-j 1` completes the stage without deadlock — including with a window `W ≥ 2`
  (no permit×window hold-and-wait, Orderings 3). The bake exposes W as a test-injectable
  parameter or `#[cfg(test)]` override so Orderings 3/4/5 can force the stated windows
  rather than relying on the shipped default.
- [ ] Re-baking the same map twice yields byte-identical shadowmask output.
- [ ] Zero selected lights (`entity_shadow_lights` absent or empty) and a single
  selected light produce the pre-change output (the `None` / empty-section paths are
  unchanged).
- [ ] A normal (non-verbose) bake gains no new per-item log spam; any per-partition
  memory/size breakdown appears only under `-v`/`--verbose`. (A review/grep gate, not a
  runnable assertion. The existing one `[cache] lightmap_layer hit`/`miss` line per
  selected light is pre-existing — this AC governs only *new* spam and the `--verbose`
  gating of any memory breakdown.)

## Tasks

### Task 1: Bounded-memory streaming composite + determinate progress (thin slice)

Restructure the shadowmask composite so peak resident per-light layer memory is
bounded and independent of the selected-light count, and give the stage a real
progress total — serial over the light axis for now (each light's `bake_light_layer`
keeps its existing per-chart `placements.par_iter()` + one governor `enter()` per chart;
only the outer loop over selected lights is serial in this slice), so this slice
falsifies the byte-identity and bounded-memory assumptions before light-axis parallelism
is added. Today `bake_shadowmask_atlas` and `bake_shadowmask_atlas_cached` build a
`Vec<LightmapLayer>` over all selected lights (the uncached path via
`.iter().map(bake_light_layer).collect()`, the cached path via a per-light loop that
`layers.push`es each cache-hit-or-baked layer), then `build_shadowmask_from_layers`
consumes the whole slice: it derives per-light masks, builds `overlap_graph` (a
`HashMap` over every covered texel → covering lights), runs
`assign_channels_with_drops` (deterministic 4-coloring with lowest-intensity drops),
and fills a `data = vec![255; width*height*layer_count*4]` buffer. **Preserve the
slice-taking seams.** `bake_shadowmask_atlas_from_layers` (pub) and
`build_shadowmask_from_layers` must stay callable with a full `&[LightmapLayer]` slice —
roughly a dozen unit tests bind to them directly (e.g. `single_light_mask_…`,
`five_way_overlap_drops_…`, `preloaded_layer_bake_matches_uncached_…`, and every cache
test's `expected`). Route both the streaming top-level path and these seams through **one**
shared membership→assign→fill core so they emit identical bytes; do not rewrite the test
surface AC 1 depends on. Keep the overlap
graph, channel assignment, and drop policy exactly as they are — channel assignment
must stay a pure, order-deterministic function of the selection and per-light layer
input hashes so output bytes do not depend on iteration order (Invariant 1). Change
only the layer lifecycle: never hold more than a bounded window of per-light layers
resident — record each light's contribution into a compact membership structure (per
covering (texel, light): the texel's global position, the light's compact index, and
its full-precision `raw_visibility`, not the full 48-byte `LayerTexel`) and drop the
`LightmapLayer` before advancing, so resident per-light layers stay O(window) instead of
O(N) (Invariant 2). Two byte-identity constraints on the membership: (i) record a
`(texel, light)` entry **iff `raw_visibility >= 0.0`** — the exact negation of the
`raw_visibility < 0.0` skip `build_shadowmask_from_layers` applies today when building
`masks` — so covered-but-dark texels never enter the overlap graph and manufacture
spurious edges; (ii) each light's **compact index is fixed from selection order before
any parallel work**, the membership carries it, and `overlap_graph`,
`assign_channels_with_drops`, and the fill all iterate by compact index, never by
scatter or completion order (Invariant 1). The fill pass sources visibility from the
membership and quantizes it exactly as `build_shadowmask_from_layers` does today
(`(raw_visibility.clamp(0,1) * 255 + 0.5) as u8`); the fill therefore **never re-reads
the layer cache**. The membership pass still obtains each light's layer once — a warm
`"lightmap_layer"` `cache.get` hit or a fresh `bake_light_layer` — exactly as today, and
the cached path still `cache.put`s a freshly baked layer at produce time, unchanged.
Preserve the cached path's whole-section memo; the `None` (empty `light_indices`) early
return and the empty-section bytes are unchanged — the all-filtered case is handled by the
`publish_total` non-empty guard below, not a byte change. **Progress** (three coordinated
pieces, mirroring the lightmap warm path): (a)
the stage must hand its `StageProgress` to the reporter via
`reporter.declare_progress(StageId::ShadowmaskAtlas, shadowmask_progress.clone())` — the
lightmap stage does this at its `begin_stage`; without it the reporter never receives the
handle and a published total renders nothing. (b) `selected_valid_lights` is known only
*inside* the bake after the out-of-range-`AlphaLights` filtering, so keep
`StageProgress::indeterminate()` at the `pipeline.rs` construction site and call
`control.publish_total(selected_valid_lights × shared.placements.len())` from inside the
bake once the valid selection is filtered (`shared.placements` is in scope there) — the
same split the lightmap path uses. **`publish_total` is reached only once the valid
selection is known non-empty:** the uncached path already early-returns its empty section
at `selected.is_empty()`, but the cached path has **no** such early return today, so gate
`publish_total` behind the non-empty check (or add the matching early return) — an
all-filtered selection must never fire `publish_total(0)` and leave the stage rendering
0-of-0 (Orderings 6). (c) `advance` must reach the published total on
**every** path: a cache-miss light advances `placements.len()` through its per-chart
`bake_light_layer` reports; a `"lightmap_layer"` cache-hit light and a light later
dropped by the >4-overlap policy each still advance a full `shared.placements.len()`; and
a whole-section (`"shadowmask_atlas"`) memo hit advances the full published total and
returns. Otherwise a warm rebake (all hits) sits at 0% and jumps to done. Keep any
per-partition memory breakdown behind the existing `--verbose` gate.

### Task 2: Governed bounded-window light-axis parallelism

On top of Task 1's streaming composite, parallelize the selected-light work so the
stage saturates worker cores up to the `-j` / TUI permit cap, without nested-governor
deadlock, without a permit×window deadlock, and without re-unbounding memory. Today only
charts within a single light parallelize: `bake_light_layer_controlled` runs
`placements.par_iter()` and enters the governor once per chart; the outer loop over
selected lights is serial. Naively wrapping the outer light loop in a second parallel
level would nest governor entries — an outer light-permit plus each inner chart-permit —
which double-counts against the permit cap and can deadlock at `-j 1`. The stage has
three phases with a hard barrier between the bake and the assignment (Invariant 3);
parallelism lives in phases 1 and 3, never straddling the barrier:

1. **Bake / membership pass (parallel, governed).** Express *only this pass* as the
   governed parallel level: `(light, chart)` work items, each taking exactly one
   `governor().enter()` at its outermost boundary (the shape the lightmap monolithic bake
   uses for charts), scattering `raw_visibility >= 0.0` texels into the global membership
   under its light's fixed compact index. **No per-`(light, chart)` callable exists today:**
   `bake_light_layer`/`bake_light_layer_controlled` (`lightmap_layer.rs`) wrap the per-chart
   body in their own `placements.par_iter().enumerate().map(…)` with a per-chart `enter()`.
   Factor that per-chart closure out of `bake_light_layer_controlled` into a callable
   per-`(light, chart)` unit the phase-1 level invokes directly, so each item takes exactly
   one `enter()` and the outer level never nests a second `enter()` inside `bake_light_layer`
   — a bounded call-site refactor of `lightmap_layer.rs`, which this task otherwise leaves
   alone (update all call sites in the same change, per `context/lib/index.md`). Accumulate
   membership by the **indexed-collect** shape (`par_iter().map().collect()`, which retains
   index order — `lightmap_layer.rs:252`), concatenating batches in **compact-index order**,
   never completion order; appending in completion order would violate Invariant 1. A warm
   cache-hit light has no charts to bake — it scatters its cached layer's texels as its own
   (non-`enter`) unit and still advances `placements.len()`. A **cache-miss** light still
   `cache.put`s its whole `LightmapLayer` (Task 1, unchanged): within its batch, join the
   light's chart outputs into the assembled `LightmapLayer` (indexed order), `cache.put` it,
   scatter its `raw_visibility >= 0.0` texels into membership under its compact index, then
   drop it — all inside the window bound so resident layers stay ≤ W. The progress `advance`
   units of Task 1 are emitted here and must sum to the published total under concurrency.
2. **Assignment (serial barrier).** After the last chart of the last light has scattered,
   run `overlap_graph` + `assign_channels_with_drops` **once** over the complete global
   membership. This barrier is mandatory: coloring from a partially-populated membership
   would miss a cross-light edge and let two overlapping lights share a channel, racing
   the fill and breaking byte-identity (Invariant 1, 3).
3. **Fill (parallel, ungoverned-safe).** Write each light's quantized visibility into its
   assigned channel of `data`. Parallelizable because writes are provably disjoint:
   overlapping lights are adjacent in the graph, so the coloring gives them distinct
   channels — distinct byte offsets at any shared texel — and non-adjacent lights never
   share a texel (state this argument in the code).

Bound resident layers to a window of W lights by **batch boundary**, not a concurrently
held primitive: submit at most W lights' `(light, chart)` items into one phase-1 parallel
level, `join`, then the next batch — so `governor().enter()` is the *only* blocking
admission and no work item ever holds a permit while blocked waiting for a window slot
(the permit×window hold-and-wait that would deadlock at `-j 1`, W≥2). The window bounds
resident **layers** only; it never partitions the membership or the coloring — the
assignment in phase 2 still runs once over every batch's membership (partitioning it per
window is the rejected per-tile shape). W and the batch mechanics are implementation
choices; the constraints are: one governor entry per bake item (no nesting), in-flight
lights capped by batch join, the phase-2 barrier before any `data` write, phase-3
writes disjoint by construction, and **W reachable by tests** (a parameter or a
`#[cfg(test)]` override) so Orderings 3/4/5 and AC 5 can force `W = 4`, `W = 1`, `W = 2`
rather than depending on the shipped default. Pause/`-j` changes are observed at each phase-1
`enter()`; if the phase-3 fill is long enough to matter, add a `checkpoint` so a pause is
honored there too.

## Sequencing

**Cross-plan independence:** `lighting-scale--lightmap-bake-incremental-flush` Task 2
bumps `LAYER_FORMAT_VERSION` and re-slices the `"lightmap_layer"` cache blobs. This plan
is designed to be independent of that reslice: its **fill** sources visibility from the
in-memory membership and never re-reads the layer cache (Task 1). The membership pass
still obtains each light's layer once — a warm `cache.get` hit or a fresh
`bake_light_layer` → `cache.put` — consuming a whole `LightmapLayer` through the existing
`StageCache` + `from_bytes` API exactly as today, and version-keyed by
`LAYER_FORMAT_VERSION`, so a reslice that bumps the version simply misses and re-bakes
rather than mis-decoding. There is **no ordering dependency** between the two plans;
build either first. The only residual is if incremental-flush reslices the
`LightmapLayer` *type itself* (per-atlas-layer chunks) rather than just its on-disk blob
— then whichever plan lands second does a bounded call-site update to the shared type
(normal pre-stable churn, per `context/lib/index.md` "update all call sites in the same
change"), not a redesign.

**Phase 1 (sequential):** Task 1 — thin slice; establishes the streaming composite,
deterministic assignment, and progress, and falsifies the byte-identity and
bounded-memory assumptions serially.
**Phase 2 (sequential):** Task 2 — consumes Task 1's streaming structure and
deterministic assignment; adds parallelism over the same work items.

## Invariants

| # | Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|---|
| 1 | Shadowmask output bytes are a pure function of (selection, per-light layer input hashes, atlas dims) — independent of bake concurrency or completion order. Requires: membership keyed by fixed compact index; only `raw_visibility >= 0.0` texels recorded; consumers iterate by compact index, not scatter order | Task 1 (fixed compact index, `>= 0.0` filter, deterministic coloring) | Task 2 phase-3 fills write only disjoint channel bytes (adjacency ⇒ distinct channel); a fill that reads scatter/completion order, or a membership missing the `>= 0.0` filter, diverges | AC 1, 6; Orderings 1, 2, 8 |
| 2 | Resident per-light `LightmapLayer` set bounded to a fixed window of W lights, independent of N | Task 1 (record-and-drop per light) | Task 2's batch-boundary window caps held layers; a fan-out retaining all partial layers re-unbounds it | AC 2, 3; Orderings 4 |
| 3 | Channel assignment runs once over the complete global membership, before any `data` byte is written | Task 2 (phase-2 barrier after the last batch) | Per-window/per-batch coloring, or a fill that starts before the barrier, omits cross-light edges and races the fill | AC 1; Orderings 1, 5 |

## Orderings

Concurrency scenarios the implementation must satisfy. Each row is concrete enough to
write a test from; the ACs reference them.

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| 1 | Two lights over one texel | A and B both cover texel T; charts complete in either order; fills run concurrently | Overlap graph carries edge A–B (built after both fully scattered); coloring gives distinct channels; bytes identical for either completion order |
| 2 | Reversed completion | Submit A, B, C; complete C, B, A | Membership keyed by fixed compact index (A=0, B=1, C=2), not completion; graph, drops, channels, bytes identical to submission-order run |
| 3 | `-j 1` with a window | `permits = 1`, `W = 4`, 5 lights | Stage completes, no deadlock; no work item holds a governor permit while blocked on window admission (batch-boundary window); advance reaches the published total |
| 4 | `W = 1` window | One light's charts in flight at a time | Byte-identical to serial and to `W = N`; resident `LightmapLayer`s ≤ one light's; membership still global; single assignment after the last light |
| 5 | Last partial batch | N = 5, W = 2 → batches {A,B}, {C,D}, {E}; A and E share texel T | Assignment runs once after E over all 5 lights' membership (not per-batch); A–E edge present ⇒ distinct channels at T |
| 6 | 0 selected valid lights | `light_indices` empty, or all out-of-range | Empty → `None`; all-filtered → empty section; both early-return **before** `publish_total`, so the stage stays indeterminate and completes instantly (no `publish_total(0)` / 0-of-0 render); no parallel pass; bytes == pre-change |
| 7 | Light dropped by >4 overlap | 5 lights share texel T | All 5 baked; each advances `placements.len()`; lowest-intensity dropped at coloring; dropped light not filled; total counts all 5; same mask dropped as today |
| 8 | Warm cache, all hits | 157 lights, every `"lightmap_layer"` a cache hit, no bake | advance still reaches published total; percentage not stuck at 0; section built from membership; bytes == cold path |

## Rough sketch

Bake — `crates/level-compiler/src/shadowmask_bake.rs`. The composite splits into:
(1) a streaming membership pass — per selected light, obtain its `LightmapLayer`
(cache-hit, or `bake_light_layer`, or the phase-1 work item in Task 2), scatter its
`raw_visibility >= 0.0` texels into the compact membership (texel global position +
fixed compact light index + full-precision `raw_visibility`), drop the layer; (2) once,
after every batch has scattered, the unchanged
`overlap_graph`/`assign_channels_with_drops` over the complete membership (Invariant 3);
(3) a fill pass
writing each light's visibility into its assigned channel of the `data` buffer, sourcing
visibility **from the membership** and quantizing once, identically to today — no layer
re-read. Resident bound: O(window) layers + membership + the one `data` output buffer.
The membership entry is roughly a third of the 48-byte `LayerTexel` (it drops
irradiance, weighted direction, and fallback normal), and is the atlas-size term this
plan does not further reduce.

Progress — the three coordinated pieces are in Task 1: `declare_progress` the stage's
`StageProgress` to the reporter (as the lightmap stage does at `begin_stage`);
`publish_total` from inside the bake after selection filtering (that site has
`shared.placements`); and advance to the full total on every path — per-chart on a miss,
`placements.len()` on a cache-hit or >4-dropped light, the full total on a whole-section
memo hit.

Parallelism — phase 1 is W-sized batches of `(light, chart)` items, each taking a single
`governor().enter()`; `join` each batch before the next, so the window is the join, never
a held primitive. No nested `enter()` inside `bake_light_layer` for this call path.
Phase 3 (fill) is separately parallelizable since its writes are disjoint by
construction.
