# Base-Density Forward-Predictor Spike — Findings & Recommendation

> Spike deliverable per `context/lib/experimental_spikes.md`: a **decision**, not a
> contracted threshold. This note is the hand-off to the foundational base-density
> (bake-time-win) spec's drafting session. Raw numbers, full sweeps, confusion
> matrices, and the build-profile note live in `research.md` (Task 3 section); this
> note is the recommendation and what it rests on.

## Recommendation: **NO-PROMOTE**

**Do not open the foundational base-density spec's one-way door (base-grid
`density_level` format change + renderer variable-density sampler) on the strength of
a cheap forward predictor. No measured predictor family clears both bars, and the
failure is structural, not a poverty of the cheap signals.**

The bake-time win requires deciding, *before* the base-indirect SH bake, which affinity
bricks can skip dense probes without receiver error. The composed-receiver oracle says
the base-indirect field is highly coarsenable almost everywhere (76 % of bricks on
occlusion-test, **97.6 %** on stress-warren-mini) — so the predictor's real and only
hard job is protecting the small dense-needed minority (24 % / **2.4 %** of bricks).
No measured forward predictor can identify that minority cheaply and safely.

## The two bars, and how each family did

**Cost bar — PASSED decisively by every candidate.** On the heavy realistic fixture
the base-indirect bake is **715.94 s (~12 min)**; predictor eval is a rounding error:
P1 0.218 s (**0.03 %** of the bake), P3 0.107 s (0.015 %), even the occlusion-augmented
P2 6.10 s (0.85 %). Cost is not why this fails.

**Accuracy bar — FAILED by every candidate, including the richest-possible ceiling.**
No family reaches a threshold that is simultaneously *safe* (near-zero unsafe
false-positives) and *useful* (frees a meaningful fraction of the coarsenable
majority). The failure looks different on the two fixtures but is the same failure:

| Family | occlusion-test — accuracy | stress-warren-mini — accuracy | bar missed |
|---|---|---|---|
| **P1** contribution-geometry | FP floors at **16.5 %** (132/802); never near-zero at any threshold | near-zero FP only by refusing to coarsen (recover 17 % @ FP 0.13 %); r=-0.068 (no signal) | **accuracy** |
| **P2** P1 + occlusion | FP floor 16.6 % (133) — occlusion does *not* shrink it, only raises recovered savings | same no-knee tradeoff; FP climbs with savings | **accuracy** |
| **P3** direct-field *ceiling* (not a real candidate — id 35 bakes AFTER id 34) | FP floor 16.6 % (133) — the richest signal floors at the **same** point as P2 | same no-knee tradeoff | **accuracy** (and ordering: not pre-bake) |
| distance (negative control) | "safe" (FP 4.6 %) only by recovering 13 % — refuses to coarsen | FP 0.01 % only by recovering 9 % | control — refuted |
| surface_distance (negative control) | essentially no signal (r=+0.026) | r=-0.134 | control — refuted |

### Why it is structural, not a cheap-signal artifact

1. **The P3 direct-field ceiling fails identically.** P3 reads the actual baked
   direct-at-probe field (id 35) — the richest signal in the building — and it floors
   at the *same* 133-brick FP as P2 on occlusion-test and shows the *same* no-knee
   FP-vs-savings tradeoff on stress-warren-mini. A richer forward input does not move
   the bar. (P3 is also not a real candidate: id 35 bakes *after* id 34, so it is never
   available pre-base-indirect-bake — measured, `p3_direct_baked_before_base_indirect:
   false`.)
2. **The confusion matrices show the miss is on the exact bricks the machinery would
   exist to protect.** occlusion-test P1 at its *safest* threshold predicts L2 (fully
   coarsen) for **131 of the 190 dense-needed L0 bricks** — 69 % of the dense regions
   under-baked before any savings tradeoff is even made.
3. **The "low FP" on stress-warren-mini is a denominator artifact.** With 97.6 % of
   bricks coarsenable, a predictor that mostly says "coarsen" is mostly right, so FP as
   a fraction of *all* bricks is small. But to free a meaningful fraction of the
   coarsenable majority the predictor must mis-coarsen essentially the entire
   dense-needed minority: recover 17 % -> FP 10 bricks; recover 87 % -> FP 251 bricks,
   already exceeding the whole 186-brick L0 population's worth of mis-leveling. There is
   no threshold with both near-zero FP and meaningful savings.
