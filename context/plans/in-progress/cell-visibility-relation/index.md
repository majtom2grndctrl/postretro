# Cell-Visibility Relation (portal-graph graded foundation)

## Goal

A view-independent, baked Cell→Cell coupling relation derived from the portal graph the compiler
already builds: for cells A and B, a conservative gate `perceivable` (can the two cells couple at
all) plus graded axes `distance` (traversable path length) and `aperture` (tightest portal
constriction on the best path). Computed once at compile time by three cheap graph passes over
cells and portals, emitted as an optional PRL section, queried at runtime CellId-only through a
consumer-agnostic API. This is the shared substrate E15 network relevance, E12 audio occlusion,
E10 AI-perception broad-phase, projectile-VFX culling, and E17-F doors-as-occluders all consume —
built once so none of them reinvents it. The gate is conservative portal-reachability; the graded
axes carry the discrimination consumers actually use. A tighter *sightline* refinement is designed
for but deferred — additively, as a separate axis, never by redefining `perceivable`.

## Scope

### In scope

- A compile-time bake over the portal graph computing, for every cell pair: `perceivable`
  (portal-reachability — same connected component), `distance` (shortest portal-graph path length),
  and `aperture` (widest-path bottleneck over portal apertures).
- A new optional PRL section (`CellVisibility`, id 46) carrying a per-cell component id (the
  reachability gate) plus a sparse `distance` + `aperture` side-table on coupled pairs.
- Loader support with a conservative fallback (missing section → all cells mutually perceivable,
  no graded detail).
- A consumer-agnostic runtime query API over `CellId` only, returning
  `CouplingTuple { perceivable, distance, aperture }`.
- A `cell_visibility` observability dump: emit the baked relation for a loaded map as
  deterministic JSON through the headless runner.
- Property/invariant test coverage: conservative gate (equals portal-reachability), symmetric,
  distance/aperture graded-and-monotone, deterministic.
- The generalizability gate: API smell-test audit + the second-consumer paper-check
  (`research.md`).

### Out of scope

- The **sightline / line-of-sight refinement** — tightening the gate from portal-reachability
  toward true anti-penumbra visibility. Deferred to a later spec (`drafts/perf-anti-penumbra-pvs`
  holds the math). It lands as a *separate additive axis* consulted by consumers that want hard
  visibility (render-adjacent net/VFX culling), never as a redefinition of `perceivable` — so it
  cannot break audio, which needs around-corner coupling.
- Dynamic geometry — doors/movers/destructibles as dynamic portals, blocker masks, the widened
  `(a, b, portal-state)` query. Deferred to E17-F / the destructible epic; the section format and
  query struct are shaped to accept it additively.
- Any wired gameplay consumer (network relevance, audio, AI broad-phase). Proven here by tests +
  observability + paper-check; the real consumers attach in their own epics.
- Runtime recompute of the relation. The relation is a pure function of static geometry.
- Mod-facing / scripting surface. The substrate is engine-internal; no FGD or SDK types.

## Direction

**Problem.** The engine has no view-independent, area-source coupling relation. The only visibility
is per-frame, point-source portal traversal (`crates/visibility/src/portal_vis.rs`), which answers
"what does this camera see now" and structurally cannot answer "how strongly can any observer in A
couple to B" — the question network relevance, audio PAS, AI perception, and door occlusion all ask.
E17-F and three other epics are each written to consume a substrate that does not exist.
Foundation-first builds it once, consumer-agnostic, so those consumers stop being blocked on it.

**The central bet — exploit the portal graph, grade rather than gate.** The compiler already
generates the portal graph (`generate_portals`); a portal exists only between two non-solid leaves,
so solid geometry is portal absence and connectivity is fully carried by the graph. Three cheap
graph passes over it — connected components (reachability), all-pairs shortest path (distance),
all-pairs widest-path bottleneck (aperture) — yield the whole relation in `O(V·E log V)` with no
raycasting, no separating-plane geometry, and no float-heavy polygon clipping. The discrimination
consumers need is carried by the **graded** axes: a pair that is reachable but weakly connected
(long path, tiny bottleneck) self-suppresses under any consumer's distance/aperture weighting,
without a hard sightline gate. So the gate stays a cheap conservative superset (reachability) and
the graded axes do the work. This is the substrate doc's `{ perceivable, distance, aperture }` tuple
with the emphasis moved from an expensive binary axis to the two graded ones.

**Prior commitments.** This formalizes v1 of `context/research/cell-visibility-substrate.md`
(design intent). It honors that doc's architectural contract — CellId-only vocabulary, neutral
crate, the `{ perceivable, distance, aperture }` tuple as the minimal orthogonal basis, conservatism
and symmetry, the optional-section fallback, and additive seams for the sightline refinement and
dynamic masks. It aligns with **Baked over computed** (`context/lib/index.md` §2): the relation is a
pure function of static geometry, so it bakes. It relies on the tree-wide `CellId == BSP leaf index`
identity (`pack::encode_cells`) and on portalization being complete (`generate_portals`). It is the
prerequisite E17-F's visibility half builds its dynamic blocker-mask layer on.

Three witting divergences from the substrate doc. Two are build-guidance clauses overridden by the
owner — building ahead of the doc's "measured consumer need" trigger, and proving via
tests+observability rather than a wired gameplay consumer (argued in `research.md` §Owner-approved
divergences); these keep the doc's architectural clause intact. The third is architectural: the doc's
invariant #1 mandates `perceivable` be a *sightline* PVS and explicitly rejects mere reachability;
this spec **redefines `perceivable` = portal-reachability**, keeping the doc's conservatism and
symmetry but overriding its sightline definition. The pivot is argued on the merits under **The
central bet** and **Alternatives rejected (2)**: reachability is the correct conservative gate for
these non-camera consumers (a sightline gate is actively wrong for audio, which needs around-corner
coupling), and the graded axes carry the discrimination the sightline bit was meant to provide.

**Alternatives rejected.**
(1) *Let the first consumer build the relation inline and extract later.* Rejected — the substrate
doc's central failure mode: a relation built inside one consumer absorbs that consumer's policy and
every other consumer forks it.
(2) *Sightline PVS (anti-penumbra separating-plane flood) as the v1 gate, aperture deferred.* This
was an earlier shape of this very plan; rejected on reflection. Three reasons. **(a) Wrong axis for
the consumers.** This substrate is not a render cull (the runtime portal flood already owns that);
its consumers — net relevance, audio, AI broad-phase — grade by distance and aperture. A hard
line-of-sight bit is the *least* useful and *most* expensive thing to compute for them, and for
audio it is actively wrong: sound propagates around corners, so a sightline gate would wrongly
decouple audible pairs. **(b) The rebuttal to reachability weakens once aperture is in v1.**
Reachability-only "barely culls in a connected level" — true when the only output is the binary gate,
false once distance and aperture grade the reachable set: the weak connections score low coupling
and self-suppress. **(c) Cost and fragility.** The anti-penumbra flood is a CPU-software-rasterizer-
era technique (separating planes, Sutherland–Hodgman, epsilon slivers) chosen to make a 1996 bake
tractable. The graph passes are cheaper, simpler, symmetric-by-construction (the portal graph is
undirected), and deterministic without float-clipping care. The sightline tightening is retained as
a deferred *additive axis* (`research.md` §Deferred sightline axis) for when a measured consumer
shows the graded axes do not cull hard enough — which is exactly where the substrate doc originally
placed it, before the earlier shape pulled it into v1.
(3) *Compute the relation on demand at runtime against the BVH, cache it.* Rejected for v1 —
authoritative co-op wants a deterministic, identical-across-machines relation (byte-identical bake),
the relation is queried per-entity-per-client-per-tick (precompute wins the hot path), and static
geometry means nothing to invalidate. Runtime-compute becomes attractive exactly when dynamic
occlusion lands, which is the deferred dynamic-mask seam, not v1.
(4) *Defer the standalone substrate; build the reachability gate in one run with its first consumer
(E15 Phase 4 network relevance), consumer-agnostic from the start (substrate task → consumer task),
gated on the measured bandwidth pressure the roadmap names.* This is the roadmap's own prescribed
shape for this substrate, and — unlike (1) — it avoids policy absorption by keeping the substrate task
consumer-neutral. It is **not** rejected on technical merits; it is a sound alternative. The owner
chose foundation-first instead — build the substrate now, proven by tests + observability, so all five
committed consumers attach without any one of them owning or gating it — accepting that no consumer's
measured-need trigger has yet fired. That is the load-bearing timing decision (see **Prior
commitments**, divergences 1–2); this spec's direction stands or falls with it.

