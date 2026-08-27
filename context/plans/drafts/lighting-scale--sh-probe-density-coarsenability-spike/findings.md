# SH Probe Density Coarsenability — findings

Build-to-learn spike. Deliverable is a decision, not a shippable feature.

> **Read this when:** authoring the measurement-first adaptive-SH-probe-density v2 spec,
> or judging whether adaptive SH density is worth building. This note is the evidence that
> v2 was gated on.
> **Key finding:** adaptive SH probe density pays, and the payoff grows with intentional
> lighting. The right density predictor is **light contribution** (cone + falloff + facing),
> not distance-to-light — which is why the archived surface-distance classifier failed.
> **Related:** `context/plans/drafts/lighting-scale--adaptive-sh-probe-density/` (superseded
> draft this evidence feeds) · `context/research/archived-plans/lighting-scale--adaptive-base-probe-density/`
> (stopped; its distance-only classifier is the thing this note refutes) ·
> `context/plans/done/lighting-scale--sh-base-atlas-at-rest-slimming/` · `context/lib/experimental_spikes.md`
> · `context/plans/drafts/lighting-scale--cold-bake-reaching-light-spike/out-of-scope-findings.md` §4.

## TL;DR

The `--sh-analyze` coarsenability pass, run at dense 1.0 m probe spacing across three fixtures
of increasing lighting intentionality, gives a clear result:

1. **Adaptive density pays, and the payoff scales with lighting structure.** Coarsenable brick
   fraction at a near-lossless threshold rose **18.5 % → 43.9 % → 83.8 %** from a uniform warren
   to dispersed point lighting to theatrical spot lighting. Projected SH size fell to **0.22×** on
   the theatrical map. The style the engine most wants to serve is the style adaptivity helps most.
2. **The density predictor must be contribution-aware, not distance-aware.** On aimed spots,
   in-cone bricks never coarsen; at matched distance, out-of-cone bricks carry 6–29× less error.
   Angle-off-cone-axis predicts error far better than distance (**r = −0.765 vs −0.355**). A
   distance-only halo — the archived classifier — over-protects the dark side of every spot.
3. **The coarsenability ceiling is set by receiver composition, not the indirect bounce.** The
   base indirect field is near-losslessly coarsenable everywhere on all three maps. Composition
   through the receiver (base + direct + delta SH) manufactures the error, **~5–8×**, rising with
   theatricality.
4. **The dense bake is feasible.** After the cold-SH falloff early-out, a 1.0 m bake peaked at
   2.0 GiB (uniform warren) and under 0.32 GiB on the two smaller maps — well under the 16 GB box.

**Recommendation — one contribution-aware strategy, sequenced storage-first.** The v2 spec should
adopt a single contribution-aware density mechanism, not a per-room strategy chosen by dominant
light type (rejected below). Ship the measure-then-coarsen storage win first — it is already
validated and light-type-blind. Add a forward contribution predictor for the bake-time win second.
Expose creative control as an author fidelity threshold, not compiler light classification.

## What was measured, and how

`--sh-analyze` (`crates/level-compiler/src/sh_analyze.rs`) is an output-preserving pass: it changes
no emitted `.prl` bytes. Per 4×4×4 base-probe brick it classifies the coarsest storable level whose
**composed-receiver** reconstruction error stays under a threshold, and writes an `AnalysisReport`
JSON. Three stored levels: L0 (all 64 base probes, dense ground truth), L1 (8 corner probes,
trilinear), L2 (single brick-mean tile). The pass already existed; the spike only ran it.

Each fixture baked at dense 1.0 m spacing, coarse lightmap, cold cache:

```
prl-build <map> -o <out>.prl --sh-probe-spacing 1.0 --lightmap-density 1.0 --no-cache \
  --sh-analyze --sh-analyze-out <out>.sh-analysis.json --no-tui
```

Config notes, load-bearing for reading the numbers:

- **Lightmap density is irrelevant to the SH numbers** — the SH stage is independent of it. 1.0 m
  (coarse) keeps the lightmap and shadowmask tail cheap so the run reaches and completes the SH stage.
- **`--no-cache` without `--release` is approximate-indirect.** Absolute error magnitudes would shift
  under an exact (`--release`) bake; the cross-fixture *contrast* is the valid signal, not the absolute
  values. Exact-indirect confirmation is an open question for v2.
- Artifacts (`.prl`, analysis JSON, logs) were written to scratch and are not retained. Re-measurement
  is a re-run of the command above — no committed harness, because the pass is already in-tree.

## Fixtures

Chosen to bracket lighting intentionality, not map size. Size is incidental; lighting structure is
the variable.

