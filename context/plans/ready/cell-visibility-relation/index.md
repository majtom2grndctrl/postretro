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
side-table can approach N². v1 bounds it with a generous coupling cap (below); the exact profile is
measurable only when real maps exist (fixtures are synthetic — see `research.md`). So the go decision
rests partly on an unvalidated bet — that the graded axes cull hard enough and the capped side-table
stays affordable — taken with eyes open because the direction is a two-way door: backing out deletes
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
  pairs within the coupling cap (`distance <= cap`, compared in the same fixed-point domain as the
  stored value) and `None` otherwise (diagonal, non-perceivable, beyond cap i.e. `distance > cap`, or a
  pair touching a faceless invalid-bounds cell that contributes no metric node — coupled-but-no-graded);
  symmetric; equals the shortest portal-graph path length under the pinned edge metric (Task-1
  fixed-point scale); deterministic across recompiles.
- [ ] AC5 — `aperture(a, b)` is `Some` on the same coupled pairs as `distance` and `None` otherwise;
  symmetric; equals the widest-path bottleneck — the maximum over all paths of the minimum portal
  aperture on the path — under the pinned aperture metric; monotone (narrowing any portal on the
  sole best path cannot raise the stored aperture); deterministic across recompiles.
- [ ] AC6 — The query returns `CouplingTuple { perceivable, distance, aperture }`; under the
  fallback it returns `{ true, None, None }` for every off-diagonal pair. Adding the deferred
  sightline axis later is additive (a new field / section-version bump), not a change to these three
  fields (no consumer signature churn on the existing axes).
- [ ] AC7 — `xtask observe` / `--headless` with a `cell_visibility` dump emits the relation as JSON:
  the `u32[cell_count]` component-id array (the reachability gate, inspectable independent of the
  coupled-pair list) alongside one entry per unordered off-diagonal pair with `cell_a < cell_b` that is
  coupled (perceivable and within the cap — the informative set; diagonal omitted), sorted ascending by
  `(cell_a, cell_b)`; `distance`/`aperture` serialize as their integer or `null`. Two identical runs are
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
- [ ] AC11 — The coupling cap bounds the graded side-table: pairs whose `distance` exceeds the
  pinned cap (`distance > cap`, in the same fixed-point domain as the stored `distance`) are omitted
  (perceivable stays true; `distance`/`aperture` read `None` — coupled but beyond the stored horizon).
  The cap choice is the only difference and is deterministic given the cap. On an uncapped small
  fixture the table holds every reachable off-diagonal pair. The omission boundary is unit-tested via a
  cap parameter on the side-table-assembly function (a small cap over a hand-built graph exercises
  `distance == cap` stored / `distance == cap+1` omitted, pin P7), since the generous production cap is
  never crossed by a synthetic fixture. The shipped `cell_visibility_bake` stage binds this parameter to
  the `pub const` cap; only unit tests pass another value, so no stray cap value can leak into the
  shipped bytes (P9).
- [ ] AC12 — The bake stage logs its duration through the Build Summary path, so the cost is visible
  on the first real-map compile. (Wiring gate — provable by the stage-contract test confirming
  `CellVisibility` is in `ORDERED_STAGES` and the stage calls `finish_stage`.)

## Tasks

### Task 1: Thin slice — section format, reachability gate, loader, query API

