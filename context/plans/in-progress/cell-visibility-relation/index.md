# Cell-Visibility Relation (static foundation)

## Goal

A view-independent, baked Cell→Cell "potential perceptibility" relation: for cells A and B,
can any point in A perceive any point in B along a sightline through the portal graph. Computed
once at compile time over cells and portals, emitted as a PRL section, queried at runtime as
cheap lookups through a consumer-agnostic API. This is the shared substrate E15 network
relevance, E12 audio occlusion, E10 AI-perception broad-phase, projectile-VFX culling, and
E17-F doors-as-occluders all consume — built once so none of them reinvents it. This build
ships the static floor `{ perceivable, distance }`; `aperture` and dynamic-geometry masking are
designed-for but deferred to the consumers that need them.

## Scope

### In scope

- A compile-time bake computing `perceivable` (conservative sightline PVS over the portal graph)
  and `distance` (shortest portal-graph path length) for every cell pair.
- A new optional PRL section (`CellVisibility`, id 46) carrying a per-cell perceivable bitset +
  a sparse `distance` side-table.
- Loader support with a conservative fallback (missing section → all cells mutually perceivable).
- A consumer-agnostic runtime query API over `CellId` only, returning
  `CouplingTuple { perceivable, distance, aperture }` with `aperture` at a fully-open sentinel.
- A `cell_visibility` observability dump: emit the baked relation for a loaded map as
  deterministic JSON through the headless runner.
- Property/invariant test coverage: conservative (zero false negatives), symmetric, strictly
  tighter than reachability, distance-defined-only-on-perceivable, deterministic.
- The generalizability gate: API smell-test audit + the second-consumer paper-check
  (`research.md`).

### Out of scope

- `aperture` magnitude (the bottleneck constriction). Lands with Epic 12 audio, extracted from
  the same separating planes; the runtime struct reserves the field, the section adds the column
  at a later version bump.
- Dynamic geometry — doors/movers/destructibles as dynamic portals, blocker masks, the widened
  `(a, b, portal-state)` query. Deferred to E17-F / the destructible epic; the section format and
  query struct are shaped to accept it additively.
- Any wired gameplay consumer (network relevance, audio, AI broad-phase). Proven here by tests +
  observability + paper-check; the real consumers attach in their own epics.
- Runtime recompute of the relation. The relation is a pure function of static geometry.
- Mod-facing / scripting surface. The substrate is engine-internal; no FGD or SDK types.

## Direction

**Problem.** The engine has no view-independent, area-source perceptibility relation. The only
visibility is per-frame, point-source portal traversal (`crates/visibility/src/portal_vis.rs`),
which answers "what does this camera see now" and structurally cannot answer "can any observer
in A ever perceive B" — the question network relevance, audio PAS, AI perception, and door
occlusion all ask. E17-F and three other epics are each written to consume a substrate that does
not exist. Foundation-first builds it once, consumer-agnostic, so those consumers stop being
blocked on it.

**Prior commitments.** This formalizes v1 of `context/research/cell-visibility-substrate.md`
(design intent). It honors that doc's architectural contract — CellId-only vocabulary, neutral
crate, the `{ perceivable, distance, aperture }` tuple as the minimal orthogonal basis, the
conservative/symmetric/errs-toward-coupling invariant hierarchy, the optional-section fallback,
and additive seams for aperture and dynamic masks. It aligns with the **Baked over computed**
principle (`context/lib/index.md` §2): the relation is a pure function of static geometry, so it
bakes. It relies on the tree-wide `CellId == BSP leaf index` identity
(`pack::encode_cells`). It is the prerequisite E17-F's visibility half builds its dynamic
blocker-mask layer on top of. Two build-guidance clauses of the substrate doc are consciously
overridden by the owner — building ahead of the doc's "measured consumer need" trigger, and
proving via tests+observability rather than a wired gameplay consumer — argued in
`research.md` (§Owner-approved divergences). Both keep the doc's architectural clause intact.

