# Cell Visibility Substrate — Design Intent

> **Status:** design intent / forward-looking substrate — **NOT a ready spec.** Captures what the
> substrate is, its intended consumers, and — the point of this doc — **how to keep it generalizable
> and not couple it to whichever consumer lands first.** Build it *with* its first real consumer
> (draft-plan → orchestrate, one run), not ahead of one.

## What it is

A **view-independent, Cell → Cell "potential perceptibility" relation**: for two Cells A and B, can
*any* point in A perceive *any* point in B along a sightline through the portal graph? Computed once
at bake via the **anti-penumbra** separating-plane construction (Teller 1992, §4; Quake `vis`
`ClipToSeperators` / `FindPassages`) over Cell portals, emitted as a PRL section, consumed at runtime
as a cheap O(1) lookup — `can_perceive(a: CellId, b: CellId) -> bool`.

It is the **area-source, precomputed, view-independent** cousin of the runtime `narrow_frustum` cull
(`crates/visibility/src/portal_vis.rs:577`), which is **point-source, per-frame, viewer-dependent**.
The runtime frustum answers "what does *this camera* see *now*" (rendering); this relation answers
"can any observer in A perceive B, *ever*" (relevance / audio / simulation) — a question no per-frame
camera cull can answer, and precisely why the anti-penumbra area-source math earns its extra cost
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
  Anti-penumbra tightening reduces false *positives*; it must preserve zero false negatives.
- **Symmetric.** A perceives B iff B perceives A. Store upper-triangular; consumers may rely on
  bidirectionality.
- **Presentation / relevance only — never simulation authority.** Consumers may cull render, audio,
  and replication-relevance. They must NOT cull authoritative simulation: the server still simulates
  the distant projectile / enemy (it may round a corner into view, or matter to another co-op
  player). Conflating "don't show" with "don't simulate" causes desync.

## Graded extension: perceptual coupling (v2 — optional layer on the binary relation)

The binary relation answers *can* A perceive B. A graded quantity answers *how strongly* — audio a few
rooms off should fade, not vanish. Widen the substrate output from `bool` to `bool + a neutral scalar`:
**inter-cell perceptual coupling**, a view-independent measure of how open the sightline path from A to B
is — portal-hop count and aggregate aperture constriction along the tightest passage sequence. Pure
function of the Cell graph. It names nothing about listeners, players, or dB.

Keep it neutral. "Coupling," not "relevance" — *relevance to what* is consumer policy, and a scalar named
for one consumer re-leaks the coupling the binary relation was kept clean of. Consumers map coupling onto
their own curve: audio → attenuation, VFX → pre-warm threshold + priority, net → LOD tier / send-rate.
Same scalar, consumer-side curves — the generalizability contract holds unchanged.

**Invariant — conservative upper bound.** The binary relation stays the floor: `coupling > 0` iff
`can_perceive` is true. The scalar must never *under*-report coupling — under-reporting is the graded form
of a false negative (audio wrongly silent, a live entity wrongly de-prioritized). Same safety as the
binary case, one dimension richer.

**Continuous modulation only — not a cull threshold.** Grade audio volume, LOD, pre-warm priority freely.
Do **not** hard-cull below a scalar threshold on the relevance/authority-adjacent path: culling everything
under `coupling < k` reintroduces false negatives → desync. Hard culls respect the binary floor; the scalar
modulates within it.

**Build it with audio, not before.** Net relevance (likely first consumer) needs only the binary relation —
don't force the scalar into the minimal first build. Audio (Epic 12) is the consumer that *needs* magnitude;
the scalar lands when audio is the real consumer. Note one thing an implementer will otherwise over-build:
projectile-light pre-warm ("light it before it rounds the corner") already falls out of the *binary*
relation — a perceivable Cell includes the not-yet-in-frame cell around the corner. The scalar only adds
*prioritization under budget* (warm highest-coupling cells first), not the pre-warm itself.

## Generalizability contract — how to keep it consumer-agnostic

The design-systems risk is real: a substrate built *with* its first consumer tends to absorb that
consumer's policy and become un-reusable. Hold this line.

**The substrate knows only Cells.** Its entire vocabulary is `CellId` and the perceptibility
relation. It knows NOTHING about players, entities, clients, sounds, projectiles, bandwidth, or
radii. If the substrate's API mentions any of those, policy has leaked in.

**Consumer policy stays in the consumer.** The first consumer (say, network relevance) will want a
relevance *radius*, *include-owner*, *hysteresis* to avoid flip-flop, a *grace period* before
culling, per-*client* keying. **None of that belongs in the relation.** The substrate returns raw
`can_perceive(a, b)`; the consumer maps its own domain (this client's Cell, this entity's Cell) onto
Cells, queries, and applies its own policy on top.

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