| Fixture | Static baked lights | Lighting character |
|---|---|---|
| `stress-warren-mini` | 55 (39 point, 16 spot) | uniform — one fixture per room, all near one height. Floor case. |
| `campaign-test` | 13 (11 point, 2 spot) | dispersed points across open, partly-dim spaces. |
| `kinematic-platform` | 19 (4 point, 15 spot) | theatrical — ceiling spots aimed down, 10°/45° cones, dark between pools. |

Dynamic lights do not bake and are excluded. All three are dev maps, not shipping content — the
mechanism conclusions are content-independent; the magnitudes are a floor, not a projection.

## Measured finding 1 — coarsenability tracks lighting structure

Coarsenable = brick stored at L1 or L2 rather than dense L0. `ratio` = projected SH size vs the dense
uniform baseline. By the read-equivalence in the superseded draft's Goal, the delta portion of that
projected size is also projected per-frame compose traffic — so this is a storage *and* bandwidth number.

| threshold | mini (uniform) | campaign (dispersed) | kinematic-platform (theatrical) |
|---|---|---|---|
| 0.02 (near-lossless) | 18.5 % · 0.51× | 43.9 % · 0.445× | **83.8 % · 0.222×** |
| 0.10 (perceptual) | 41.2 % · 0.39× | 75.1 % · 0.256× | **88.8 % · 0.164×** |
| 0.25 (loose) | 67.4 % · 0.25× | 88.6 % · 0.136× | 89.0 % · 0.161× |
| per-brick error median | 0.28 | 0.081 | **0.017** |
| below 0.02 | 8.3 % | 37.1 % | **62.9 %** |

The theatrical map is near-fully coarsenable at the *strictest* threshold and barely moves after —
its field is mostly low-frequency with a small, sharp lit minority. The uniform warren never reaches
that plateau: dense-packed identical lights leave no low-variation volume to coarsen. Adaptivity's
value is a function of how much of the scene is dark or evenly lit, which intentional lighting
maximizes.

## Measured finding 2 — the predictor is contribution-aware, not distance

`kinematic-platform` isolates aim. Its 15 spots are ceiling-mounted, aimed straight down, inner cone
10° / outer 45°.

- **Distance predicts weakly.** Pearson r(distance-to-nearest-light, error) = **−0.355** — the same
  ~−0.35 seen on `campaign-test` (−0.352). Error does fall with distance (mean 4.18 at 2–4 m → 0.014
  beyond 24 m), but distance alone treats the whole neighborhood of a spot as dense-worthy.
- **Aim predicts strongly.** Split by angle off the spot's axis, at matched distance (6–10 m band):
  in-cone bricks mean error **3.62, 0 % coarsenable**; out-of-cone bricks mean error **0.126, 22 %
  coarsenable**. In-cone bricks never coarsen in any distance band. r(angle-off-axis, error) =
  **−0.765** among bricks within 6 m — more than double the distance correlation.

**A distance-only classifier over-protects.** Near each spot, ~16 in-cone bricks need density and
~198 out-of-cone bricks do not; distance keeps all ~214 dense. This is the concrete failure mode of
the archived `adaptive-base-probe-density` surface/distance classifier. The correct predictor is
proximity to *delivered light* — cone + falloff + facing — which is the same fuller contribution test
the cold lightmap already applies and the SH cull still lacks
(`cold-bake-reaching-light-spike/out-of-scope-findings.md` §4). One mechanism serves both.

Distance is not a rival predictor to discard — it is the degenerate case of contribution for an
omnidirectional light. A point light is a spot with a 360° cone. Evaluating delivered irradiance
handles points and spots with one function; the cone term is identity for points.

## Measured finding 3 — composition sets the coarsenability ceiling

The base indirect bounce coarsens almost losslessly on every fixture (base-L2 mean error ~0.005).
The error that forces density appears only after composition through the receiver (base + direct +
delta SH):

| fixture | base-L2 mean | composed-L2 mean | amplification |
|---|---|---|---|
| mini | 0.0154 | 0.0923 | ~6× |
| campaign | 0.0052 | 0.0322 | 6.2× |
| kinematic-platform | 0.0054 | 0.0433 | 8.0× |

Aimed high-intensity spots amplify hardest. The implication for v2: the density budget is spent on
the sharp direct/delta composition near lit surfaces, not on the smooth indirect bounce. A scheme
that coarsened the base indirect volume alone would go much further (L2 atlas floor ~0.016×) than one
measured against composed-receiver quality — a real lever, but one that trades against final image
fidelity.

## Measured finding 4 — the dense bake is feasible