**Alternatives rejected.** (1) *Let the first consumer build dynamic-vis inline and extract
later.* Rejected — the substrate doc's central failure mode: a relation built inside one
consumer absorbs that consumer's policy and every other consumer forks it. (2) *Build the full
graded tuple + dynamic masks now (all of E17-F's visibility half).* Rejected — over-builds ahead
of consumers; `aperture` belongs with the audio consumer that needs obstruction magnitude, and
masking belongs with the door/destructible consumer that flips the bits. Foundation-first ships
only the floor every consumer shares. (3) *Mere portal-graph reachability as the default PVS.* Rejected
as the default — the doc's finding, confirmed in source (`find_exterior_leaves` shows portals keep
a level connected): reachability barely culls in a connected level, so it fails the consumers it is
built for. The sightline separating-plane test is what makes the relation useful, so it is v1
work, not a later tightening. Reachability is *retained* as the `--pvs-fast` fallback and the
past-the-depth-cap degradation path (Task 2) — both conservative-safe supersets of the sightline
floor — so the cheap flood earns its place as a guardrail, just not as the shipped default.
Measured cost confirms the default sightline pass is affordable now: the current Visibility stage
is sub-10ms and SH-bake dominates compile at ~83%, the largest fixture at ~2,561 portals — an
order of magnitude below the Quake maps where `vis` cost minutes (see `research.md`). (4) *The shape the substrate doc and roadmap prescribe — build
the static relation A→B in one run with E15 Phase 4 network relevance, or E12 audio, as the
first consumer* (`cell-visibility-substrate.md` Build Guidance; roadmap Phase 4 line 206). This
is the strongest rival and the owner diverges from it wittingly (`research.md` §Owner-approved
divergences). Two merits carry the divergence: (a) **reversibility** — the section is optional
(absent → conservative fallback), the query is neutral, and no consumer is wired, so if a real
first consumer later shows the API is wrong the cost is a version bump + recompile, not a
consumer migration; the only non-recoverable work is the Task 2 algorithm, reusable under any
API shape. This is a fundamentally weaker risk than the `perf-forward-light-cull` post-mortem
the doc cites, which shipped runtime code. (b) **The prescribed first consumer is itself
blocked and unmeasurable now** — E15 Phase 4 sits behind unbuilt session/lobby work and is
gated on 16-player bandwidth pressure that cannot be measured at current scale; E12 audio is
roadmap-flagged later/speculative. So "build with the first consumer" would either block this
foundation behind a large separately-gated epic or manufacture an unmeasured consumer — itself
against the doc's measured-need clause. The cost the divergence accepts: the paper-check (AC9)
now bears both first-consumer API validation and generalizability, so the "struct complete day
one, no consumer churns" promise stays unfalsified until the first real consumer lands —
bounded by (a).

## Acceptance criteria

- [ ] AC1 — `prl-build` on a fixture map emits a `CellVisibility` section (id 46). A map with no
  such section (old or bake disabled) loads and runs; the query returns "all perceivable" for
  every pair (conservative fallback), no error, no panic.
- [ ] AC2 — `perceivable(a, b)` is defined for every ordered pair in `0..cell_count`; it is
  symmetric (`perceivable(a,b) == perceivable(b,a)`); the diagonal `perceivable(a,a)` is true.
- [ ] AC3 — On an occluder fixture (e.g. `occlusion-test.map`), the perceivable set is a strict
  subset of the portal-reachable set: at least one reachable pair is correctly reported
  not-perceivable. (Proves sightline, not reachability.) The chosen fixture must be confirmed to
  contain a reachable-but-occluded pair — a correct bake fails this AC on a fixture that has none,
  so verify the topology or author one that does.
- [ ] AC4 — Conservative: on the fixtures, no pair with a real sightline is reported
  not-perceivable, checked against an independent ground-truth oracle on small in-memory
  portal-graph fixtures: dense point-pair sampling between the two cells' volumes, tested against
  the portal openings. The check is sound but sampling-limited — every sightline the oracle finds
  must be marked perceivable by the bake, so it cannot pass vacuously on a sampled sightline; small
  fixtures plus dense sampling make it near-exhaustive. (It is deliberately the incomplete
  direction: the oracle under-reports rather than inventing sightlines the bake could miss.) The
  in-test fixture supplies per-cell volumes (AABBs) alongside the `Vec<Portal>` so the oracle has
  cell geometry to sample between.
