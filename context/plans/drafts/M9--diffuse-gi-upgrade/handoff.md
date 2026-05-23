# Milestone 9 Planning Handoff

> **Read this when:** resuming Milestone 9 planning in a fresh session.
> **State:** roadmap committed. Spec #1 (probe weight correctness) drafted in `context/plans/drafts/M9--probe-weight-correctness/`. Spec #0 (fog wire-up) found already done — see Decisions below.
> **Related:** `context/plans/roadmap.md` (Milestone 9), `context/lib/rendering_pipeline.md` (§5 BVH, M5 lighting).

## What this milestone is

Upgrade the Milestone 5 diffuse GI so indirect light stops leaking through walls, then extend fog. M5 (SH irradiance volume + baker, runtime probe sampling as a 3D texture, lightmaps, CSM) is assumed complete and shipped — M9 is a pure upgrade layer on top, building nothing M5 already delivers.

## Decisions locked

| Decision | Choice | Why |
|---|---|---|
| Roadmap slot | New **Milestone 9** | Promoted out of scattered Future/Rendering bullets into a numbered milestone. |
| GI interpolant | **Replace** trilinear SH sampling | Depth-aware (Chebyshev) visibility weighting becomes the single runtime path; plain trilinear removed. No dual-path maintenance. |
| Fog scope | **Add directional fog** | On top of the existing volumetric fog scope. |
| Fog pass wire-up | **Already done — no spec needed** | `fog_pass.rs` is imported (`render/mod.rs`), owned by `Renderer`, and runs every frame; it was wired with the portal-fog-culling work. The "never imported" premise was stale. Directional fog (#4) builds on the live pass. |
| Probe streaming | **Defer + measure** | See rationale below. |

## "Free leak fixes" — ship before the depth atlas

Cheap, pure-ALU probe-blend fixes that do **not** get superseded by DDGI and go *first*:

- **Renormalize valid-probe weights + exclude invalid probes.** Today invalid probes are packed as zero and blended in, dragging near-wall surfaces toward black. This is a latent darkening bug **independent of DDGI**, and DDGI needs the same fix anyway (it divides by the sum of weights and drops zero-weight probes). Permanent correctness fix, not a throwaway approximation.
- **Normal-based corner rejection.** Reject trilinear corners facing away from the surface normal. Eventually folds into DDGI's unified weight (the backface/cosine term), but ships first because it's near-free and diagnostic.

**Why first:** establishes the residual-smear baseline. Measure leak/smear after the free fixes and before the depth atlas — that delta is what the atlas actually buys. Same defer-and-measure discipline as streaming.

Superseded by DDGI (do **not** ship separately): hand-rolled per-probe visibility heuristics that approximate the depth test — the Chebyshev interpolant does this properly.

## Streaming: why deferred

Including cell/portal-PVS brick streaming now would triple the milestone's moving parts on the *same* hot sampling path (DDGI interpolant + directional fog + streaming all live in the probe-sampling shader), entangling debugging. Specific risks: trilinear filtering can't cross separate brick textures (bordered-brick bake or manual neighbor lookups — the classic seam/leak time-sink); two datasets to stream (SH bands + depth atlas); dependency on the still-evolving cell system (camera-leaf lookup still rides the BSP).

Deferring is low-risk because the decision hinges on one empirical fact — does a representative large map fit resident in VRAM? Rough math says yes even near the top of the size range (~150 MB SH + ~540 MB depth for a 256×256×32 m map, inside a 6 GB 1660). The brick refactor is the same work later whether or not DDGI exists, so deferring risks no double-rewrite.

**Insurance taken:** keep the depth-atlas format chunk-friendly so a later brick split needs no interpolant rewrite; add a VRAM-budget readout + coarser open-area probe spacing (spec #4) to produce the resident-fit number that gates any future streaming milestone.

## Spec outline (to draft next)

| # | Spec | Depends on |
|---|---|---|
| 0 | ~~Wire up the fog pass~~ — **already done** (pass is live in `render/mod.rs`) | — |
| 1 | Probe weight correctness (no new data) — corner rejection + valid-weight renormalization; fixes near-wall darkening; **measurement gate** for residual smear — **drafted** → `M9--probe-weight-correctness/` | — |
| 2 | Probe depth/visibility atlas (bake) — per-probe depth moments alongside SH bands, ray-cast through the M4 BVH; chunk-friendly format | #1 measurement informs whether/how much |
| 3 | Depth-aware runtime interpolant — visibility-weighted (Chebyshev) sample replacing trilinear, for static surfaces and dynamic entities | #2 |
| 4 | Directional fog — extend the live fog pass with the directional term | — (fog pass already live) |
| 5 | Memory-budget checkpoint + coarse open-area probe spacing — the streaming "measure" gate | — |

**Sequencing:** #1 → #2 → #3 (chain; #0 already done); #4 independent (fog pass live); #5 independent. #1 is the cheap leak/darkening fix and its measurement gates the cost of #2.

## Where we're heading / next steps

1. ~~Draft the chain head.~~ Done: the chain head was #0 + #1; #0 turned out already wired (see Decisions), so the head is just spec #1 — **drafted** in `context/plans/drafts/M9--probe-weight-correctness/`. Not yet reviewed.
2. Review spec #1 (`review-draft-spec`); resolve its 4 open questions (zero-packed validity ambiguity, shader-copy scope, 72-fetch cost, measurement-gate storage). Then draft #2–#5.
3. Promote reviewed specs to `ready/` — at which point durable GI/fog contract decisions migrate into `context/lib/rendering_pipeline.md`.

## Open questions

- Exact PRL section strategy for depth moments: extend the existing SH section or add a sibling section? (Decide during spec #1.)
- Probe spacing policy for open areas: uniform-coarse vs. adaptive. (Spec #4.)
- Whether directional fog shares uniforms/format with the existing volumetric fog or gets its own. (Spec #3.)