| fixture | probe grid | SH stage | peak RSS |
|---|---|---|---|
| mini | 125×18×223 (502 k probes) | 985 s | 2.0 GiB |
| campaign | 74×23×114 (194 k) | 26 s | 0.31 GiB |
| kinematic-platform | 23×65×89 (133 k) | 16 s | 0.29 GiB |

The cold-SH falloff early-out (shipped) is what makes the 1.0 m bake reachable. Memory is not the
constraint on a 16 GB box at these scales. Wall-clock on a large uniform warren (~16 min for the SH
stage) is the cost to watch as maps grow — and is itself an argument for the forward-prediction
(bake-time) win, not only the storage win.

## Recommendation — strategy for the v2 spec

**One contribution-aware mechanism, not per-room strategy selection.** A single density predictor
keyed on delivered light contribution is correct for every light type by construction. Building it
as a family of strategies chosen by a room's dominant light type is the wrong shape:

- **Rooms are mixed.** `campaign-test` (point-dominant) carried spots; `kinematic-platform`
  (spot-dominant) carried fill points. A theatrical room is a spot key + ambient fill + emissive
  panels. Coarsenability at a brick is set by the *union* of every light's contribution, not the
  dominant one. A brick dark to the key spot but caught by a fill point still needs density.
- **The granularity is wrong.** Strategy-switching decides per room; the physics decides per brick
  per light. The unified contribution predictor is already at brick granularity; a room classifier
  cannot reach it.
- **It generalizes for free.** Area lights, emissive surfaces, and directional/sun all have a
  contribution model. A contribution predictor absorbs them; a strategy table needs a new entry each.

**Sequence the two wins.**

1. **Measure-then-coarsen (storage) first.** `--sh-analyze` / `--sh-coarsen` already coarsen each
   brick by its *measured* composed error. This is strategy-free and light-type-blind — the identical
   pass produced all three results above. It delivers the storage/bandwidth win (~0.22× on the
   theatrical map) with no predictor at all. This is the validated, lower-risk core.
2. **Forward contribution prediction (bake time) second.** To also cut bake work, predict density
   before baking from the contribution field, so dense probes are skipped where coarse suffices. Only
   this needs a predictor — and it is the single contribution-aware function, not per-room strategies.

**Creative control is an author fidelity threshold, not compiler classification.** Different
sensibilities want different *fidelity*, not different *algorithms*. Expose the error threshold
per map, ideally with a per-region override — high-contrast theatrical work sets it tight to protect
sharp pools, soft ambient sets it loose. The author declares intent; the compiler does not infer taste
from light categories. Per `experimental_spikes.md`, such a lever lives first as a debug-tool slider,
not a shipped user setting.

## Considered and rejected — per-room strategy by dominant light type

Rejected on the evidence above: mixed rooms are the norm, coarsenability is set by the light union not
the dominant type, and room granularity cannot express a per-brick per-light contribution test. A
single contribution-aware predictor is simpler, correct on mixed lighting, and extensible to unseen
light types. Recorded here so a future reader sees it was weighed, not missed.

## Honesty and correctness notes

- **Output-preserving.** `--sh-analyze` changed no emitted bytes; every `.prl` is a standard bake with
  a JSON sidecar. No spike harness was committed — the pass is in-tree.
- **Approximate indirect.** All runs used `--no-cache` approximate indirect, not `--release` exact.
  The contrast across fixtures is the signal; absolute magnitudes are indicative. Exact-indirect
  confirmation is deferred to v2.
- **Synthetic fixtures are a floor.** Three dev maps, not shipping content. The mechanism conclusions
  (coarsenability concentrates with lighting structure; contribution predicts, not distance;
  composition sets the ceiling) are content-independent. The magnitudes are a floor — real levels with
  larger dark/empty volumes should coarsen more, not less.
- **Aim analysis caveat.** The tight-range in-cone sample is thin (ceiling spots light a floor 6–8 m
  below, pushing most in-cone bricks into the 6–10 m band). The angle correlation (−0.765, n = 214)
  and the 6–10 m split carry the conclusion.

## Open questions for the v2 spec

- **Exact-indirect confirmation.** Re-bake `kinematic-platform` with `--release`; confirm the
  concentration holds under exact lighting. ~30 s.
- **Real-content magnitude.** Unknown until content exists. The mechanism is content-independent; the
  ratio is not.
- **Forward-predictor efficacy.** Does a contribution-aware forward predictor recover most of the
  offline-classified savings? This is the bake-time win's gating measurement.
- **Scope split.** Does v2 target the storage win, the bake-time win, or both — and does it inherit the
  superseded draft's id-41 direct-delta framing or restructure around the base grid?