- [ ] AC5 — With a `CellVisibility` section loaded, `distance(a, b)` is `Some` exactly on
  perceivable off-diagonal pairs and `None` otherwise; symmetric; equals the shortest portal-graph
  path length under the pinned edge metric (at the Task-1-pinned fixed-point scale, exposed as a
  named `pub const` in the `cell_visibility` section module); deterministic across recompiles. Under the missing-section fallback every pair is perceivable with `distance =
  None` — the biconditional is scoped to the section-present case.
- [ ] AC6 — The query returns `CouplingTuple { perceivable, distance, aperture }`; `aperture`
  holds the fully-open sentinel in v1; adding the aperture pass later changes the sentinel's
  value only, not the struct's fields (no consumer signature churn).
- [ ] AC7 — `xtask observe` / `--headless` with a `cell_visibility` dump emits the relation as JSON:
  one entry per unordered off-diagonal pair with `cell_a < cell_b` that is **perceivable** (the
  informative set — proportional to the relation, not N²; diagonal omitted; non-perceivable pairs
  absent), sorted ascending by `(cell_a, cell_b)`. `distance` serializes as its integer or `null`;
  the fully-open `aperture` sentinel serializes as the fixed literal `"fully_open"`. Two identical
  runs are byte-identical.
- [ ] AC8 — (Review/grep gate, not a runnable test.) The query API names only cell types (`CellId`,
  cell-count). No `Player`, `Entity`, `ClientId`, `Sound`, `Projectile`, or relevance/audible/
  cull-distance parameter appears in the bake or query signature; `crates/level-loader` (the query's
  home) gains no dependency on net/audio/gameplay crates — it stays a leaf-facing data crate.
  Verified by signature grep + a `Cargo.toml` diff, not an assertion.
- [ ] AC9 — (Review gate, not a runnable test — already satisfied in `research.md`
  §Generalizability paper-check.) The second-consumer paper-check (network relevance + audio PAS
  both expressible via the CellId query plus consumer-side policy) is recorded in `research.md`
  before merge.
- [ ] AC10 — Two compiles of the same fixture produce byte-identical `CellVisibility` section
  bytes (bake is deterministic; no HashSet iteration order leaks into output; no wall-clock value
  branches the bake).
- [ ] AC11 — `--pvs-fast` bakes the reachability floor instead of the sightline flood; the result
  is conservative (every true sightline pair is still present) and the flag choice is the only
  difference — deterministic given the flag.
- [ ] AC12 — The chain-depth cap bounds flood depth; on a synthetic deep-chain fixture (an
  in-memory portal-graph fixture built in-test, not a new `.map`), pairs past the cap are
  conservatively included (zero false negatives preserved), and the bake completes without unbounded
  recursion.
- [ ] AC13 — (Wiring gate — stage duration is printed via the Build Summary, not exposed as a
  runtime metric; provable by the stage-contract test confirming `CellVisibility` is in
  `ORDERED_STAGES` and the stage calls `finish_stage`, or by a capturing-`Reporter` test asserting a
  `CellVisibility` timing entry.) The PVS bake stage logs its duration through the Build Summary
  path, so the cost is visible on the first real-map compile.

## Tasks

### Task 1: Thin slice — section format, placeholder bake, loader, query API