The cost this direction accepts: **storage, not bake time.** Reachability is a looser gate than
sightline, so in a connected level most pairs are perceivable and carry graded values — the graded
side-table can approach N². v1 bounds it with **two** structural caps: a generous coupling *distance*
cap (a coarse pre-filter on path length) and a per-source *fanout* cap that keeps only each cell's `K`
nearest coupled partners — a hard `O(N·K)` stored-count bound and an `O(P·(N+E) + N·K)` parallel
bake working-set bound with every symbol bounded: `E` (metric-graph edge count) is linear in portal
count by the hub construction and `P` (workers concurrently executing per-source passes) is at most
`available_parallelism()` — AC11 defines both — and the reduction is applied per source, so the full
N² distance matrix is never materialized. At the design-target ceiling (`N ≈ 250k` leaves,
Ironwail/Strata-class, portals a small multiple of `N`) a per-source workset is tens of MB, so `P`
worksets plus the retained `N·K` rows total a few hundred MB on a many-core host — the
hardware-proportional bound (the scoped pool caps `P` at `available_parallelism()`) suffices without a
separate memory-derived worker cap, and the TUI permit dial gives the operator a downward override. The
distance cap alone gives neither, because distance bounds path
*length*, not pair *count* (a spatially compact but densely-connected region keeps ~N² pairs within any
distance cap). The caps' exact magnitudes tune against real content, but their *shape* is decided: the
linear `N·K` bound holds the side-table to ~128 MB at the design-target ceiling regardless of density,
so affordability is a bounded guarantee, not a bet. What stays empirical until real maps is a *quality*
question — whether the graded axes cull hard enough — taken with eyes open because the direction is a
two-way door: backing out deletes
an optional section, a `LevelWorld` field, and four accessors (the two `LevelWorld` query accessors
plus the two dump-facing accessors on `CellVisibility`), with zero consumer churn.

## Acceptance criteria

- [ ] AC1 — `prl-build` on a fixture map emits a `CellVisibility` section (id 46). A map with no
  such section (old, or lacking the section) loads and runs; the query returns "all perceivable" for every
  pair with `distance`/`aperture` `None` (conservative fallback), no error, no panic.
- [ ] AC2 — `perceivable(a, b)` is defined for every ordered pair in `0..cell_count`; it is
  symmetric; the diagonal `perceivable(a,a)` is true. It equals portal-graph reachability (a and b
  in the same connected component of the portal graph), checked against an independent BFS oracle.
- [ ] AC3 — (Design rationale; verified via AC2, no independent test.) Conservative gate:
  `perceivable` never omits a pair that any real coupling could exist between. This holds by
  construction — a sightline or audible path needs a portal path, and reachability is exactly the
  portal-path relation — and is verified by AC2's equality to the BFS reachable set (reachable ⊇
  visible ⊇ ∅, zero false negatives).
