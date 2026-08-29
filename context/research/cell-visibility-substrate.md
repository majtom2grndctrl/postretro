# Cell Visibility Substrate — Design Intent

> **Status:** The baseline substrate — `perceivable`/`distance`/`aperture` for every Cell pair,
> baked into PRL section `CellVisibility` (id 46) — shipped via
> `context/plans/done/cell-visibility-relation/`, built foundation-first ahead of a wired consumer
> (that plan's *Alternatives rejected* argues the divergence from this doc's original "build with
> the first consumer" guidance, still below). This doc now holds what's still unbuilt: the
> sightline/anti-penumbra tightening, the dynamic-geometry/destructible design, and the
> generalizability gate for the four intended consumers below, none of which reference the
> substrate yet.

## What it is

A **view-independent, Cell → Cell "potential perceptibility" relation**: for two Cells A and B, can
*any* point in A perceive *any* point in B along a sightline through the portal graph? Computed once
at bake over Cell portals, emitted as a PRL section, consumed at runtime as cheap O(1) lookups. The
floor is a conservative *sightline* PVS — does any sightline thread the portal sequence from A to B —
built by the **anti-penumbra** separating-plane construction (Teller 1992, §4; Quake `vis`
`ClipToSeperators` / `FindPassages`). Mere portal-graph *reachability* is not enough: portals keep a
level connected, so reachability barely culls. The sightline test is what makes the PVS useful, so it is
v1 work. What defers is the graded `aperture` *magnitude*, extracted from the same separating planes
later. The runtime query is a struct, not a bare bool — see the graded tuple below.

It is the **area-source, precomputed, view-independent** cousin of the runtime `narrow_frustum` cull
(`crates/visibility/src/portal_vis.rs`), which is **point-source, per-frame, viewer-dependent**.
The runtime frustum answers "what does *this camera* see *now*" (rendering); this relation answers
"can any observer in A perceive B, *ever*" (relevance / audio / simulation) — a question no per-frame
camera cull can answer, and precisely why the precomputed area-source math earns its extra cost
*here* and not at runtime.

Distinct from the archived `perf-anti-penumbra-pvs` draft, which tightened the *baked rendering* PVS
— a use the runtime narrowing frustum has superseded. **This substrate is not for camera rendering.**
It is for the non-camera consumers below.

## Intended consumers (build *with* the first real one)

- **Network relevance / interest management — E15 Phase 4.** Don't replicate entities/events whose
  Cell a client's Cell can't perceive. The canonical Quake/Source PVS-for-netcode use, and the most
  likely **first** consumer. Gated: Phase 4 marks it *"interest management via portal/PVS if needed"*
  at 16-player scale; Phase 2's component-type scoping already covers small-co-op relevance, so this
  bites only under measured host-upstream bandwidth pressure.
- **Audio occlusion / PAS — Epic 12.** Cull/occlude sounds from non-perceivable Cells; the baked
  audio-occlusion query the E10 LOS bullet already references.
- **Combat / projectile VFX culling.** Muzzle flashes, tracers, impacts from a distant firefight —
  cull the *presentation* when the firing Cell can't be perceived from the player's Cell.
- **AI perception broad-phase / sim-LOD.** A cheap gate before the exact eye-to-target LOS raycast
  (E10 "Enemy line-of-sight + cover"), and a throttle for enemies in imperceptible Cells. It plugs
  into the **target-selection seam** landed by `E10--enemy-mp-target-selection` — that spec's
  `select_target` chokepoint already accepts an injectable visibility predicate (and could later take a
  coupling *weight* rather than a bare predicate, to bias ranking by perceptual proximity).

## Correctness invariants (every consumer relies on these)

- **Conservative — zero false negatives.** The relation is *potentially* perceivable: it must NEVER
  report "cannot perceive" for a pair that actually can. This is what makes it SAFE to cull
  presentation / relevance (worst case: an unnecessary include, never a wrongly-hidden thing).
  Anti-penumbra tightening reduces false *positives*; it must preserve zero false negatives. It is also
  what lets a *baked* relation survive *dynamic* geometry: baking the most-open state is conservative for
  every intermediate state (see *Dynamic geometry*).
- **Symmetric — enforced, not asserted.** A perceives B iff B perceives A. But the FP anti-penumbra
  construction produces asymmetric pairs at the epsilon (a known Quake `vis` footgun), so populate each
  pair by *unioning* both directions rather than assuming they agree (detail in the graded hierarchy
  below). Store upper-triangular; consumers may rely on bidirectionality.
- **Presentation / relevance only — never simulation authority.** Consumers may cull render, audio,
  and replication-relevance. They must NOT cull authoritative simulation: the server still simulates
  the distant projectile / enemy (it may round a corner into view, or matter to another co-op
  player). Conflating "don't show" with "don't simulate" causes desync.

## Graded extension: the coupling tuple

The binary floor answers *can* A perceive B. Graded consumers ask *how strongly* — audio a few rooms off
should fade, not vanish; net wants a priority *ordering*, not just an include bit. The naive widening is a
single "relevance score." Reject it. The union of consumers argues for a small **orthogonal feature
tuple**, because the two things that make Cells more or less coupled — path *length* and path *openness* —
are physically distinct and consumers weight them differently. Every modern audio engine (Steam Audio,
Wwise, FMOD) models distance attenuation and obstruction lowpass as *separate* parameters; collapsing them
into one scalar silently bakes a distance-vs-aperture blend, which is policy. And "score" re-invites the
"relevance to *what*" leak the neutral vocabulary was kept clean of. So the graded output is a struct:

| Field | Meaning | Role |
|---|---|---|
| `perceivable` | the binary floor — does any sightline thread the portal sequence A→B | **cull authority** |
| `distance` | metric shortest-path length through the portal graph (Dijkstra) | modulation / priority |
| `aperture` | narrowest solid-angle constriction (bottleneck) along that path | modulation / obstruction |

Two graded axes is the *minimum* separating basis — one axis re-collapses length and openness. No third
axis: streaming lookahead is `distance × player-velocity`, computed consumer-side; a *directional*
incidence term is listener-position-dependent, hence not a view-independent Cell-pair property (its
view-independent seed — the bottleneck portal id — is runtime-derivable from the portal graph the camera
cull already walks, so keep it out). Consumers map the tuple onto their own curve: audio → attenuation +
lowpass, net → accumulator priority, AI → think-rate, VFX → pre-warm priority. Same tuple, consumer-side
curves — the generalizability contract holds unchanged.

**Storage: a floor bitset + a sparse side-table, never an N² scalar matrix.** `perceivable` is the
existence bit — a Quake-PVS-style per-Cell reachability bitset (RLE), the structure `vis` has shipped for
30 years. The two scalars hang off *present* pairs only (the perceivable minority), in a sparse side-table
keyed by pair. Scalars for all pairs would be O(Cells²); on perceivable pairs it is bounded by real
adjacency.

**`distance` is a broad-phase priority key, never a DSP attenuation parameter.** Runtime consumers already
have metric *Euclidean* distance from world transforms. What the substrate uniquely adds is the
*path-aware* length Euclidean distance gets wrong through walls. Use it to *order* and *throttle* (net
priority, AI think-rate, prefetch), not as a literal gain/delay. A long U-bend reads louder-than-physical
here — a safe over-couple; true around-corner attenuation is runtime audio-propagation's job. The per-Cell
edge weight (portal-representative to portal-representative) is a coupling-*quality* knob, not geometric
ground truth. Pick centroid-vs-edge-hugging explicitly, so nobody later "fixes" it as a bug.

**`aperture` is the bottleneck, not integrated attenuation.** The `min` constriction along the path is the
neutral openness hint; the consumer integrates it into its own dB/transmission curve. Baking integrated
attenuation would leak DSP policy — the aperture-side twin of the single-scalar collapse.

### Invariant hierarchy (supersedes flat per-field bounds)

1. **`perceivable` is the sole cull authority; it must be conservative — zero false negatives.** It is a
   *sightline* PVS (the separating-plane construction), not mere graph reachability — reachability is
   conservative but barely culls in a connected level, so it fails the consumer it is built for. The rule:
   never omit a pair that has a real sightline. Note `perceivable ⊆ reachable` — a pair can have finite
   `distance` yet no sightline, so `distance` is defined only on perceivable pairs.
2. **Graded axes modulate only; they never gate inclusion.** A consumer may throttle/attenuate by
   `distance`/`aperture`, but must gate on `perceivable` first and never hard-cull on a scalar threshold —
   dropping everything under a `coupling < k` reintroduces false negatives → desync.
3. **Reconciliation errs toward more-coupling.** Whenever two candidate values for a graded axis must
   merge, take the more-coupling one: **min `distance`, max `aperture`.** Two things force a merge:
   approximation error, and the symmetrization tie-break. The latter fires even for exact axes — the
   symmetric relation is enforced by *unioning* the two directions (the FP anti-penumbra floor is
   asymmetric), which yields two candidate scalars per pair. So this rule is load-bearing, not an
   approximation footnote. And where an axis is *approximated* (the aperture pass), "must not under-report
   openness" is a hard acceptance criterion on that bake task, not optional polish.

### Staging — the tuple grows with its consumers

- **v1 = `{ perceivable, distance }`**, shipped WITH net relevance (the likely first consumer). v1 does
  the sightline separating-plane construction for `perceivable` (the geometry that makes it cull) and
  Dijkstra over the portal graph for `distance`. `perceivable` *is* the PVS; its one-hop dilation over the
  adjacency graph *is* the PHS (potentially-hearable set) — so v1 already delivers Quake's PVS+PHS model.
  (PHS comes from the adjacency structure, not from thresholding `distance`: metric length and hop count
  are different orderings.) `distance` also gives the net accumulator a fully-ordered priority key, so it
  improves the *first* consumer directly, not just future audio.
- **+`aperture`** lands WITH audio (Epic 12) — the consumer that genuinely needs obstruction magnitude —
  extracted from the same separating planes v1 already computed for `perceivable`. A small add-on, not new
  geometry.
- **The consumer-facing struct is complete from day one.** Consumers code against
  `{ perceivable, distance, aperture }` in v1 with `aperture` sentinel-filled (fully-open), so no consumer
  signature churns when the aperture pass lands. Do **not** speculatively reserve on-disk PRL bytes: the
  section is a recompile-everything compile artifact under *move-fast-break-APIs*, so the aperture column
  is added at a plain version bump + recompile — free here. Spend the "pay now" budget on the runtime type
  shape, not the disk layout.

**One over-build to avoid:** projectile-light pre-warm ("light it before it rounds the corner") already
falls out of `perceivable` alone — a perceivable Cell includes the not-yet-in-frame cell around the
corner. The scalars only add *prioritization under budget* (warm highest-coupling cells first), not the
pre-warm itself.

## Dynamic geometry: destructibles and movers

Destruction changes what can be seen, heard, and replicated — a ceiling drops, a rock face opens a cave.
A *baked* relation survives this because the floor is conservative. Destruction almost always *opens* the
world, and a more-open state is *more* perceivable. So **bake the maximally-open configuration** (every
destructible removed) and the static relation is conservatively correct for the whole lifecycle. While a
wall is intact, its through-pairs are baked-perceivable but not yet real — a false *positive*
(over-include), which is safe. Graded axes baked open over-couple while intact (tier 3: err toward
more-coupling). No pair is ever wrongly hidden, in any intermediate state.

That is correctness, not culling. Baking open under-culls the common case — the wall is intact most of the
match. Restore the culling the way Source areaportals do: **record, per baked sightline, the destructibles
it crosses (a blocker mask); at runtime a pair is perceivable iff baked-perceivable AND all its blockers
are destroyed.** A cheap bitmask test — no snapshot swap, no recompute. It handles combinatorial
destruction (N independent breakables, no 2^N) because each sightline ANDs only the blockers it crosses; a
*closing* collapse is the same mask with opposite polarity. A destructible region is, in substrate
vocabulary, a **dynamic portal** — Cell/portal data, not gameplay — so this stays inside the contract.

**Storage sizes to the open envelope.** The side-table holds every open-envelope-perceivable pair; masking
hides the currently-blocked ones at runtime. So the table is larger than any single intermediate state,
and runtime tightness comes from masks, not a smaller table. Negligible for authored set-pieces (a few
dramatic destructibles); it would bloat for every-surface destruction (R6-Siege scale) — a scope limit
worth naming.

**Defer the hard parts to their real consumers.** Procedural / unauthored destruction (voxel, Geo-Mod —
openings the bake can't enumerate) needs genuine runtime recompute; a boomer shooter's scripted set-pieces
never do, so leave recompute behind the resolver seam for a mod that wants it. The runtime query widens
from `(a, b)` to `(a, b, portal-state)` when masking lands — a signature change paid *then*, with the
destructible epic as its consumer, not pre-threaded now (move-fast-break-APIs). Modders author the
*content* (destructible region + opened geometry + a script trigger firing a `region destroyed` event),
never a pre-bake-vs-recompute knob; the engine picks resolution.

## Generalizability contract — how to keep it consumer-agnostic

The design-systems risk is real: a substrate built *with* its first consumer tends to absorb that
consumer's policy and become un-reusable. Hold this line.

**The substrate knows only Cells.** Its entire vocabulary is `CellId` and the perceptibility
relation. It knows NOTHING about players, entities, clients, sounds, projectiles, bandwidth, or
radii. If the substrate's API mentions any of those, policy has leaked in.

**Consumer policy stays in the consumer.** The first consumer (say, network relevance) will want a
relevance *radius*, *include-owner*, *hysteresis* to avoid flip-flop, a *grace period* before
culling, per-*client* keying. **None of that belongs in the relation.** The substrate returns the raw
tuple for a Cell pair (`perceivable` + graded axes); the consumer maps its own domain (this client's Cell,
this entity's Cell) onto Cells, queries, and applies its own policy on top.

**Home it in a neutral crate.** The relation lives in a visibility/cell crate (e.g. `crates/visibility`
or a sibling), **not** in `crates/net`, `crates/audio`, or a gameplay crate. If it lives in the first
consumer's crate, it is already coupled.

**Smell tests for over-coupling (reject in review if any are present):**

- The bake or query API takes a `Player`, `Entity`, `ClientId`, `Sound`, or `Projectile` type.
  → It should take `CellId`.
- A "relevance radius" / "audible range" / "cull distance" parameter *on the substrate*.
  → Consumer policy that leaked in.
- The output is keyed by client-id or entity-id rather than Cell-id.
  → Consumer indexing leaked into storage.
- The substrate crate `use`s `net` / `audio` / a gameplay crate.
  → Dependency direction inverted. Consumers depend on the substrate, never the reverse.

**Validate against a SECOND consumer on paper before merge.** If network relevance is the first
consumer, sketch how audio PAS would call the same API (or vice-versa). If both fit using only
Cell-vocabulary + consumer-side policy, it's generalizable. If the second consumer would need a
*different* substrate API, refactor before merging the first.

## Bake-side contract — keeping the compiler output neutral

The runtime contract keeps the *query* consumer-agnostic. The bake needed the mirror, or the first
consumer's shape would freeze into the PRL — worse than a runtime leak, because it's baked. Precedent
is old and settled: Quake `vis` bakes **PVS and PHS** from one pass — the visible set (rendering) and
the hearable set (audio/events), where PHS is PVS dilated one portal-hop. One bake, multiple derived
sets; the coarser hearable set is the discrete ancestor of the graded coupling axes.

**Shipped as designed** — one neutral `CellVisibility` section (id 46) carrying `perceivable` +
`distance` + `aperture` together at full fixed-point precision (no consumer-side bucketing), reading
only Cell/portal geometry, with a conservative all-perceivable fallback when the section is absent.
Built and documented: `context/lib/build_pipeline.md` §PRL section IDs (id 46); code in
`level-compiler/src/cell_visibility_bake.rs`, `level-format/src/cell_visibility.rs`.

**One correction to the record:** `perceivable` shipped as connected-component portal-reachability,
not the sightline separating-plane construction this doc originally specified for v1 —
`context/plans/done/cell-visibility-relation/` made that call explicitly (see its *Alternatives
rejected*), leaning on the graded axes for discrimination instead. The sightline tightening itself
stays deferred, unbuilt design (*What it is* above, `perf-anti-penumbra-pvs`); if it lands, it is an
additive axis, never a redefinition of the now-shipped `perceivable`. Destructibles remain outside
the bake in practice, not just by policy — the cell-graph-and-portals-only input contract is shaped
to accept them as dynamic portals (*Dynamic geometry* above), but that consumption isn't wired.

## Build guidance

- **Build WITH the first real consumer, in one orchestration run** — not ahead of one, and not
  inline-then-extract. Draft one plan: **Task A** = the Cell-visibility relation (bake +
  PRL section + neutral-crate query), **Task B** = the first consumer, sequenced A → B. Add an
  explicit **generalizability-gate** AC: run the smell tests and the second-consumer paper-check
  before merge. This is the `orchestrate` shape that lands "generalizable substrate + first consumer"
  decomposed and de-coupled in a single pass — the intended payoff of building *with* rather than
  *after*.
- **Trigger is a *real* consumer need**, not a hunch: E15 Phase 4's measured bandwidth pressure
  ("if needed"), or Epic 12 opening. Do not build because a per-frame cull "looks expensive" — measure
  first. (This is the same discipline the shelved `perf-forward-light-cull` violated;
  `context/plans/done/perf-forward-light-cull/` has the post-mortem.)
- **Plug points already placed:** `E10--enemy-mp-target-selection` lands the `select_target`
  chokepoint the AI-perception broad-phase consumer slots into without re-touching the FSM.