Stand the full pipe end to end with a placeholder relation, to falsify the boundary assumptions
before the hard algorithm lands. Add `SectionId::CellVisibility = 46` and its `from_u32` arm
(`crates/level-format/src/lib.rs`; do not reuse the retired id 14). Author the section module
(mirror `cell_locator.rs`): a `CellVisibilitySection` with `CELL_VISIBILITY_VERSION: u32 = 1`,
`to_bytes`/`from_bytes` (bounds-checked, rejects truncation and trailing bytes), carrying the
complete v1 wire layout from the start so later tasks fill data into a fixed format, never re-cut
it. Wire constraints (task agents do not receive the Wire-format section — restated here): little
endian, u32 counts (mirror `cell_locator.rs`); a leading `version: u32` (`CELL_VISIBILITY_VERSION`,
rejected on mismatch) then `cell_count: u32` (must equal the map's cell count; reject `0`); a
per-cell RLE-compressed perceivable bitset row over `0..cell_count` with per-row offset/length
framing so a reader can seek a cell's row (diagonal bit set; a cell with no perceivable neighbours
still emits a valid near-empty row); and a count-prefixed, ascending-sorted `(cell_a, cell_b,
distance)` distance side-table with `cell_a < cell_b`, present only on perceivable off-diagonal pairs
(empty encodes as count `0`). Pin the distance fixed-point scale as a named `pub const` in the
section module (fractional bits + representable maximum from the map world-bound budget, e.g.
1/256-unit → max ≈ 16.7M units) so Task 3 reads it from source, not from prose it never receives. Add a bake stage after `encode_vis` in `crates/level-compiler/src/pipeline.rs`,
modeled on `cell_draw_index_bake`, consuming `result.tree` (or `vis_result.leaves_section`),
`generated_portals`, and `exterior_leaves`. Like `cell_draw_index_bake`, the stage runs early
(just after `encode_vis` / `BvhBuild`) and emits pre-serialized bytes that are held and handed into
`pack_and_write_portals` at the later Packing stage as a new optional-section argument — the inputs
stay in immutable scope across the intervening bake stages, they do not run adjacent. Registering
the bake as a named pipeline stage adds `StageId::CellVisibility` to `ORDERED_STAGES` and its
`label` / `progress_label` arms, and updates the pinned
`planned_stage_contract_pins_order_labels_and_sdf_prediction` test (stage count 22→23, the label
vector, and the `SdfAtlasBake` ordinal, which shifts from `[19]` to `[20]`). Bracket the bake with
`begin_stage`/`finish_stage` so its duration prints in the Build Summary (AC13) — the placeholder
bake is timed here and Task 2's real flood inherits the same bracketed stage, so no run-loop stage
lists a stage that never begins/finishes. In this task the bake fills
`perceivable` with **portal-graph reachability** (a BFS flood over the portal graph — conservative,
a valid over-include floor). This reachability path is **retained, not throwaway**: Task 2 builds
the sightline PVS as the default on top of it, and this same flood remains the `--pvs-fast`
fallback and the past-the-depth-cap degradation target (see Task 2). The distance side-table is
left empty here. Wire the loader (`crates/level-loader/src/prl_loader.rs`): read the
optional section, lower it, and store it on `LevelWorld` as `Option<CellVisibility>`; `None`
means the conservative fallback. Expose the query on `LevelWorld` (a new
`crates/level-loader` submodule), co-located with the existing `locate_cell` / `cell_count`
CellId-query precedent, CellId-only: `perceivable(a, b) -> bool` (fallback all-true when the
section is absent) and `coupling(a, b) -> CouplingTuple` returning
`{ perceivable, distance: None-in-slice, aperture: fully-open sentinel }`. `cell_count` comes
from `LevelWorld::cell_count()`. Tests: bake→load→query round-trip on a compiled fixture, and a
missing-section fallback test. Plumbing: the new pack argument is threaded from the bake stage
through `pack_and_write_portals`; the `LevelWorld` field is populated in the `LevelWorld { … }`
construction inside `load_prl` (`prl_loader.rs`), alongside the other `convert_*`-lowered sections
(there is no `LevelWorld::new`; `new_visibility_only` is a partial constructor off the load path —
set the new field to `None` there, and in any other `LevelWorld { … }` literal the compiler flags).

### Task 2: Perceivable — conservative sightline PVS