4. **Contribution-aware barely beats distance on these fixtures.** On the delta-indirect
   primary fixture P1/P2 correlate -0.198/-0.213 with the oracle vs distance's -0.169 —
   all far below the coarsenability spike's r=-0.765 for angle-off-cone-axis. P1's
   delivered-magnitude-*gradient* score is a much blunter contribution signal than raw
   angle-off-cone, and even adding occlusion (P2) or the full baked direct field (P3)
   does not recover the separation.

### The second fixture does not contradict the first — it confirms it

stress-warren-mini was the designated stronger cross-check (heavier, `has_delta_indirect:
true`, 12 baked aimed spots, 6 animated). It reproduces the same conclusion through a
different failure mode: on the primary fixture the predictor *cannot reach* near-zero FP
at all; on the heavier fixture it *can* reach near-zero FP but only by abandoning the
savings, and reaches meaningful savings only by destroying the dense-needed regions.
Both say: **a cheap forward, pre-bake signal cannot resolve the contribution-concentrated
dense-needed bricks the composed-receiver oracle marks.**

## Is a richer-but-still-cheaper forward predictor worth a follow-up spike?

**No — this data does not warrant reopening the line.** The strongest evidence is the P3
ceiling: it reads the actual baked direct-at-probe field and clears neither bar, floors
at the same FP as the cheap P2, and shows the same no-knee tradeoff. Signal *richness*
within the forward-predictor paradigm is not the missing ingredient. The measured
ceiling on forward predictability sits well short of the accuracy the one-way door would
need, so a follow-up spike on "a slightly richer forward predictor" would be reopening
the door against evidence, not on it.

If the bake-time win is ever revisited, the honest next question is **not** a richer
forward predictor but a different formulation of the problem — e.g. a cheap *partial*
pre-pass (a coarse first-bounce estimate) whose cost is still a fraction of the full
256-ray bake and which is scored against this same oracle — and that is a new spike
with a new hypothesis, not a resumption of this one. Nothing here obligates it.

## What the door stays shut on (deliberately untouched by this spike)

For the record, so a future drafter knows exactly what a promote *would* have required
and what remains undesigned — none of it built, none of it recommended:

- **`density_level` format semantics** — `sh_volume.rs` still writes reserved-zero and
  rejects nonzero on parse; the wire format for variable-density is undesigned.
- **The renderer variable-density sampler** — `sh_sample.wgsl` `probe_tile_origin` and
  `sh_compose.rs` `build_probe_indirection_words` are unchanged; runtime sampling of a
  mixed-density base grid is undesigned.
- **The bake path that skips unstored probes** — `sh_bake.rs` `bake_sh_volume_controlled`
  still bakes `0..total` dense; skipping is undesigned.

This spike is measurement-only and output-preserving (S1 verified byte-identical on
occlusion-test); it opened none of these doors, by design, precisely so the decision to
open them would rest on data. The data says: keep them shut.

## Base-indirect coarsenability characterization (for whoever revisits the storage/bandwidth siblings)

Independently of the forward-predictor question, this spike confirms the base-indirect
field itself coarsens near-losslessly almost everywhere on both fixtures under the
production relative gate (`rel_p95_max` 0.10 / `rel_max_max` 0.25 / `darkness_frac`
0.02): **76.3 % coarsenable (71.3 % fully L2) on occlusion-test; 97.6 % coarsenable
(95.1 % fully L2) on stress-warren-mini.** The problem was never whether the field is
coarsenable post-bake — it demonstrably is — but whether a *forward* predictor can find
the coarsenable regions *before* the bake. It cannot, cheaply and safely. That
post-hoc coarsenability is already productionized for the storage/bandwidth half in
`lighting-scale--sh-adaptive-coarsening-v2`; only the *bake-time* win depended on the
forward predictor, and that is what this spike closes out.

## Cross-references

- Raw numbers, full FP-vs-recovered sweeps, confusion matrices, per-family eval times,
  and the build-profile / lightmap-density environment note: `research.md` (Task 3).
- Harness: `crates/level-compiler/src/sh_forward_predict.rs`, behind default-off
  `--sh-forward-predict` (`--sh-forward-predict-out <PATH>`).
- Prior commitment this builds on: `context/plans/drafts/lighting-scale--sh-probe-density-coarsenability-spike/findings.md`
  (contribution-aware > distance; base indirect coarsens far more readily than the
  composed receiver).
- Shipped storage/bandwidth half: `context/plans/ready/lighting-scale--sh-adaptive-coarsening-v2`.