Stand the full pipe end to end with the real reachability gate and an empty graded side-table, to
falsify the boundary assumptions before the graded passes land. Add `SectionId::CellVisibility = 46`
and its `from_u32` arm (`crates/level-format/src/lib.rs`; do not reuse the retired id 14). Author the
section module (mirror `cell_locator.rs`): a `CellVisibilitySection` with
`CELL_VISIBILITY_VERSION: u32 = 1`, `to_bytes`/`from_bytes` (bounds-checked, rejects truncation and
trailing bytes), carrying the complete v1 wire layout from the start so later work fills data into a
fixed format, never re-cuts it. Wire constraints (task agents do not receive the Wire-format section
— restated here): little-endian, u32 counts (mirror `cell_locator.rs`); a leading `version: u32`
(`CELL_VISIBILITY_VERSION`, rejected on mismatch) then `cell_count: u32` (must equal the map's cell
count; reject `0`); a per-cell **component id** array (`u32[cell_count]` — the reachability gate,
`perceivable(a,b) = component[a] == component[b]`); and a count-prefixed, ascending-sorted
`(cell_a, cell_b, distance, aperture)` graded side-table with `cell_a < cell_b`, present only on
coupled pairs (empty list encodes as count `0`). Pin the `distance` and `aperture` fixed-point
scales, and the coupling `distance` cap, as named `pub const`s in the section module (chosen from the
map world-bound budget; scales' fractional bits + representable maxima fixed once) so Task 2 reads
them from source. Pin the cap in the same domain as the stored `distance` (fixed-point `u32` counts, at
the Task-1 scale), and pin the boundary operator: `distance <= cap` → stored; `distance > cap` →
omitted (`perceivable` stays true, graded `None`). Add a bake stage `cell_visibility_bake` (name the stage function so Task 2 can locate it) after
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
passes inherit the same bracketed stage. In this task the bake fills the **component ids** (connected
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
the portal-centroid graph (portals = nodes), keyed by ordered pair `(min, max)`, stored on coupled
off-diagonal pairs. Pin the edge metric explicitly so nobody later "fixes" it as a bug: nodes are
portals represented by their polygon centroid, edge weight between two portals sharing a cell is the
Euclidean distance between their centroids, and a cell's own endpoints contribute
cell-center→portal-centroid segments (cell-center is `BspLeaf.bounds.centroid()` — `result.tree` is
threaded into the bake — guarded by `Aabb::is_valid()`: a faceless leaf whose bounds are the empty
sentinel (`min=+INF, max=-INF`, so `centroid()` is NaN) contributes no cell-center node, mirroring how
`find_exterior_leaves` skips faceless leaves. Solid and exterior cells both carry *valid* bounds — the
empty sentinel is not the solid/exterior marker; solids are absent from the portal-centroid graph
anyway by portal absence (`generate_portals` emits only between two non-solid leaves), and exterior
cells are non-solid and participate normally through their portals. The only excluded participant is a
faceless invalid-bounds non-solid leaf that still holds a portal: it reads coupled-but-no-graded, see
AC4/AC5); the result is a path-length coupling-quality key, not geometric ground truth. Populate
distance only for pairs the component array marks perceivable — the cell-adjacency components gate the
side-table; the portal-centroid graph's connectivity agrees with cell-component membership by
construction: every non-solid cell that participates in coupling touches at least one portal, and a
non-solid zero-portal cell is its own singleton in both graphs (like a solid cell), so both graphs
connect the same cell set. **Aperture:** the
widest-path (maximin) bottleneck — the largest, over all paths, of the smallest portal aperture on
the path. Pin the per-portal aperture metric (the portal polygon's minimum width, or its area — the
implementer picks one and documents it; it is a coupling-quality key, not a solid angle). Compute
all-pairs bottleneck from a maximum spanning tree of the cell-adjacency portal graph (cells = nodes,
portals = edges) weighted by aperture (the bottleneck between two cells is the minimum aperture on the
unique tree path). Both axes are
symmetric by construction — the portal graph is undirected, so `dist(a→b) == dist(b→a)` and the
bottleneck is symmetric; no union-of-directions is needed. The algorithm operates purely on the
portal graph — no BVH / brush raycast, because solid geometry is already portal absence
(`generate_portals` only emits portals between two non-solid leaves). Solid cells (no portals) are
their own singleton component (perceivable only on their own diagonal, no graded entries). Store both
at the Task-1-pinned fixed-point scales — read the named `pub const`s from the section module (Task 2
receives neither Task 1's paragraph nor the Wire-format section); assert every value fits its
representable range and clamp-with-warning at the maximum rather than wrapping.

The **coupling cap** (AC11) bounds storage: a pair whose `distance` exceeds the pinned cap is omitted
from the side-table entirely (both axes) — it stays perceivable (same component) but reads
`distance`/`aperture` `None`, i.e. coupled-but-beyond-the-stored-horizon. The cap is a structural
storage bound, set generously (beyond any plausible consumer's range), not a consumer policy; it is
the determinism-preserving guardrail against N² storage blow-up on large connected maps. On the small
fixtures leave it effectively uncapped so tests see the full reachable set.

Determinism (AC10): build adjacency as `Vec`s in a fixed portal order; key the Dijkstra frontier on
`(cost, node_id)` — `node_id` is a single injective index over all frontier node kinds (portal-centroid
nodes and any cell-endpoint / super-source nodes get disjoint id ranges, e.g. portals `0..P`, cell/
virtual nodes `P..`), so `(cost, node_id)` is a genuine total order: two distinct nodes at equal float
cost can never share an id, so float last-ULP accumulation differences cannot flip the stored value;
break equal-aperture ties in the maximum spanning tree by portal index; no HashMap/HashSet iteration
order feeds component-id assignment, relaxation, tree order, side-table pair collection, or
serialization (see Determinism pins P2/P3) — component ids are assigned by an explicit sort of distinct
representatives ascending (not first-seen HashMap iteration), so "dense from 0 in ascending
representative order" is a pinned procedure, not only a pinned result. Assemble the side-table by
iterating source cells in ascending id and emitting only targets `t > s`, so each unordered pair's
stored value comes from `Dijkstra(min→max)` exactly once — no dedup HashMap, no cross-direction
last-write-wins reconciliation (float addition is non-associative, so `a→b` and `b→a` can round to
adjacent fixed-point integers). If the per-source Dijkstra is run under `rayon` `into_par_iter()` for
speed (optional — it is cheap single-threaded), collect rows index-aligned to cell id via an
order-preserving collect, never a completion-order push (pin P1). Task 1's `coupling` accessor already
reads the side-table, so filling it here makes the accessor return `Some` on coupled pairs with no
`crates/level-loader` edit — Task 2's changes stay in `crates/level-compiler` (the `cell_visibility_bake`
stage Task 1 created). The side-table-assembly function takes the coupling cap as a parameter (the
`pub const` is the production default), so a unit test can drive a small cap to exercise the omission
boundary (AC11 / pin P7) without a fixture that crosses the generous production cap. Tests:
`distance`/`aperture` present exactly
on coupled off-diagonal pairs and absent on diagonal/non-perceivable/beyond-cap pairs; both symmetric;
`distance` matches an independent shortest-path oracle and `aperture` an independent bottleneck oracle
on a fixture (same-metric oracles validate the relaxation/tree and determinism, not the metric
choice, which is a pinned design decision); a small-cap unit test confirms `distance == cap` stored /
`distance == cap+1` omitted (perceivable stays true); an over-range value drives the clamp-with-warning
path (the small fixtures never approach u32 max); deterministic across recompiles.

### Task 3: Observability dump

Add a `cell_visibility` dump option to the headless runner so the baked relation is inspectable as
deterministic JSON. Extend `DumpSpec` (`crates/postretro/src/observability/runspec.rs`) with the
option; add a record type + `OutputDocument` field in `observability/document.rs`; widen
`build_output_document` to also receive the loaded relation (the driver has `world` in scope, so the
`driver.rs` call-site change is trivial — this is the one signature the dump needs beyond the
registry). Emit the `u32[cell_count]` component-id array (O(V), cheap) as a sibling field alongside a
JSON array of `{ cell_a, cell_b, distance, aperture }` entries, so the reachability gate — the
`perceivable` component partition AC2/AC3 rest on — is inspectable independently of the coupled-pair
list, distinguishing "different component" from "same component but beyond cap". The pair array holds
one entry per unordered off-diagonal pair `cell_a < cell_b` that is coupled (perceivable and within the
cap; diagonal omitted; non-coupled pairs absent), `distance`/`aperture` as their integer or `null` —
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
  `(cell_a, cell_b, distance, aperture)` records with `cell_a < cell_b`, present only for coupled
  off-diagonal pairs (same component and within the `distance` cap); `distance` and `aperture` as u32
  fixed-point at scales **pinned in Task 1** (fractional bits + representable maxima fixed once). Task
  2 asserts every value fits and clamps-with-warning at u32 max rather than wrapping. Empty list
  (v1 Task-1 placeholder, or a map with no coupled pairs) encodes as count `0`.
- The **coupling cap** (a pinned `pub const` distance threshold, in the same fixed-point domain as the
  stored `distance`) bounds the side-table: `distance <= cap` is stored, `distance > cap` is omitted
  and reads as perceivable-with-`None`-graded-values. Generous and structural, not a consumer range
  policy. `CouplingTuple`'s `distance`/`aperture` `None` means "no graded detail" and covers BOTH the
  conservative fallback AND a perceivable-but-beyond-cap pair; a consumer must NOT infer "no bake
  present" from `None`. This ambiguity is not resolved with a new flag or sentinel in v1 — that is a
  separate deferred decision.
- No sightline axis in v1. The deferred sightline refinement lands at a `CELL_VISIBILITY_VERSION`
  bump as an added per-pair bit / column, additively — it does not alter the three v1 fields.
- The section is a recompile-everything artifact; determinism (AC10) requires the bake to emit the
  component array and the side-table in a fixed order — no HashMap/HashSet iteration order feeds
  component-id assignment, relaxation, tree order, side-table pair collection, or serialization.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Conservative gate — `perceivable` never omits a pair with any real coupling (zero false negatives) | Task 1 (component = portal-reachability) | Any future gate tightening must stay a conservative superset; the deferred sightline lands as a *separate* axis, never narrowing `perceivable` | AC2, AC3 |
| Symmetric — `perceivable`, `distance`, `aperture` all symmetric | Task 1 (undirected components), Task 2 (undirected graph → symmetric paths) | Side-table stores one `(min,max)` entry; query canonicalizes the pair | AC2, AC4, AC5 |
| `perceivable` is sole cull authority; graded axes modulate only, never gate | Task 2 (`distance`/`aperture` defined only on perceivable pairs) | Consumers/tests must not hard-cull on a scalar threshold | — (designed-for; no consumer wired in this build, so no verifying AC — enforced by consumers in their epics) |
| Reconciliation errs toward more coupling (min `distance`, max `aperture`) | Task 2 | N/A in v1 (symmetric by construction — no merge to reconcile); reserved for the deferred dynamic-mask layer | N/A in v1 (symmetric by construction — no merge to reconcile); reserved for the deferred dynamic-mask layer |
| Optional section → conservative fallback (all perceivable, no graded detail) | Task 1 (loader `None` path) | Loader default must be all-true, never all-false | AC1 |
| Coupling cap bounds storage without a false negative | Task 2 (cap omits graded detail, keeps `perceivable`) | Cap must drop only graded values, never a component-gate bit | AC11 |
| Deterministic bake + dump (byte-identical) | Task 1 (fixed emit order), Task 2 (pinned tie-breaks + fixed-point), Task 3 (pre-sorted pairs) | HashSet/HashMap iteration order; Dijkstra/MST tie-break; parallel reassembly order (P1); no wall-clock branch | AC7, AC10 |
| `CellVisibility` bake-stage cost is observable | Task 1 (stage bracketed with `begin_stage`/`finish_stage`; Task 2 inherits it) | Timing must not branch output (diagnostic only) | AC12 |
| CellId-only neutral query surface | Task 1 (query API) | Any consumer wiring or API addition | AC8, AC9 |

## Determinism & ordering pins

This build has no runtime mutable state, timer, or event surface (the query is load-once, read-only),
and the compiler bake reads only values that are final and immutably borrowed at its insertion point
(verified against `pipeline.rs`). The only ordering hazards are determinism-under-graph-algorithms in
the bake. Each row is concrete enough to write a test from; the task tests reference the pins.

| Pin | Scenario | Ordering the bake must fix | Expected outcome (AC) |
|---|---|---|---|
| P1 | Same fixture, two compiles; per-source Dijkstra tasks (if parallel) finish in different completion orders | Rows reassembled index-aligned to cell id via order-preserving collect; serialize the component array and side-table in fixed order — never a completion-order push | Byte-identical `CellVisibility` bytes (AC10) |
| P2 | Two `a→b` paths equal to last-ULP under different float accumulation orders | Deterministic `Vec` adjacency in portal order; Dijkstra frontier keyed `(cost, node_id)` | Identical stored fixed-point `distance` across recompiles (AC4, AC10) |
| P3 | Two spanning trees tie on an aperture-equal edge | Maximum spanning tree breaks equal-aperture ties by portal index; no HashSet/HashMap iteration in tree construction | Identical stored `aperture` across recompiles (AC5, AC10) |
| P4 | Coupled pairs discovered in arbitrary order before serialization | Component array in cell order; side-table sorted by `(cell_a,cell_b)`, `cell_a<cell_b`; dump pre-sorts identically | Byte-identical section and dump (AC7, AC10) |
| P5 | Large connected map whose longest path exceeds the fixed-point range or whose pair count blows up | Task 1 pins distance/aperture scales + representable maxima and the coupling cap; Task 2 asserts fit (clamp-with-warning) and omits beyond-cap pairs | Bounded, non-wrapping side-table; overflow is a loud diagnostic (AC11) |
| P6 | Task 1 adds `StageId::CellVisibility` to the stage list | `ORDERED_STAGES`, `label`/`progress_label`, and `planned_stage_contract_pins_order_labels_and_sdf_prediction` updated together: the `[StageId; 22]` type annotation (→23), both `assert_eq!(...len(), 22)` sites (→23), both `[19]` ordinal assertions (`without_sdf`/`with_sdf`, each →`[20]`), and the label-vector insert between `"BVH Build"` and `"NavMesh"`; no new `predicted_present` arm needed (inherits `true` via `id != SdfAtlasBake \|\| needs_sdf`) | Build Summary shows the stage duration (AC12); stage-contract test green; `.prl` bytes identical across runs (timing contributes no bytes) |
| P7 | Pair with `distance` exactly `== cap` and pair `== cap+1` | Cap compare is `distance <= cap` stored / `> cap` omitted, in fixed-point domain | `==cap` stored with graded values; `==cap+1` perceivable with `None` graded (AC4, AC11) |
| P8 | Path `a→b` and `b→a` sum identical edges in opposite float order, rounding to adjacent integers at a fixed-point boundary | Every unordered pair's value comes from `Dijkstra(min→max)` only (iterate sources ascending, emit targets `t>s`); no dedup map | One deterministic stored distance/aperture per pair, byte-identical across serial/parallel builds (AC10) — distinct from P1 (per-source row reassembly, not the two-direction pair-value choice) |
| P9 | Side-table-assembly fn is cap-parameterized; a unit test passes a small cap, the shipped bake must not | The `cell_visibility_bake` stage binds `cap = <the pub const>`; only tests pass another value | Shipped side-table always reflects the production cap; two compiles byte-identical AND the artifact uses the intended cap (AC10, AC11) |

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
  (`!Aabb::is_valid()`) contributes no distance node (guarded before `centroid()`, so no NaN edge); if
  it holds a portal it is perceivable but reads coupled-but-no-graded (AC4/AC5).
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
- **Coupling cap value** is pinned by Task 1 but its right magnitude is unmeasurable until real maps
  exist (fixtures are synthetic). Flagged so review confirms the cap is generous enough not to hide
  coupling any plausible consumer would use, and that the uncapped-small-fixture test path exists.