Replace Task 1's reachability placeholder with the real conservative sightline PVS: the
anti-penumbra separating-plane portal flood (Teller 1992 §4; Quake `vis` `ClipToSeperators` /
`FindPassages`). For an ordered portal chain from a source cell, build separating planes between
consecutive portals (a plane through an edge of one portal and a vertex of the next, normal
oriented so the near portal is on the front side, kept only if all of the far portal is also
front-side), intersect the anti-penumbra wedges along the chain, and Sutherland–Hodgman-clip the
target portal polygon against the running wedge stack; non-zero clipped area means the target
cell is perceivable from the source. Use double precision (`glam::DVec3`) throughout, narrowing
to storage precision only at emit; reject slivers below a small area epsilon. The floor is
computed per source cell over its reachable portal chains and written as that cell's bitset row.
Enforce symmetry by unioning both directions (`perceivable(a,b) = flood(a→b) OR flood(b→a)`) —
the separating-plane construction is asymmetric at the epsilon, so do not assume the two
directions agree. Run the union as a serial post-join pass over the fully-populated raw matrix
(`perceivable[a][b] = raw[a][b] || raw[b][a]`); no per-cell flood task reads another cell's row, so
there is no cross-row race and `||`'s commutativity makes the result order-independent (see
Determinism pin P2). Solid cells (no portals) are perceivable only on their own diagonal. The
algorithm operates purely on the portal graph — no BVH / brush raycast, because solid geometry is
already represented as portal absence (`generate_portals` only emits portals between two non-solid
leaves). Structure the flood core as a pure function over portal-graph inputs —
`(cell_count, &[Portal])`, plus per-cell volumes (AABBs) where the AC4 oracle needs them — with the
pipeline stage adapting `result.tree` / `vis_result.leaves_section` / `generated_portals` down to
that signature. Do not take `&BspTree`: the portal-graph signature is what keeps the AC4/AC12
in-memory fixtures (a synthetic `Vec<Portal>`, no `.map`→BSP compile) buildable. This is the only
task that needs the separating-plane math; the invariant contract
(conservative, zero false negatives) is what the AC gates on, so any conservative sightline
construction that beats reachability satisfies it.

Run the per-source-cell flood in parallel with a `rayon` order-preserving parallel iterator
(`into_par_iter()` as in `sh_bake.rs`; `lightmap_layer.rs` uses `par_iter()`), each task taking a
`control.governor().enter()` permit at its outer boundary — the exact shape the SH and lightmap
bakers use, so the `--jobs` cap and cooperative pause come for free. Collect the per-cell rows
index-aligned to cell id via an order-preserving collect
(`into_par_iter().map(compute_row).collect::<Vec<_>>()`, whose `IndexedParallelIterator` collect
preserves input order), never a completion-order push into a shared buffer; serialization then emits
rows `0..cell_count` in that fixed index order (AC10; see Determinism pin P1).

Three scale guardrails, all determinism-preserving (AC10 requires byte-identical recompiles, so no
wall-clock-based branching — a timing-triggered degrade would make output machine-load-dependent):
- **`--pvs-fast` flag** — bake the Task 1 reachability floor instead of the sightline flood, for
  fast iteration builds. Explicit author choice; deterministic given the flag. Conservative
  (reachability ⊇ sightline), so a `--pvs-fast` map is correct, just looser. Plumbing: add
  `--pvs-fast` in `main.rs`; it is a distinct string from the retired `--pvs` (the test
  `parse_args_pvs_flag_rejected` matches `--pvs` exactly), so that rejection can remain — just don't
  reuse the bare `--pvs` name.
  Thread the `--pvs-fast` selection and the chain-depth-cap constant from arg-parse into the
  pipeline's build-options / config struct — the same carrier `--release` and the size options ride
  to their stages — so the bake stage in `pipeline.rs` can read them.
- **Recursion / chain-depth cap** — the flood stops refining past a fixed portal-chain depth and
  conservatively includes everything reachable beyond it (a localized reachability fallback for the
  deepest, most expensive chains). Deterministic and zero-false-negative — it only loosens tightness
  on very deep chains, bounding worst-case work on adversarial collinear-portal topology. This
  replaces the anti-penumbra draft's wall-clock "3× BFS auto-degrade," which would break AC10.
- **Stage-timing log** — no wiring here: Task 1 already brackets the stage with
  `begin_stage`/`finish_stage` (`pipeline.rs` / `reporter.rs`), so the real sightline flood's
  duration prints in the Build Summary automatically (AC13), self-validating the cost estimate on
  the first real-map bake. Keep the flood free of wall-clock branches — timing is diagnostic only.