- [ ] AC4 — With a section loaded, `distance(a, b)` is `Some` exactly on perceivable off-diagonal
  pairs that are stored (`distance <= cap` in the same fixed-point domain as the stored value, AND the
  pair is among either endpoint's `K` nearest coupled partners — the per-source fanout cap) and `None`
  otherwise (diagonal, non-perceivable, beyond the distance cap i.e. `distance > cap`, beyond the
  fanout cap, or a pair touching — or whose every metric path crosses — a faceless invalid-bounds
  cell, which contributes no hub node — all coupled-but-no-graded);
  symmetric; equals the shortest portal-graph path length under the pinned edge metric (Task-1
  fixed-point scale); deterministic across recompiles.
- [ ] AC5 — `aperture(a, b)` is `Some` on the same coupled pairs as `distance` and `None` otherwise;
  symmetric; equals the widest-path bottleneck — the maximum over all paths of the minimum portal
  aperture on the path — under the pinned aperture metric; monotone (narrowing any portal on the
  sole best path cannot raise the stored aperture); deterministic across recompiles. Over-range graded
  **values** (a `distance` or `aperture` exceeding its fixed-point representable maximum) clamp-with-
  warning at the maximum, never wrap — distinct from over-range **counts/sizes** (pair count, output
  length, allocation capacity), which abort the bake (AC11): values clamp, counts abort.
- [ ] AC6 — The query returns `CouplingTuple { perceivable, distance, aperture }`; under the
  fallback it returns `{ true, None, None }` for every off-diagonal pair. Adding the deferred
  sightline axis later is additive (a new field / section-version bump), not a change to these three
  fields (no consumer signature churn on the existing axes).
- [ ] AC7 — `xtask observe` / `--headless` with a `cell_visibility` dump emits the relation as JSON:
  the `u32[cell_count]` component-id array (the reachability gate, inspectable independent of the
  coupled-pair list) alongside one entry per unordered off-diagonal pair with `cell_a < cell_b` that is
  coupled and stored (perceivable, within the distance cap, and within either endpoint's fanout — the
  informative set; diagonal omitted), sorted ascending by
  `(cell_a, cell_b)`; `distance`/`aperture` serialize as integers — a stored pair always carries both
  graded values (AC5: `aperture` is `Some` on exactly the pairs `distance` is), so `null` never appears
  in the pair array in v1 (the JSON stays nullable for forward-compat). Two identical runs are
  byte-identical.
- [ ] AC8 — (Review/grep gate, not a runnable test.) The query API names only cell types (`CellId`,
  cell-count). No `Player`, `Entity`, `ClientId`, `Sound`, `Projectile`, or relevance/audible/
  cull-distance parameter appears in the bake or query signature; `crates/level-loader` (the query's
  home) gains no dependency on net/audio/gameplay crates. Verified by signature grep + a `Cargo.toml`
  diff.
- [ ] AC9 — (Review gate, already satisfied in `research.md` §Generalizability paper-check.) The
  second-consumer paper-check — network relevance (gate on `perceivable`, prioritize by `distance`)
  and audio PAS (gate on `perceivable`, attenuate by `distance` and `aperture`), both expressible
  via the CellId query plus consumer-side policy — is recorded before merge.
- [ ] AC10 — Two compiles of the same fixture produce byte-identical `CellVisibility` section bytes
  (bake is deterministic; no HashSet/HashMap iteration order leaks into output; no wall-clock value
  branches the bake; float-derived weights resolved through a pinned tie-break and fixed-point scale).
  The bytes are also invariant to the governor permit count: two compiles at different core budgets
  (e.g. 1 vs `available_parallelism()`) produce identical output — the parallel per-source map uses an
  order-preserving collect and the side-table is sorted before serialization (pin P1).
- [ ] AC11 — Two structural caps bound the graded side-table to a hard `O(N·K)` entry count.
  (a) The coupling **distance cap**: a directed candidate `(s→t)` with `D(s→t) > cap` is dropped.
  (b) The per-source **fanout cap** `K`: among its in-cap partners (`D(s→t) <= cap`), each cell keeps only
  its `K` nearest by `(D(s→t), partner id)`, reduced per source inside the Dijkstra pass; a pair is
  stored iff it is among *either* endpoint's `K` nearest (union — preserving symmetry), with the stored
  value the kept direction's (min→max tie-break when both keep it) so it always passed a per-source cap
  — membership, cap, and stored value are the same number, no pair stored with `distance > cap`. `K`
  caps each source's directed *selection* (its own `K` outgoing choices), which bounds the total at
  `<= N·K` entries regardless of density; it does **not** bound a cell's final stored *degree* — a cell
  that many others rank among their `K` nearest appears in more than `K` stored pairs, which is the
  intended union semantics, not a violation. The bake runs sources in parallel under the shared
  `Governor` (TUI-adjustable core count, like the other bakes): peak relation working memory is
  `O(P·(N+E) + N·K)` — the shared read-only metric graph (`O(N+E)`), `P` concurrent Dijkstra worksets
  (each `O(N+E)`: a node-distance array plus a frontier heap bounded by one entry per edge
  relaxation), and the retained top-`K` rows — never an N² matrix. Both symbols are defined and
  bounded. `E` is the metric graph's edge count: the graph joins each portal-holding cell's hub node
  to each of its incident portal centroids (Task 2 pins the metric), and a portal joins exactly two
  cells, so `E = 2·portal_count` — linear in portal count — with node count `<= N + portal_count`
  (`N` = `cell_count` = leaf count, the `CellId == BSP leaf index` identity, so the `<= N·K` allocation
  sizes against the u32 `cell_count` wire field).
  `P` is the number of workers concurrently executing per-source passes: `min(governor permits,
  scoped-pool width)`, and the bake bounds it by **construction** — it runs its per-source `par_iter`
  inside a scoped Rayon `ThreadPool` sized to `available_parallelism()` (width `=
  available_parallelism().map(NonZeroUsize::get).unwrap_or(1)` — the same unwrap the TUI worker uses;
  the error path falls back to 1, which only tightens the bound), entered via `pool.install(...)`, so
  `P <= available_parallelism()` is guaranteed regardless of the ambient global-pool size
  (`RAYON_NUM_THREADS`, an external `build_global`, or a caller's surrounding pool never widen it). A
  failed `ThreadPoolBuilder::build()` **aborts the bake with an error — it never falls back to
  `into_par_iter()` on the ambient global pool**, since that fallback would surrender the very ceiling
  the memory bound rests on. This scoped-pool-plus-shared-`Governor` combination is a new production
  pattern with no compiler precedent (the SH/lightmap bakes run on the global pool; only test code uses
  a scoped pool today), so it is validated end-to-end — including the AC10 determinism assertion run at
  both `permits = 1` and `permits = available_parallelism()` — not by analogy. The governor throttles
  *within* that ceiling: the TUI clamps permits to `available_parallelism()` and lowers `P` live,
  `BakeControl::unrestricted()` (permits effectively unbounded) leaves the scoped-pool width as the
  binding bound, and a permit only ever admits a worker the scoped pool already supplies, never adds
  one. The per-source Dijkstra pass acquires **no nested `enter()` permit and spawns no nested
  `par_iter`**, so no permitted item ever waits on another (the `Governor`'s sole liveness rule) and
  the interaction cannot deadlock even at pool width 1. Output is byte-identical
  regardless of `P` (order-preserving collect + the `(cell_a,cell_b)` sort — pin P1). Pair-count
  conversion to the u32 wire field, output-length arithmetic, and allocation
  capacity arithmetic are checked; a value that does not fit aborts the bake with an error, never truncates
  or wraps. Test the count-conversion boundary without allocating `u32::MAX + 1` records. (The distance cap
  bounds path length, not pair count; the fanout cap bounds count and bake memory.) Omitted pairs stay perceivable; `distance`/`aperture` read
  `None` (coupled-but-no-graded). Both caps are deterministic given their pinned values. On a small fixture left effectively uncapped (distance cap
  high, `K >= cell_count`) the table holds every reachable off-diagonal pair. The omission boundaries
  are unit-tested via cap/fanout parameters on the side-table-assembly function: a small distance cap
  exercises `distance == cap` stored / `distance == cap+1` omitted (pin P7); a small `K` over a
  hand-built graph exercises the top-`K`-by-distance selection and the union-store symmetry (pin P10).
  The shipped `cell_visibility_bake` stage binds both parameters to their `pub const`s; only unit tests
  pass other values, so no stray value leaks into the shipped bytes (P9).
- [ ] AC12 — The bake stage logs its duration through the Build Summary path, so the cost is visible
  on the first real-map compile. Two-part wiring gate: (a) the stage-contract test confirms
  `CellVisibility` is registered in `ORDERED_STAGES` with its label — but that test operates only on
  the static `planned_stages_for_sdf(...)` list and cannot prove the executed bake is bracketed; so (b)
  the `begin_stage`/`finish_stage` bracketing of the actual bake call in `run_after_parsing` is a
  code-review/grep fact (like AC8's grep gate), verified by reading the stage body, not inferred from
  the stage-contract test.

## Tasks

### Task 1: Thin slice — section format, reachability gate, loader, query API

Stand the full pipe end to end with the real reachability gate and an empty graded side-table, to
falsify the boundary assumptions before the graded passes land. Add `SectionId::CellVisibility = 46`
and its `from_u32` arm (`crates/level-format/src/lib.rs`; do not reuse the retired id 14). Author the
section module (mirror `cell_locator.rs`): a `CellVisibilitySection` with
`CELL_VISIBILITY_VERSION: u32 = 1`, a fallible `to_bytes`/`from_bytes` pair (bounds-checked, rejects truncation,
u32 pair-count overflow, output-size overflow, and trailing bytes), carrying the complete v1 wire layout from the start so later work fills data into a
fixed format, never re-cuts it. Wire constraints (task agents do not receive the Wire-format section
— restated here): little-endian, u32 counts (mirror `cell_locator.rs`); a leading `version: u32`
(`CELL_VISIBILITY_VERSION`, rejected on mismatch) then `cell_count: u32` (must equal the map's cell
count; reject `0`); a per-cell **component id** array (`u32[cell_count]` — the reachability gate,
`perceivable(a,b) = component[a] == component[b]`); and a count-prefixed, ascending-sorted
`(cell_a, cell_b, distance, aperture)` graded side-table with `cell_a < cell_b`, present only on
coupled pairs (empty list encodes as count `0`). Pin the `distance` and `aperture` fixed-point
scales, the coupling `distance` cap, and the **fanout cap** `K` (each source's max top-`K` directed
selection — the hard count bound, total `<= N·K`, not a per-cell final-degree bound), as named
`pub const`s in the section module, so Task 2 reads them from source. Pin `CELL_VISIBILITY_FANOUT_K:
usize = 32` — a memory-budget-derived default, not a measured one: at 16 B/entry and the design-target
ceiling `N ≈ 250k` leaves (Ironwail/Strata-class content), `N·K` tops out at ~128 MB, scaling linearly
with `N` beyond that; `K` is a retunable dial (raise it for graded-quality margin at proportional cost,
lower it to shrink the table) whose load-bearing guarantee is the linear `N·K` bound, not the specific
value (scales' fractional bits + representable maxima are likewise fixed once from the map world-bound
budget). Pin the distance cap in the same domain as the stored `distance` (fixed-point `u32`
counts, at the Task-1 scale), and pin the boundary operator: `distance <= cap` → stored; `distance >
cap` → omitted (`perceivable` stays true, graded `None`). Pin `K` so the side-table stays `<= N·K` and
the parallel bake working set stays `O(P·(N+E) + N·K)` (`P` and `E` as defined in AC11) regardless of
density:
each cell keeps its `K` nearest coupled partners by `distance` (reduced per source inside the Dijkstra
pass — the full N² matrix is never materialized), and a pair is stored if *either* endpoint keeps it
(union — preserving symmetry). Add a bake stage `cell_visibility_bake` (name the stage function so Task 2 can locate it) after
`BvhBuild` in `crates/level-compiler/src/pipeline.rs`. `cell_draw_index_bake`
is the template only for the bytes-held-and-handed-into-`pack_and_write_portals` pattern: consuming
`result.tree` (or `vis_result.leaves_section`), `generated_portals`, and `exterior_leaves`, and
emitting pre-serialized bytes held and handed into `pack_and_write_portals` at the later Packing stage
as a new optional-section argument — the inputs stay in immutable scope across the intervening bake
stages, they do not run adjacent. Unlike `cell_draw_index_bake` (which runs unbracketed, with no
`StageId` and no `ORDERED_STAGES` entry), the new bake must be a registered timed stage: follow a
genuinely bracketed stage such as `StageId::NavMesh` / `StageId::BvhBuild` for the
`begin_stage`/`finish_stage` + `ORDERED_STAGES` registration half. The stage runs after `BvhBuild`
(matching pin P6 and where `cell_draw_index_bake` actually sits); its inputs have been live since
`encode_vis`, so the ordering is by convenience, not data dependency. Registering the
bake as a named pipeline stage adds `StageId::CellVisibility` to `ORDERED_STAGES` and its
`label`/`progress_label` arms, and updates the pinned
`planned_stage_contract_pins_order_labels_and_sdf_prediction` test: the `const ORDERED_STAGES:
[StageId; 22]` type annotation (22→23), both `assert_eq!(...len(), 22)` sites (22→23), both `[19]`
ordinal assertions (`without_sdf[19]` and `with_sdf[19]`, each → `[20]`), and the label-vector insert
between `"BVH Build"` and `"NavMesh"`. The new stage needs no new `predicted_present` arm — it inherits
`predicted_present = true` via the existing `id != SdfAtlasBake || needs_sdf` logic. Bracket the bake with
`begin_stage`/`finish_stage` so its duration prints in the Build Summary (AC12) — Task 2's graded
passes inherit the same bracketed stage. Thread the pipeline's `Arc<Governor>` into the stage as a
`BakeControl` (`BakeControl::new(Arc::clone(&governor), &progress)`, exactly as the SH/lightmap stages
do) and hand it to the side-table-assembly function, so Task 2's parallel per-source Dijkstra runs
under the shared governor — the operator raises/lowers this bake's core count live from the TUI like
every other bake, and it reports progress via `publish_total`/`advance`. Task 1's own component pass is
`O(cells + portals)` single-pass and needs no parallelism; it only stands up the `BakeControl` seam
Task 2 fills.
In this task the bake fills the **component ids** (connected
components of the portal graph — union-find or BFS over portal adjacency, treating solid leaves as
barriers). Reuse `find_exterior_leaves`'s portal-adjacency construction and solid-barrier handling, but
the outer loop over all unvisited cells and the dense component-id assignment are net-new —
`find_exterior_leaves` is single-seed BFS from an exterior probe, not an all-cells components labeler.
Assign component ids deterministically: representative = lowest member cell id, ids dense from 0 in
ascending representative order. The graded side-table is left empty here. Wire the loader: in
`crates/level-loader/src/prl_loader.rs`, read the optional section and lower it via the `convert_*`
path; in `crates/level-loader/src/prl.rs`, add the `LevelWorld` struct field
`cell_visibility: Option<CellVisibility>` (`None` means the conservative fallback). Expose the query by
adding `perceivable`/`coupling` accessors to the `impl LevelWorld` block in `prl.rs`, beside the
existing `locate_cell` / `cell_count` CellId-query precedent, CellId-only: `perceivable(a, b) -> bool`
(fallback all-true; reads the component array) and `coupling(a, b) -> CouplingTuple`. Implement
`coupling` as a real side-table lookup now — it returns `{ perceivable, distance: None, aperture: None }`
in this task only because the side-table is empty, not by hardcoding `None`; Task 2 populates the table
and the same accessor then returns `Some` with no further loader edit. Also expose, on the runtime
`CellVisibility` type, a component-id slice accessor and a coupled-pairs iterator, both reachable
cross-crate from `crates/postretro` (pub or pub accessor), so Task 3's dump reads the component
partition and the coupled set directly rather than recomputing labels from pairwise `perceivable()`.
`cell_count` comes from `LevelWorld::cell_count()`. Tests: bake→load→query round-trip on a compiled fixture; a
missing-section fallback test; component equality vs. an independent BFS reachable-set oracle (AC2);
the component-id slice accessor length equals `cell_count` and the coupled-pairs iterator is empty on
the placeholder (component-only) section; two compiles of the fixture produce byte-identical
`CellVisibility` section bytes for the component-only section (AC10 — Task 2 extends this once the
graded side-table is present).
Plumbing: the new pack argument is threaded from the bake stage through `pack_and_write_portals`; the
`LevelWorld` field is added in `prl.rs` and populated in the `Ok(LevelWorld { … })` construction inside
`load_prl` (`prl_loader.rs`), alongside the other `convert_*`-lowered sections (there is no
`LevelWorld::new`; `new_visibility_only` is a partial constructor off the load path, in `prl.rs` — set
the new field to `None` there, and in any other `LevelWorld { … }` literal the compiler flags).

### Task 2: Graded axes — distance and aperture over the portal graph

Fill the graded side-table with the two coupling axes, both cheap graph passes over the portal graph,
into the format Task 1 froze. **Distance:** all-pairs shortest path via per-source-cell Dijkstra over
the hub metric graph, keyed by ordered pair `(min, max)`, stored on coupled off-diagonal pairs. Pin
the edge metric explicitly so nobody later "fixes" it as a bug: the graph holds one node per portal
(its polygon centroid) and one **hub node** per portal-holding cell (the cell-center); the only edges
join a cell's hub to each of its incident portal centroids, weighted by the Euclidean distance between
the two points — every path alternates hub→portal→hub and `D(s→t)` is the shortest hub-to-hub path.
A portal joins exactly two cells, so the graph has exactly two edges per portal: `E = 2·portal_count`
and node count `<= N + portal_count` — the linear bounds AC11's working-set budget rests on. The
hub-routed length **is** the definition of the distance key — a path-length coupling-quality key, not
a discretization of some truer portal-to-portal chord it approximates. There is no chord metric it
owes fidelity to: in-cell transit routes hub→portal→hub by definition, and the resulting value is the
one all consumers grade against. (This is why the edge metric is pinned so nobody later "corrects" it
toward chords and reintroduces the quadratic clique.) The value is monotone and symmetric, which is all
the graded axis requires. Cell-center is
`BspLeaf.bounds.centroid()` — `result.tree` is
threaded into the bake — guarded by `Aabb::is_valid()`: a faceless leaf with invalid bounds — the
empty sentinel (`min=+INF, max=-INF`, so `centroid()` is NaN) or any non-finite bound (`is_valid()`
rejects `min > max` AND any non-finite component) — contributes no hub node, mirroring how
`find_exterior_leaves` skips faceless leaves. Solid and exterior cells both carry *valid* bounds — the
empty sentinel is not the solid/exterior marker; solids are absent from the metric graph
anyway by portal absence (`generate_portals` emits only between two non-solid leaves), and exterior
cells are non-solid and participate normally through their portals. The only excluded participant is a
faceless invalid-bounds non-solid leaf that still holds a portal: it — and any pair whose every metric
path crosses it, since its portals have no hub to route through — reads coupled-but-no-graded, see
AC4/AC5). Populate
distance only for pairs the component array marks perceivable — the cell-adjacency components gate the
side-table; the metric graph's connectivity agrees with cell-component membership for every
valid-bounds cell by construction: every non-solid cell that participates in coupling touches at least
one portal and so contributes a hub, and a non-solid zero-portal cell is its own singleton in both
graphs (like a solid cell). The one divergence is the faceless invalid-bounds cell above (no hub): a
same-component target the per-source pass never reaches is simply not stored — identical handling to a
beyond-cap pair (`perceivable` stays true, graded `None`), never an error. **Aperture:** the
widest-path (maximin) bottleneck — the largest, over all paths, of the smallest portal aperture on
the path. Pin the per-portal aperture metric (the portal polygon's minimum width, or its area — the
implementer picks one and documents it; it is a coupling-quality key, not a solid angle). Compute
all-pairs bottleneck from a maximum spanning tree of the cell-adjacency portal graph (cells = nodes,
portals = edges) weighted by aperture (the bottleneck between two cells is the minimum aperture on the
unique tree path). Both axes are
symmetric in exact arithmetic — the portal graph is undirected, so `dist(a→b) == dist(b→a)` and the
bottleneck is symmetric; no directed max/min *merge* of the two path values is needed for correctness.
(The stored fixed-point `distance` still picks a deterministic direction where the two directions round
apart — P8 — and fanout *membership* still unions both endpoints' keep-sets — P10; those are
determinism and count mechanisms, not a symmetry repair.) The algorithm operates purely on the
portal graph — no BVH / brush raycast, because solid geometry is already portal absence
(`generate_portals` only emits portals between two non-solid leaves). Solid cells (no portals) are
their own singleton component (perceivable only on their own diagonal, no graded entries). Store both
at the Task-1-pinned fixed-point scales — read the named `pub const`s from the section module (Task 2
receives neither Task 1's paragraph nor the Wire-format section); assert every value fits its
representable range and clamp-with-warning at the maximum rather than wrapping.

Storage is bounded (AC11) by two structural caps applied in order. First the **distance cap**: a pair
whose `distance` exceeds the pinned cap is dropped — a coarse pre-filter on path length. But the
distance cap does not bound *count*: a spatially compact, densely-connected component keeps ~N² pairs
within any cap, so the distance cap alone can still blow the side-table on a large map. So second, the
**per-source fanout cap** `K`: for each source cell, among its in-cap coupled partners (`D(s→t) <= cap`),
keep only the `K` nearest by `(D(s→t), partner cell id)` — per source, reduced inside the Dijkstra pass
(pin P10) — then store an unordered pair iff it survives in *either* endpoint's kept set. The stored
value is the kept direction's (min→max when both keep it), so it always passed a per-source cap: the
cap filter and the stored value are the same number, and no pair is stored with distance `> cap`. This
keep-set union keeps the relation symmetric (each stored pair is one `(min,max)` entry, reachable from
both ends), bounds the emitted table at `<= N·K`, and keeps the parallel bake working set at
`O(P·(N+E) + N·K)` (`P` and `E` as defined in AC11) — the full N² distance matrix is never
materialized (each source reduces to its top-`K` as it finishes), so a spatially compact,
densely-connected map neither blows the side-table nor exhausts memory during the bake. A dropped pair (beyond the distance cap, or beyond both endpoints' fanout) stays perceivable
(same component) and reads `distance`/`aperture` `None` — coupled-but-beyond-the-stored-horizon. Both
caps are structural bounds set generously (beyond any plausible consumer's range), not consumer policy.
On the small fixtures leave both effectively uncapped (distance cap high, `K >= cell_count`) so tests
see the full reachable set. `aperture` is stored on exactly the pairs `distance` is (the fanout ranks by
`distance`; both axes ride the same surviving set, aperture queried from the max-spanning tree).

Determinism (AC10): build adjacency as `Vec`s in a fixed portal order; key the Dijkstra frontier on
`(cost, node_id)` — `node_id` is a single injective index over both frontier node kinds (portal nodes
and hub nodes get disjoint id ranges, e.g. portals `0..portal_count`, hubs `portal_count..`), so
`(cost, node_id)` is a genuine total order: two distinct nodes at equal float
cost can never share an id, so float last-ULP accumulation differences cannot flip the stored value;
break equal-aperture ties in the maximum spanning tree by portal index; no HashMap/HashSet iteration
order feeds component-id assignment, relaxation, tree order, side-table pair collection, or
serialization (see Determinism pins P2/P3) — component ids are assigned by an explicit sort of distinct
representatives ascending (not first-seen HashMap iteration), so "dense from 0 in ascending
representative order" is a pinned procedure, not only a pinned result. Assemble the side-table with
**bounded peak memory** — never materialize the full N² distance matrix. (1) Run per-source Dijkstra,
but reduce each source `s`'s result to that source's top-`K` in-cap partners *inside the pass* —
`topK(s)` = the `K` nearest `t` with `D(s→t) <= cap`, ranked by `(D(s→t), partner_id)` (pin P10) —
where `D(s→t)` is resolved to its stored fixed-point value *once*, and that single value feeds the
rank key, the `<= cap` compare, AND storage alike (pin P12), so ranking never uses a float that
disagrees with the stored integer (same-binary determinism, per AC10) —
retaining only `(t, D(s→t))` per kept partner (`O(K)` per source, `O(N·K)` total; the full `O(N+E)`
per-source workset — node-distance array and frontier — is discarded once reduced). (2) Keep an unordered pair `{s,t}` iff `t ∈ topK(s)` or
`s ∈ topK(t)` (the keep-set union — symmetric). (3) Store each kept pair `{a,b}`, `a < b`, once as
`(min,max)`; its value is the kept direction's — `D(a→b)` if `b ∈ topK(a)`, else `D(b→a)`, and `D(a→b)`
(min→max) when both keep it (a deterministic tie-break, pin P8). A mutually-kept pair surfaces as two
directed candidates `(a→b)` and `(b→a)`; collapse them to one `(min,max)` row by **sorting the directed
candidates by `(min,max)` and collapsing each equal-key run to a single entry** (taking the P8 value on
a two-direction run) — a HashSet-free dedup that honors the no-HashSet-in-pair-collection pin (pin
P13), so a forgotten collapse cannot silently ship duplicate `(a,b)` rows that pass AC10 yet violate
AC7/AC11's one-entry-per-pair. Both directions are individually
deterministic (pin P2) and each passed its per-source cap, so the stored value is deterministic and
always `<= cap` — the cap filter and the stored value are the same number by construction, never a
completion-order or last-write-wins dedup. `aperture` rides the same kept set, queried from the
max-spanning tree (`O(V)`, not per-source). Emit kept pairs sorted by `(cell_a,cell_b)` (pin P4). Run
the per-source Dijkstra passes in parallel like the other bakes, but inside a **scoped Rayon
`ThreadPool`** the stage builds at `available_parallelism()` width and enters with `pool.install(...)`,
so the pool ceiling is the bake's own, never the ambient global pool
(`RAYON_NUM_THREADS`/`build_global` cannot widen it — the AC11 memory bound); a failed `build()` aborts
the bake, never falls back to the global pool (AC11):
`pool.install(|| sources.into_par_iter().map(|s| { let _permit = control.governor().enter(); …reduce
to topK(s)…; control.advance(1); topK(s) }).collect())` with an **order-preserving collect**
(index-aligned to source id — the `sh_bake` pattern), so the number of workers never affects the
output: the keep-set union and the `(cell_a,cell_b)` sort canonicalize regardless of completion order
or permit count (pin P1). `sources` is the range `0..cell_count`, so the collect index *is* the source
cell id (a faceless source yields an empty `topK`); if the source list is instead pre-filtered to skip
faceless cells, each result must carry its source cell id and be remapped before the union/sort, never
left aligned to a filtered subset (pin P15). Each
admitted source holds one Dijkstra workset (`O(N+E)`: a node-distance array plus a frontier heap
bounded by one entry per edge relaxation) and never waits on another admitted item (sources are
independent — satisfies the governor's no-permit-waits-on-permit rule), so peak memory is the shared
read-only graph plus `P` worksets plus the retained rows: `O(P·(N+E) + N·K)` (`P` and `E` as defined
in AC11).
Concurrency is the shared `Governor`, reached through a `&BakeControl` the stage builds from the
pipeline's `Arc<Governor>` exactly as the SH/lightmap stages do, so the operator adjusts the
cell-visibility bake's core count live from the TUI (the same +/- permit control, clamped to
`available_parallelism()`), throttling *within* the scoped-pool ceiling. `BakeControl::unrestricted()`
off the TUI path (tests, `--no-tui`) leaves permits effectively unbounded, so there the scoped-pool
width binds `P` — because the pool is sized to `available_parallelism()` by construction (not the
ambient global pool), `P <= available_parallelism()` holds on every path regardless of
`RAYON_NUM_THREADS` or an external `build_global`, so the AC11 memory bound is guaranteed, not
merely default-config-true.
Task 1's `coupling` accessor already
reads the side-table, so filling it here makes the accessor return `Some` on coupled pairs with no
`crates/level-loader` edit — Task 2's changes stay in `crates/level-compiler` (the `cell_visibility_bake`
stage Task 1 created). The side-table-assembly function takes both the coupling cap and the fanout `K`
as parameters (the `pub const`s are the production defaults), so unit tests can drive a small cap
(AC11 / pin P7) or a small `K` (pin P10) to exercise both omission boundaries without a fixture that
crosses the generous production values. Tests:
`distance`/`aperture` present exactly
on stored coupled off-diagonal pairs and absent on diagonal/non-perceivable/beyond-cap/beyond-fanout
pairs; both symmetric; `distance` matches an independent shortest-path oracle and `aperture` an
independent bottleneck oracle on a fixture (same-metric oracles validate the relaxation/tree and
determinism, not the metric choice, which is a pinned design decision). The independent oracle
validates *values* only; fanout-membership ("stored exactly on within-cap ∩ within-either-endpoint's-
`K`-nearest pairs") is checked by the uncapped `K >= cell_count` property fixtures (where it reduces to
the BFS reachable set) and the hand-built small-`K` unit test — never by an oracle that re-derives the
top-`K` union, which would test the impl against a copy of itself. A small-cap unit test confirms
`distance == cap` stored / `distance == cap+1` omitted (perceivable stays true); a small-`K` unit test
confirms each source *selects* at most its `K` nearest partners (the directed cap) and the table holds
`<= N·K` entries, and — over a hand-built graph with one cell many others rank in their top-`K` —
that the keep-set union is symmetric (a pair kept by one endpoint is present for both) and that a
cell's final stored *degree* is allowed to exceed `K` (the union of others' selections), so the test
asserts the `<= N·K` total and the per-source `<= K`, never a per-cell `<= K` degree; an over-range value
drives the clamp-with-warning path (the small fixtures never approach u32 max); a monotonicity test
narrows a portal on the sole best path between two cells, re-bakes, and asserts the stored `aperture`
did not rise (AC5); a faceless-cell test builds a non-solid leaf with invalid (empty-sentinel) bounds
that still holds a portal and asserts its pairs read `perceivable` with `distance`/`aperture` `None`
(coupled-but-no-graded, AC4/AC5) and that the `is_valid()` guard runs before `centroid()` so no NaN
edge enters the graph; deterministic across recompiles.

### Task 3: Observability dump

Add a `cell_visibility` dump option to the headless runner so the baked relation is inspectable as
deterministic JSON. Extend `DumpSpec` (`crates/postretro/src/observability/runspec.rs`) with the
option; add a record type + `OutputDocument` field in `observability/document.rs`; widen
`build_output_document` to also receive `&world` (the driver has it in scope, so the `driver.rs`
call-site change is trivial — the one signature the dump needs beyond the registry). Pass the world,
not just `Option<CellVisibility>`: under the fallback (relation `None`) the dump still emits the trivial
single-component array of length `cell_count`, which it reads from `LevelWorld::cell_count()` — the
`None` relation cannot supply it. Emit the `u32[cell_count]` component-id array (O(V), cheap) as a sibling field alongside a
JSON array of `{ cell_a, cell_b, distance, aperture }` entries, so the reachability gate — the
`perceivable` component partition AC2/AC3 rest on — is inspectable independently of the coupled-pair
list, distinguishing "different component" from "same component but not stored". The pair array holds
one entry per unordered off-diagonal pair `cell_a < cell_b` that is coupled and stored (perceivable,
within the distance cap, and within either endpoint's fanout; diagonal omitted; non-stored pairs
absent), `distance`/`aperture` as integers (both always present for a stored pair; no `null` in the
pair array in v1) —
**pre-sorted ascending by `(cell_a, cell_b)`** by the producer, because `to_deterministic_json` sorts
object keys but leaves arrays in data order (see Determinism pin P4). Consumes only Task 1's runtime
type via its component-id slice accessor and coupled-pairs iterator (both exposed by Task 1), so it is
correct regardless of whether Task 2 data is placeholder or final — the determinism AC holds either way. Under the conservative fallback (no section loaded), the dump emits an
empty coupled-pair array; the component array reflects the fallback's all-perceivable semantics as a
single trivial component (all cells id 0), so the component field is always present, matching the
loaded-map dump shape. Test: two identical `--headless` runs
over a compiled fixture produce byte-identical stdout.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the index-space, section round-trip,
optional-fallback, reachability-gate, and query-neutrality assumptions before the graded passes land.
**Phase 2 (concurrent):** Task 2 (graded axes, compiler bake — the plan's heaviest, though far
lighter than a sightline flood: two graph passes + the cap) ‖ Task 3 (observability crate) — disjoint
files (`crates/level-compiler` only for Task 2, since Task 1's `coupling` accessor already reads the
side-table; `crates/postretro/observability` for Task 3, reading Task 1's component-id slice and
coupled-pairs accessors). Both consume Task 1's fixed format and query surface. Task 3 may be authored
concurrently with Task 2 (disjoint files), but AC7's integer-serialization path (non-null
`distance`/`aperture`) is verifiable only after Task 2 lands, or via a hand-built section fixture.

## Wire format

New PRL section `CellVisibility`, id 46, **optional** (absent → conservative fallback, not an error).
Mirrors the little-endian, u32-count conventions of `cells.rs` / `cell_locator.rs`. Constraints
(layout offsets are the implementer's, per the constraints-not-solutions rule):

- Little-endian throughout. Leading `version: u32` (`CELL_VISIBILITY_VERSION`, rejected on mismatch),
  then `cell_count: u32` (must equal the map's cell count; reject `0`).
- **Reachability gate** — a `u32[cell_count]` component-id array. `perceivable(a,b)` is
  `component[a] == component[b]`; the diagonal is trivially true. Component ids dense from 0, assigned
  in ascending lowest-member-cell order (determinism). This is `O(cell_count)` storage — the gate
  never materializes an N² matrix.
- **Graded side-table** — a count-prefixed, ascending-sorted list of
  `(cell_a, cell_b, distance, aperture)` records with `cell_a < cell_b`, present only for *stored*
  coupled off-diagonal pairs (same component, within the `distance` cap, and within either endpoint's
  per-source fanout cap `K` — so the list is `<= N·K` entries, never N²); `distance` and `aperture` as
  u32 fixed-point at scales **pinned in Task 1** (fractional bits + representable maxima fixed once).
  Task 2 asserts every value fits and clamps-with-warning at u32 max rather than wrapping. The writer
  checks `usize`→u32 pair-count conversion and all output-size/capacity arithmetic; it returns an error
  instead of serializing a truncated count. Empty list
  (v1 Task-1 placeholder, or a map with no coupled pairs) encodes as count `0`.
- **Two storage caps** (pinned `pub const`s), generous and structural, not consumer range policy. The
  **distance cap** (same fixed-point domain as the stored `distance`) drops `distance > cap` — a
  path-length pre-filter. The **per-source fanout cap** `K` then keeps only each cell's `K` nearest
  coupled partners by `distance`, storing a pair iff *either* endpoint keeps it (union). The distance
  cap bounds path length, not count; the fanout cap is the hard count bound (`<= N·K`), so a spatially
  compact, densely-connected component cannot blow the side-table to N². `CouplingTuple`'s
  `distance`/`aperture` `None` means "no graded detail" and covers the conservative fallback, a
  beyond-distance-cap pair, AND a beyond-fanout pair; a consumer must NOT infer "no bake present" from
  `None`. Distinguishing those cases is not resolved with a flag or sentinel in v1 — a separate
  deferred decision.
- No sightline axis in v1. The deferred sightline refinement lands at a `CELL_VISIBILITY_VERSION`
  bump as an added per-pair bit / column, additively — it does not alter the three v1 fields.
- The section is a recompile-everything artifact; determinism (AC10) requires the bake to emit the
  component array and the side-table in a fixed order — no HashMap/HashSet iteration order feeds
  component-id assignment, relaxation, tree order, side-table pair collection, or serialization.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Conservative gate — `perceivable` never omits a pair with any real coupling (zero false negatives) | Task 1 (component = portal-reachability) | Any future gate tightening must stay a conservative superset; the deferred sightline lands as a *separate* axis, never narrowing `perceivable` | AC2, AC3 |
| Symmetric — `perceivable`, `distance`, `aperture` all symmetric | Task 1 (undirected components), Task 2 (undirected graph → symmetric paths) | Side-table stores one `(min,max)` entry; query canonicalizes the pair; the fanout keep-set is unioned across both endpoints, so a pair kept by one is present for both | AC2, AC4, AC5 |
| `perceivable` is sole cull authority; graded axes modulate only, never gate | Task 2 (`distance`/`aperture` defined only on perceivable pairs) | Consumers/tests must not hard-cull on a scalar threshold | — (designed-for; no consumer wired in this build, so no verifying AC — enforced by consumers in their epics) |
| Reconciliation errs toward more coupling (min `distance`, max `aperture`) | Task 2 | N/A in v1 (symmetric by construction — no merge to reconcile); reserved for the deferred dynamic-mask layer | N/A in v1 (symmetric by construction — no merge to reconcile); reserved for the deferred dynamic-mask layer |
| Optional section → conservative fallback (all perceivable, no graded detail) | Task 1 (loader `None` path) | Loader default must be all-true, never all-false | AC1 |
| Distance cap + per-source fanout cap `K` bound storage (`<= N·K`) without a false negative | Task 2 (caps omit graded detail, keep `perceivable`) | Caps drop only graded values, never a component-gate bit; the distance cap bounds path length while the fanout cap bounds count; fanout must union both endpoints' keep-sets or symmetry breaks | AC11 |
| Deterministic bake + dump (byte-identical) | Task 1 (fixed emit order), Task 2 (pinned tie-breaks + fixed-point), Task 3 (pre-sorted pairs) | HashSet/HashMap iteration order; Dijkstra/MST tie-break; parallel completion order (P1: order-preserving collect + `(cell_a,cell_b)` sort → output invariant to the governor permit count); no wall-clock branch | AC7, AC10 |
| `CellVisibility` bake-stage cost is observable | Task 1 (stage bracketed with `begin_stage`/`finish_stage`; Task 2 inherits it) | Timing must not branch output (diagnostic only) | AC12 |
| CellId-only neutral query surface | Task 1 (query API) | Any consumer wiring or API addition | AC8, AC9 |

## Determinism & ordering pins

This build has no runtime mutable state, timer, or event surface (the query is load-once, read-only),
and the compiler bake reads only values that are final and immutably borrowed at its insertion point
(verified against `pipeline.rs`). The only ordering hazards are determinism-under-graph-algorithms in
the bake. Each row is concrete enough to write a test from; the task tests reference the pins.

| Pin | Scenario | Ordering the bake must fix | Expected outcome (AC) |
|---|---|---|---|
| P1 | Same fixture, two compiles under different governor permit counts (TUI-adjusted cores); parallel per-source Dijkstra tasks finish in different orders | Reduce each source to its `topK` inside the parallel map; **order-preserving collect** index-aligned to source id (the `sh_bake` pattern), never a completion-order push; serialize the component array in cell order and the side-table sorted by `(cell_a,cell_b)` | Byte-identical `CellVisibility` bytes regardless of permit count / completion order (AC10) |
| P2 | Two `a→b` paths equal to last-ULP under different float accumulation orders | Deterministic `Vec` adjacency in portal order; Dijkstra frontier keyed `(cost, node_id)` | Identical stored fixed-point `distance` across recompiles (AC4, AC10) |
| P3 | Two spanning trees tie on an aperture-equal edge | Maximum spanning tree breaks equal-aperture ties by portal index; no HashSet/HashMap iteration in tree construction | Identical stored `aperture` across recompiles (AC5, AC10) |
| P4 | Coupled pairs discovered in arbitrary order before serialization | Component array in cell order; side-table sorted by `(cell_a,cell_b)`, `cell_a<cell_b`; dump pre-sorts identically | Byte-identical section and dump (AC7, AC10) |
| P5 | Large connected map whose longest path exceeds the fixed-point range (path length) OR whose in-cap pair count blows up (a spatially compact, densely-connected component — the distance cap does NOT bound count) | Task 1 pins distance/aperture scales + representable maxima (fit); the **distance cap** bounds path length and the **per-source fanout cap `K`** bounds count at `<= N·K`; Task 2 asserts fit (clamp-with-warning), omits beyond-distance-cap pairs, reduces each source to its `K` nearest inside the parallel per-source Dijkstra map, and checks pair-count/output-size conversions (union across endpoints; full N² matrix never materialized) | Bounded, non-wrapping side-table AND `O(P·(N+E) + N·K)` bake working set (`P` and `E` as defined in AC11); overflow is a loud diagnostic (AC11) |
| P6 | Task 1 adds `StageId::CellVisibility` to the stage list | `ORDERED_STAGES`, `label`/`progress_label`, and `planned_stage_contract_pins_order_labels_and_sdf_prediction` updated together: the `[StageId; 22]` type annotation (→23), both `assert_eq!(...len(), 22)` sites (→23), both `[19]` ordinal assertions (`without_sdf`/`with_sdf`, each →`[20]`), and the label-vector insert between `"BVH Build"` and `"NavMesh"`; no new `predicted_present` arm needed (inherits `true` via `id != SdfAtlasBake \|\| needs_sdf`) | Build Summary shows the stage duration (AC12); stage-contract test green; `.prl` bytes identical across runs (timing contributes no bytes) |
| P7 | Pair with `distance` exactly `== cap` and pair `== cap+1` | Cap compare is `distance <= cap` stored / `> cap` omitted, in fixed-point domain | `==cap` stored with graded values; `==cap+1` perceivable with `None` graded (AC4, AC11) |
| P8 | Path `a→b` and `b→a` sum identical edges in opposite float order, rounding to adjacent integers at a fixed-point boundary | Each stored pair's value is its kept direction's `D` — `D(a→b)` if `b∈topK(a)`, else `D(b→a)`, and `D(a→b)` (min→max) when both keep it (a deterministic tie-break); each direction is individually deterministic (P2), and each passed its per-source cap; no last-write-wins dedup | One deterministic stored distance per pair, always `<= cap`, byte-identical across recompiles and across permit counts (AC10) — the direction choice is distance-only; aperture is the symmetric MST bottleneck (P3), with no per-direction value to pick. Distinct from P1 (which pins collect/emit order, not the pair-value choice) |
| P9 | Side-table-assembly fn is cap/fanout-parameterized; a unit test passes a small `cap` or `K`, the shipped bake must not | The `cell_visibility_bake` stage binds `cap` and `K` to their `pub const`s; only tests pass other values | Shipped side-table always reflects the production `cap`/`K`; two compiles byte-identical AND the artifact uses the intended values (AC10, AC11) |
| P10 | A cell has more than `K` in-cap coupled partners; two compiles could keep a different `K`-subset if the ranking is not a total order | `topK(s)` selected by `(D(s→t), partner_cell_id)` — a total order (partner ids unique), reduced per source inside the parallel per-source Dijkstra map; the stored set is the union of both endpoints' `topK`; the cap filter and the stored value both use the kept direction's own `D` (P8), so membership, cap, and value agree by construction; no HashSet/HashMap iteration in selection | Identical `K`-nearest selection and stored pair set across recompiles and permit counts; symmetric (a pair kept by one endpoint present for both); no pair stored with `distance > cap`; `<= N·K` entries and `O(P·(N+E) + N·K)` bake working set (AC10, AC11) |
| P11 | One pair whose two directed Dijkstra sums straddle the cap: `D(a→b)_fp = cap+1`, `D(b→a)_fp = cap` (a ULP difference rounding to opposite sides of the boundary), where `b` has fewer than `K` closer in-cap partners (so the within-cap direction can actually rank into its top-`K`) | Membership is **top-`K`, not merely in-cap**: `b ∉ topK(a)` (the `a→b` direction is over cap); `a ∈ topK(b)` **iff** `a` ranks into `b`'s `K` nearest in-cap partners — being in-cap (`D(b→a) <= cap`) is necessary but not sufficient. The keep-set union stores the pair iff the within-cap direction ranks it into that source's top-`K`, never merely because it is in-cap; the over-cap `a→b` direction never stores it. Stored value = the kept direction's `D` (`= cap`) | With `b`'s fanout admitting `a`: pair stored once, value `cap` (`<= cap` holds), symmetric, byte-identical across recompiles and permit counts (AC10, AC11). If instead `b` already holds `K` closer partners so `a ∉ topK(b)`: pair dropped (perceivable, `None`/`None`) — never stored from the over-cap direction, and the `<= N·K` bound is never inflated by storing an in-cap-but-not-top-`K` pair. Distinct from P7 (single distance vs cap) and P8 (both directions keep) |
| P12 | Two in-cap partners of source `s` whose true float distances differ but round to the same fixed-point value; an implementer could rank by float while filtering/storing fixed-point | `D(s→t)` is resolved to its stored fixed-point value once; that single value feeds the rank key, the `<= cap` compare, and storage; equal fixed-point values break by `partner_id` | Identical top-`K` and stored value across recompiles and permit counts — no float-rank vs fixed-point-rank divergence (AC10). Scope is same-binary byte-identity (all AC10 requires); cross-implementation / cross-arch float reproducibility of the pre-round sum is not pinned and not needed — the baked `.prl` ships, so every consumer loads identical bytes |
| P13 | Pair `{a,b}` mutually in each other's top-`K` emits two directed candidates `(a→b)`, `(b→a)` into pair collection | Collapse to one `(min,max)` entry by sort-then-collapse (sort directed candidates by `(min,max)`, collapse each equal-key run, take the P8 **distance** on a two-direction run — aperture is the pair's MST bottleneck (P3), symmetric, with no per-direction value to pick) — no HashSet, honoring the no-HashSet-in-pair-collection pin | Exactly one `(a,b)` row, never two (AC7/AC11 one-entry-per-pair); byte-identical (AC10). A duplicate-row bug is invisible to AC10 alone, so this is its own pin |
| P14 | Operator moves the TUI ± permit dial mid-bake, so `set_permits` changes `P` partway through the per-source `par_iter` (some sources finish at P=1, others at P=W within one compile) | Order-preserving collect is index-aligned to source id and every stored value is completion-order-independent (P1, P8), so a live permit change cannot reorder or revalue output | Byte-identical to any fixed-permit compile of the same fixture (AC10) — extends P1 from across-compile to within-compile invariance, the scenario the "live from the TUI" prose promises. A meaningful test needs a test-only barrier (block a source's completion until a `set_permits` has provably landed); without one a small fixture may finish before the toggle fires and pass vacuously — the pin asserts the invariant, the barrier proves the scenario ran |
| P15 | The per-source `par_iter` runs over a source list that skips faceless (no-hub) cells, so the collected Vec could align to a filtered subset rather than cell id `0..cell_count` | Iterate `0..cell_count` (a faceless source yields an empty `topK`) so the collect index is the cell id, OR carry each result's source cell id and remap collect-index → cell-id before the union/sort — never leave output aligned to a filtered/compacted subset (a `filter`/`filter_map` inside the `par_iter` desyncs the index the same way). The source→hub lookup is a total map `cell_id → Option<hub>` (hubs are a compacted `portal_count..` range, so a dense `hub_node[s]` index would be out-of-bounds for faceless / solid / zero-portal cells); `None` yields an empty `topK` | Keep-set union and `(cell_a,cell_b)` sort operate on true cell ids; byte-identical regardless of whether faceless cells are filtered before or after iteration (AC10) |

## Rough sketch

- **Types.** `CouplingTuple { perceivable: bool, distance: Option<u32>, aperture: Option<u32> }`.
  Runtime `CellVisibility` holds the lowered component array + graded side-table; `LevelWorld` gains
  `cell_visibility: Option<CellVisibility>` (added in `prl.rs`) and the `perceivable` / `coupling`
  accessors, added to the `impl LevelWorld` block in `prl.rs` beside the `locate_cell` / `cell_count`
  precedent.
- **Bake stage** in `crates/level-compiler`, template `cell_draw_index_bake`: consumes `result.tree`
  / `vis_result.leaves_section.leaves`, `generated_portals` (leaf-keyed == cell-keyed by the identity
  mapping), `exterior_leaves`; runs connected-components (Task 1), then per-source Dijkstra + max-
  spanning-tree bottleneck (Task 2); emits pre-serialized bytes into `pack_and_write_portals`. Reuses
  `find_exterior_leaves`'s portal-adjacency construction and solid-barrier handling; the outer loop
  over all unvisited cells and the dense component-id assignment are net-new (`find_exterior_leaves`
  is single-seed BFS from an exterior probe, not an all-cells components labeler).
- **Exterior/solid cells.** Valid CellIds. Solid cells (no portals) are singleton components,
  diagonal-only. Exterior cells are non-solid with valid bounds and participate through their portals;
  no special-casing keeps the relation neutral. A faceless non-solid cell with invalid bounds
  (`!Aabb::is_valid()`) contributes no hub node (guarded before `centroid()`, so no NaN edge); if
  it holds a portal it is perceivable but reads coupled-but-no-graded, as does any pair routable
  only through it (AC4/AC5).
- **Deferred sightline axis.** When a measured consumer needs hard visibility, add a per-pair
  line-of-sight refinement (the anti-penumbra math in `drafts/perf-anti-penumbra-pvs`) as a *new*
  axis at a version bump — consulted by that consumer, ignored by audio. It narrows nothing in the
  three v1 fields, so no consumer churns.
- **Split-before-extend.** `pack.rs`, `prl_loader.rs`, and `pipeline.rs` are already multi-thousand-
  line files the project extends in place (e.g. `cell_draw_index_bake` lives in `pipeline.rs`); do not
  split them as part of this plan. Extend along the existing section/stage seams.

## Open questions

- **Aperture metric** (portal min-width vs. polygon area) is left to Task 2's implementer within the
  wire-format constraints; both are monotone coupling-quality keys. Flagged so review confirms the
  chosen metric stays deterministic and the widest-path result is symmetric.
- **Distance-cap magnitude.** `K` is decided (`CELL_VISIBILITY_FANOUT_K = 32`, budget-derived: ~128 MB
  at the design-target `N ≈ 250k`-leaf ceiling, linear beyond — a retunable dial whose load-bearing
  guarantee is the linear `N·K` bound, not the value). The coupling `distance` cap's magnitude stays a
  generous Task-1 `pub const` (a coarse path-length pre-filter, set beyond any plausible consumer's
  range); it is a dial, not a measured value. Flagged so review confirms both defaults are generous
  enough that a cell's real near-ring is never dropped, and that the uncapped/large-`K` small-fixture
  test path exists. Retune both against real stress-warren content as prl-build's large-map support
  lands — but neither blocks shipping, because a dropped pair stays `perceivable` and the linear bound
  holds at every value.
- **Fanout ranks by `distance` only.** A pair far by path but wide by `aperture` (a long, wide
  corridor) can be dropped by the fanout even when its aperture is high. Accepted: dropped pairs stay
  `perceivable` (the gate — audio's potentially-audible set — is intact and conservative) and read
  `None`/`None` (the same fallback every consumer already handles), and distance-ranking aligns with all
  three primary consumers (net relevance, AI perception, and audio are all distance-dominant, so a
  cell's `K` nearest partners are the ones they weight most). The substrate does not preserve
  high-aperture-but-far pairs in the graded table — an accepted limitation of a distance-ranked fanout,
  not a bug.