Tests: conservative vs. the sampled ground-truth oracle (AC4) on small in-memory portal-graph
fixtures; symmetry; strictly-tighter-than-reachability on the `occlusion-test.map` occluder fixture
(AC3); `--pvs-fast` yields the reachability floor and stays conservative; the depth cap bounds chain
depth while preserving zero-false-negative (a synthetic deep-chain fixture, built in-test as an
in-memory portal graph, stays conservative past the cap). The oracle small fixtures and the
deep-chain fixture are constructed in the test module — no new `.map` authoring.

### Task 3: Distance — Dijkstra over the portal graph

Fill the `distance` side-table. Run Dijkstra over the portal graph to get the shortest path
length between perceivable cell pairs, keyed by ordered pair `(min, max)`, stored only on
perceivable off-diagonal pairs (the perceivable minority — never an N² matrix). Pin the edge
metric explicitly so nobody later "fixes" it as a bug: nodes are portals represented by their
polygon centroid, edge weight between two portals sharing a cell is the Euclidean distance
between their centroids, and a cell's own endpoints contribute cell-center→portal-centroid
segments; the result is a path-length coupling-quality key, not geometric ground truth. Store at the
Task-1-pinned fixed-point scale — read it from the named `pub const` Task 1 exposes in the
`cell_visibility` section module (Task 3 receives neither Task 1's paragraph nor the Wire-format
section); assert every path length fits the representable range and clamp-with-warning at the u32
maximum rather than wrapping. Make the relaxation deterministic
(AC5): build adjacency as `Vec`s in a fixed portal order and key the frontier on `(cost, node_id)`
(total order; node id breaks equal-cost ties) so float last-ULP accumulation differences cannot flip
the stored value; no HashMap/HashSet iteration participates in relaxation order (see Determinism pin
P5). On the symmetrization tie-break or any merge, err toward more coupling (**min distance**),
which is commutative over the two deterministic per-direction values. Populate `CouplingTuple.distance` from the side-table (`Some` on perceivable
pairs, `None` elsewhere); `aperture` stays the fully-open sentinel. Depends on Task 2 — distance
is defined only on the perceivable set it produces. Tests: `distance` present exactly on
perceivable off-diagonal pairs; symmetric; matches an independent shortest-path oracle on a
fixture (a same-metric oracle validates the relaxation and determinism, not the metric choice
itself — the edge metric is a pinned design decision, not a testable ground truth); deterministic.

### Task 4: Observability dump

Add a `cell_visibility` dump option to the headless runner so the baked relation is inspectable
as deterministic JSON. Extend `DumpSpec` (`crates/postretro/src/observability/runspec.rs`) with
the option; add a record type + `OutputDocument` field in `observability/document.rs`; widen
`build_output_document` to also receive the loaded relation (the driver has `world` in scope, so
the `driver.rs` call-site change is trivial — this is the one signature the dump needs beyond the
registry). Emit the relation as a JSON array of `{ cell_a, cell_b, perceivable, distance,
aperture }` entries — one per unordered off-diagonal pair `cell_a < cell_b` that is perceivable
(the informative set, proportional to the relation not N²; diagonal omitted; `perceivable` retained
for schema stability, always `true` in v1's emitted set), `distance` as its integer or `null`, the
fully-open `aperture` sentinel as the fixed literal `"fully_open"` — **pre-sorted ascending by
`(cell_a, cell_b)`** by the producer, because `to_deterministic_json` sorts object keys but leaves
arrays in data order (see Determinism pin P7). Consumes only Task 1's
loaded struct + query, so it is correct regardless of whether Task 2/3 data is placeholder or
final — the determinism AC holds either way. Test: two identical `--headless` runs over a
compiled fixture produce byte-identical stdout.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the index-space, section round-trip,
optional-fallback, and query-neutrality assumptions before the algorithm lands.
**Phase 2 (concurrent):** Task 2 (perceivable, compiler bake — the plan's heaviest: the
separating-plane flood plus the three scale guardrails) ‖ Task 4 (observability crate) — disjoint
files; both consume Task 1's fixed format and query surface.
**Phase 3 (sequential):** Task 3 — consumes Task 2's perceivable set (distance is defined only on
perceivable pairs).

## Wire format

New PRL section `CellVisibility`, id 46, **optional** (absent → conservative fallback, not an
error). Mirrors the little-endian, u32-count conventions of `cells.rs` / `cell_locator.rs`.
Constraints (layout offsets are the implementer's, per the constraints-not-solutions rule):

- Little-endian throughout. Leading `version: u32` (`CELL_VISIBILITY_VERSION`, rejected on
  mismatch), then `cell_count: u32` (must equal the map's cell count; reject `0`).
- **Perceivable floor** — one RLE-compressed bitset row per cell over `0..cell_count` (Quake-vis
  style: full rows for fast lookup, not upper-triangular). Row layout carries its own offset/length
  framing so a reader can seek a cell's row. Diagonal bit set. A cell with no perceivable
  neighbours still emits a valid (near-empty) row.
- **Distance side-table** — a count-prefixed, ascending-sorted list of `(cell_a, cell_b, distance)`
  triples with `cell_a < cell_b`, present only for perceivable off-diagonal pairs; `distance` as u32
  fixed-point at a scale **pinned in Task 1** — the fractional bits and representable maximum are
  fixed once (chosen from the map world-bound budget, e.g. 1/256-unit → max ≈ 16.7M units), so Task
  3 fills a genuinely fixed format and never re-cuts it. Task 3 asserts every path fits and
  clamps-with-warning at u32 max rather than wrapping. Empty list (v1 Task-1 placeholder, or a map
  with no perceivable pairs) encodes as count `0`.
- No `aperture` column in v1. Added at a `CELL_VISIBILITY_VERSION` bump when the audio consumer
  lands; the runtime `CouplingTuple` already carries the field at a fully-open sentinel, so no
  runtime type changes then.
- The section is a recompile-everything artifact; determinism (AC10) requires the bake to emit
  rows and the side-table in a fixed order independent of HashSet/HashMap iteration.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Conservative — `perceivable` never omits a real-sightline pair (zero false negatives) | Task 2 (sightline flood) | Task 3 must not drop pairs; `--pvs-fast` and past-the-depth-cap fallbacks are reachability supersets, so conservative by construction | AC4, AC11, AC12 |
| Symmetric — `perceivable(a,b)==perceivable(b,a)`, `distance` symmetric | Task 2 (union both flood directions), Task 3 (min over directions) | Side-table stores one `(min,max)` entry; query canonicalizes the pair | AC2, AC5 |
| `perceivable` is sole cull authority; graded axes modulate only, never gate | Task 3 (`distance` defined only on perceivable pairs) | Consumers/tests must not hard-cull on a scalar threshold | — (designed-for; no consumer wired in this build, so no verifying AC — enforced by consumers in their epics) |
| Reconciliation errs toward more coupling (min `distance`) | Task 3 | Symmetrization tie-break and any future approximation merge | AC5 |
| Optional section → conservative fallback (all perceivable) | Task 1 (loader `None` path) | Loader default must be all-true, never all-false | AC1 |
| Deterministic bake + dump (byte-identical) | Task 1 (fixed emit order), Task 4 (pre-sorted pairs) | HashSet/HashMap iteration order in bake or dump; parallel flood reassembly order (P1); symmetrization read-before-write (P2); Dijkstra frontier tie-break (P5); no wall-clock branch in the flood (depth cap, not timing, is the scale guard) | AC7, AC10 |
| PVS stage cost is observable | Task 1 (stage bracketed with `begin_stage`/`finish_stage`; Task 2's flood inherits it) | Timing must not branch output (diagnostic only) | AC13 |
| CellId-only neutral query surface | Task 1 (query API) | Any consumer wiring or API addition | AC8, AC9 |

## Determinism & ordering pins

This build has no runtime mutable state, timer, or event surface (the query is load-once,
read-only), and the compiler bake reads only values that are final and immutably borrowed at its
insertion point (verified against `pipeline.rs`). The only ordering hazards are
determinism-under-parallelism in the bake and format-frozen-before-algorithm. Each row below is
concrete enough to write a test from; the task tests reference the pins rather than restating the
mechanism.

| Pin | Scenario | Ordering the bake must fix | Expected outcome (AC) |
|---|---|---|---|
| P1 | Same fixture, two compiles; per-cell flood tasks finish in different completion orders | Rows reassembled index-aligned to cell id via order-preserving collect; serialize rows `0..cell_count` in index order — never a completion-order push | Byte-identical `CellVisibility` bytes (AC10) |
| P2 | Flood for cell `a` needs `raw[b][a]` while `b`'s flood is unfinished | Symmetrization is a serial post-join pass over the fully-populated raw matrix; no task reads a sibling row mid-flood | No race; `perceivable(a,b)==perceivable(b,a)`; identical every run (AC2, AC10) |
| P3 | Perceivable pairs discovered in arbitrary order before serialization | Rows in fixed cell order; side-table sorted by `(cell_a,cell_b)`, `cell_a<cell_b`; no HashSet/HashMap iteration reaches output | Byte-identical section (AC10) |
| P4 | Large map whose longest portal path exceeds Task 1's fixed-point range | Task 1 freezes distance scale + representable maximum from the world-bound budget; Task 3 asserts fit, clamps-with-warning at u32 max | Task 3 fills the frozen format with no re-cut; overflow is a loud diagnostic, not silent wrap (AC5) |
| P5 | Two `a→b` paths equal to last-ULP under different float accumulation orders | Deterministic `Vec` adjacency in portal order; frontier keyed `(cost, node_id)`; symmetrize as `min(dist(a→b),dist(b→a))` | Identical stored fixed-point distance across recompiles (AC5, AC10) |
| P6 | Task 1 adds `StageId::CellVisibility` to the stage list | `ORDERED_STAGES`, `label`/`progress_label`, and `planned_stage_contract_pins_order_labels_and_sdf_prediction` (count 22→23, label vector, `SdfAtlasBake` ordinal) updated together | Build Summary shows the PVS duration (AC13); stage-contract test green; `.prl` bytes identical across runs (timing contributes no bytes) |
| P7 | Task 4 dump over a fixture compiled with Task 1 placeholder vs. Task 2 final data | Dump producer emits perceivable pairs in fixed `(cell_a,cell_b)` order, independent of the underlying set's iteration order | Two `--headless` runs byte-identical regardless of placeholder-vs-final content (AC7) |

## Rough sketch

- **Types.** `CouplingTuple { perceivable: bool, distance: Option<u32>, aperture: Aperture }`
  with `Aperture` carrying an explicit fully-open sentinel constructor. Runtime `CellVisibility`
  holds the lowered bitset + side-table; `LevelWorld` gains `cell_visibility:
  Option<CellVisibility>` and the `perceivable` / `coupling` accessors, in a new
  `crates/level-loader` submodule beside the `locate_cell` / `cell_count` precedent.
- **Bake stage** in `crates/level-compiler`, template `cell_draw_index_bake`: consumes
  `result.tree` / `vis_result.leaves_section.leaves`, `generated_portals` (leaf-keyed ==
  cell-keyed by the identity mapping), `exterior_leaves`; emits pre-serialized bytes into
  `pack_and_write_portals`. Separating-plane helpers (`separating_planes`,
  `clip_polygon_against_planes`, `polygon_area`) liftable in shape from
  `drafts/perf-anti-penumbra-pvs` — the algorithm only; its identifiers are dead (see
  `research.md`).
- **Exterior/solid cells.** Valid CellIds. Solid cells (no portals) are diagonal-only. Exterior
  cells participate through their portals; no special-casing in the relation keeps it neutral.
- **Extension-in-place accepted.** `pack.rs`, `prl_loader.rs`, and `pipeline.rs` are already
  multi-thousand-line files the project extends in place (e.g. `cell_draw_index_bake` lives in
  `pipeline.rs`); do not split them as part of this plan. Extend along the existing section/stage
  seams. (`observability/document.rs` is small and needs no splitting — Task 4 still extends it.)

## Open questions

- **RLE row format specifics** (run encoding, per-row framing) are left to Task 1's implementer
  within the wire-format constraints; flagged only so review confirms the chosen encoding stays
  deterministic and seekable.
