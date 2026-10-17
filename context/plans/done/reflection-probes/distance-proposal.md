# Distance-corrected reflection probes — proposal

Status: design proposal for `context/plans/in-progress/reflection-probes`. Read-only research at `949a79411`; no engine, cargo or GPU run. Numbers come from a pure-Python model (scratchpad `dmodel.py`, `scenes.py`, `final_eval.py`, `vista_mean.py`). The model traces the test map's actual brushes, builds a 128² octahedral tile with the compiler's four supersamples, border rule and bilinear lookup, and ports the shader's slab test, horizon clamp and mirror.

## 1. Problem, restated

Box projection assumes every reflected surface lies on the box. Two owner play-test cases break that assumption.

- **Crate** (pool room, outside the water): the reflection is too short. At east-rim fragments the true mirror shows the crate up to 27.5 / 33.6 / 40.6 / 45.0° elevation (x 300 / 320 / 336 / 344). Box projection shows it only up to 23.0 / 25.1 / 27.1 / 28.2°. From the west coping at 3 m eye height, box projection shows no crate in the water at all.
- **Pillar** (room B, standing in the canal): the reflection is misplaced. Box projection never shows the pillar at its base (0 of 15 near-base fragments agree). From the west walkway at 1.6 m it paints the pillar on 371 fragments where the true mirror shows something else, and the true mirror shows the pillar on only 85. 160 of those false fragments are on water the capture point cannot see: the wedge behind the pillar, over its underwater shadow. That is the "shadow lights up" the owner saw: the lit west face is drawn onto that wedge.

## 2. Technique options

Notation: C is the capture point, P is the fragment (or the pixel-style cell centre), R is the reflected ray, and Q(t) = P + tR. The stored value per texel is w = 1/|hit − C|₁. The search reads it as h(Q) = |Q − C|₁ · w(dir(Q − C)) − 1:
- h < 0: Q is in front of the surface the capture saw in that direction.
- h ≥ 0: Q is behind that surface.

On a plane seen from C, h is linear in t, and w is affine across each octahedral octant, so bilinear filtering reproduces a plane exactly.

| Option | Taps (distance + colour) | Crate: wrong fragments, eyes 0.5/1.6/3 m (true 116/96/36) | Pillar false fragments (west 1.6 m) | Vista: rays > 3° off | Verdict |
|---|---|---|---|---|---|
| Box projection (today) | 0 + 1 | 230 / 130 / 36 | 371 | 2.8% | Fails both play tests |
| Single-step depth correction from the box hit (one sphere step) | 1 + 1 | 52 / 59 / 53 | 291 | — | The wall behind the crate is visible from C, so a search that starts at the box hit stops there |
| Linear search (N=4–8) + secant, Umenhoffer et al. form | 5–9 + 1 | 33 / 27 / 28 (N=6) | — | 12.8–15.1% | Coarse samples land in pillar shadows: phantom roots |
| **Recommended: hybrid sphere search + false position** | ≤ 6 (2 parallel, ≤ 4 serial) + 1 | **15 / 22 / 17** | **35** (0 in the shadow) | **5.4%** | Fixes both play tests; small vista residual |
| + verification tap at the root | ≤ 7 + 1 | 64 / 49 / 16 | 201 | 3.3% | Rejected: falls back to the box hit at real edges |
| Two-layer distance (front + back of the first solid) | as above | 33 / 27 / 28 | 163 (no fade) | 14–18% | Rejected for v1: no gain, 2× distance VRAM |

**Recommended search, per reflecting fragment (one function, mirrored on the CPU):**
1. Issue two independent taps, at t = 0 (P) and at t_box (the slab-test hit). If h(0) ≥ 0, P lies behind a surface the capture saw: status **occluded** (§2.2).
2. Take up to K = 3 sphere steps from t = 0. Each step moves to the point of R at distance |X − C| from C, where X is the stored surface in Q's direction. The first step whose sample has h ≥ 0 closes the bracket.
3. If no step closes the bracket, the box hit closes it when h(t_box) ≥ 0. Otherwise status is **miss**: geometry lies beyond the box, and the lookup is the box hit (today's answer).
4. Take M = 2 false-position estimates on the bracket (one more tap). Lookup = Q(t*) − C.
5. Clamp the lookup to the horizon exactly as today. One colour tap at that direction. The light-share ratio uses the same final direction.

Walls that coincide with the box keep box projection's exact answer: h(t_box) = 0 there, so false position returns t_box.

### 2.1 Refinement steps (K) on the crate case

Top elevation of the reflected crate. Columns 1–4: per fragment, ray due east at y 224 (true 27.5 / 33.6 / 40.6 / 45.0). Columns 5–7: on-screen depression angle of the reflection's top edge, eye on the west coping (x 152), on the pool centreline (true 15.7 / 24.8 / 34.6).

| Method | x 300 | x 320 | x 336 | x 344 | eye 0.5 m | eye 1.6 m | eye 3 m |
|---|---|---|---|---|---|---|---|
| True mirror | 27.5 | 33.6 | 40.6 | 45.0 | 15.7 | 24.8 | 34.6 |
| Box projection | 23.0 | 25.1 | 27.1 | 28.2 | 17.0 | 23.1 | none |
| One step from the box hit | 24.9 | 27.6 | 30.0 | 31.4 | 15.6 | 24.1 | 31.5 |
| Recommended, K=1 | 26.5 | 29.1 | 31.4 | 43.0 | 15.6 | 24.8 | 32.3 |
| Recommended, K=2 | 28.0 | 33.4 | 38.2 | 42.0 | 15.6 | 24.9 | 34.5 |
| **Recommended, K=3** | **28.0** | **34.6** | **41.9** | **41.7** | **15.6** | **24.9** | **34.9** |

At x 344 the fragment is 8 units from the 6-unit coping, and the result is 3.3° low. The residual stays near one capture texel (≈1.4°) elsewhere.

### 2.2 Pillar in the water (room B, authored box, C ≈ (1039.6, 256, −15.92))

| Eye | Method | True pillar fragments | Agree | False pillar (in the capture shadow) | Near-base agree | Height error where both show the pillar |
|---|---|---|---|---|---|---|
| West walkway, 1.6 m | Box | 85 | 1 | 371 (160) | 0/15 | 11.5 u |
| West walkway, 1.6 m | Recommended, shadow → no term | 85 | 26 | **35 (0)** | 8/15 | 1.8 u |
| West walkway, 3 m | Box | 81 | 2 | 366 (160) | 0/15 | 39.8 u |
| West walkway, 3 m | Recommended, shadow → no term | 81 | 26 | **29 (0)** | 8/15 | 2.3 u |
| SW walkway, 1.6 m | Box | 53 | 0 | 257 (118) | 0/14 | — |
| SW walkway, 1.6 m | Recommended, shadow → no term | 53 | 6 | **12 (0)** | 3/14 | 6.4 u |

The balcony edge (deck z 192 + 1.6 m) sees only 4 pillar fragments, all on faces the capture cannot see. No method draws them.

**Objects standing in the water.** C sits on the plane, so the capture-facing face's base lies on the tile's horizon row. On that side the search lands on the face, and the reflection attaches at the base, within about 2 units (5 cm). The two side faces are edge-on to C, and the far face is hidden. No single-capture method can reflect them; this is where the 7 of 15 near-base misses come from.

**Disocclusion.** Water behind the object as seen from C gives h(0) > 0, an exact test against the stored distance. The two candidate rules:
- **Box fallback** (today's answer): keeps the false lit pillar over the shadow, 160 fragments from the west eye.
- **No probe term** (recommended): the water draws as it does without reflection, so the underwater shadow stays dark. The cost is a missing reflection of the far wall in that wedge. At the 1.6 m eye about 76° incidence, F ≈ 0.27, so the wedge reads as a darker reflection shadow behind the pillar.

Use a soft ramp, F × (1 − smoothstep(0, κ, h(0))), with κ ≈ 0.05 pinned in the reference, so the wedge has no hard edge. **This is an owner decision.**

### 2.3 Failure modes

| Mode | Behaviour | Size in the model |
|---|---|---|
| Ray grazes another object's capture shadow before its true hit (slim pillars across a vista) | False position lands on the filtered shadow edge, giving a sliver of the occluder's edge colour on the far wall ("rubber sheet") | Vista: 5.4% of rays > 3° (box 2.8%, all of which were object hits) |
| Thin object narrower than a sphere step's jump | Skipped; reflects what lies behind it | Not observed at K=3 on the test maps |
| Hidden faces: far side, edge-on sides, crate top seen from above | Not in the capture. The search returns a nearby face seen from C, or no term in the shadow | Pillar: 7 of 15 near-base fragments |
| Capture miss (no surface; stored 0) | h = −1, never a root; the lookup ends at the box hit | Exact today-behaviour |
| Geometry beyond the box (doorway, vista past the fitted box) | h(t_box) < 0: the box hit, as today | — |
| Bound water the capture point cannot see (L-shaped pool around a corner) | Draws no probe term under the shadow rule: a regression against today | Needs the compiler warning in §6 |

### 2.4 Composition with the rest of the term

- **Horizon clamp.** Applies to every distance-tap direction and to the final lookup. The 2 mm lift drops out, because the clamped P − C is horizontal.
- **Pixel style.** The search runs from the cell centre about the tilted normal. Every fragment of a cell computes the same lookup, so the one-sample-per-cell rule (row 26) holds.
- **Light share.** The ratio reads the final lookup direction, unchanged.
- **Probe layer.** The layer index still comes from the face binding alone (grep gate, row 31). Distance taps use the same layer.

## 3. Storage

| Option | Binding | VRAM at the 256 cap | Precision (vista: lookup shift against f32, p50 / p95 / p99) | Notes |
|---|---|---|---|---|
| (a) Extra layers in the BC6H array | None | +4 MiB | 0.000 / 0.23 / 1.34° | Doubles layers, so the cap halves to 128 probes (the stress map fills 256). Tiles placed side by side instead (256×128 layers) keep the cap. BC6H blocks across silhouettes snap distances; G and B are wasted. Couples the distance to the colour format. |
| **(b) Separate R16Float array, 128², inverse L1 distance** | **+1 sampled texture (10 of 16); reuses the linear-clamp `sh_atlas_sampler`** | **+8 MiB (total ≈12.1 MiB)** | **0.000 / 0.008 / 0.02°** | Recommended. R16Float is filterable on every backend. Same mapping, border and UV as the colour tile. |
| (b′) R16Float at 64² | +1 | +2 MiB | — | Crate wrong-fragment total 81 against 54 at 128² (32²: 159). Not worth the saving. |
| (b″) R8Unorm, log-encoded, 0.25–512 m | +1 | +4 MiB | 0.004 / 0.50 / 2.16° | Viable fallback if VRAM ever binds. One exp2 per tap. |
| (c) Pack into an existing channel | — | — | — | BC6H has no spare channel. RGBA16F colour would be 32 MiB at the cap and 8× the colour bandwidth. Rejected. |

**Stored quantity: w = 1/|hit − C|₁.**
- Planes reconstruct exactly under bilinear filtering within an octant.
- The search needs no sqrt for h.
- The shader's octahedral encode already computes the L1 norm.
- A miss stores 0, a natural "infinitely far" sentinel.

**Supersample combine: mean of w.** This is unbiased for planes. The nearest-surface choice (max w) biases far walls about 6% short and raised vista errors from 5.4% to 8.1%. Filtering must be bilinear: an unfiltered (analytic) distance in the model left brackets straddling silhouettes and doubled the errors.

## 4. Section 54 and compiler

**Section 54 format.** Internal version 1 → 2, and a v1 section is refused. Only this unmerged branch ever wrote v1, so dev maps rebuild. The header gains a distance format tag (one legal value: R16F inverse-L1). The distance payload follows the colour payload: layer-major, one 128² tile per probe with the same border, little-endian f16. That is +32 KiB per probe on disk: e24 +384 KiB, stress map +8 MiB.

**Stage.**
- `STAGE_VERSION` 3 → 4. The inputs are unchanged, so the version bump alone re-keys the cache.
- Distance costs no extra rays: `capture_row` already has `hit.distance` per supersample. It accumulates the mean of 1/(t·|d|₁), with 0 for a miss, in fixed supersample order.
- Borders copy by `irradiance_tile_source_texel`, as colour does.
- The f32 → f16 conversion is deterministic. Expect no measurable stage-time change.

## 5. Cost

| Item | Today's probe term | With distance correction |
|---|---|---|
| Probe-array taps | 1 | 1 |
| Distance taps | 0 | Worst case 6: 2 independent, then ≤ 3 sphere steps and 1 false-position estimate, serial. Typically 4–5 |
| Extra ALU | — | About 30 per tap (octahedral encode, L1, h, sphere step). Worst case about 190; the live-light ratio is already ≤ 600 |
| VRAM | 4.14 MiB at the cap | + 8 MiB R16F (≈12.1 MiB); e24 (12 probes) +384 KiB |
| Compile | — | ≈ 0; PRL +32 KiB per probe |

**e24 codegen risk.** Task 8b moves the probe term into the reflecting pipeline only, so draws that cannot reflect pay nothing. The reflecting variant already shows the e24 scheduling pathology (≈120 registers). The search adds about 16 live floats across dependent taps, and that can lower occupancy further.

Mitigations:
- Run the search before the ratio loop, so its state dies before the loop.
- Unroll it at a constant K.
- Keep K tunable (K = 2 costs 1 tap less: crate eye-3 m top edge 34.5° against true 34.6°).

This needs measuring, not predicting: capture A/B of the reflecting variant on the 1660, at e24's pool pose and the test-map pool pose, plus the owner's 5300M trace. The test map's straight-down pool pose currently costs +0.68 ms for the whole probe term.

## 6. Contract amendments (old → new)

**Decisions**

- **Technique**
  - Old: "**Technique: box-projected local probes, scaled per light** (`research.md` §Prior art, §Light share). Runtime slab test only: no precomputed intersection table, no per-pixel technique choice by lookup direction, no temporal reprojection, no SSR-exit fallback."
  - New: "**Technique: distance-corrected box-projected local probes, scaled per light** (`research.md` §Prior art, §Distance, §Light share). The runtime slab test bounds a short runtime search along the reflected ray against the probe's stored per-texel distance; the tile is sampled toward the hit it finds, or toward the box hit when geometry lies beyond the box. No precomputed intersection table, no per-pixel technique choice by lookup direction, no temporal reprojection, no SSR-exit fallback."
- **Storage**: append "Beside it, a distance tile at the same size, mapping and border holds each texel's mean over its supersamples of 1/|hit − capture point|₁, 0 where the ray hit nothing (R16F)."
- **Runtime placement**: "The fluid pass gains the probe array and the share table" → "The fluid pass gains the probe array, the distance array and the share table".
- **New Decision (owner)**: "**Capture shadow.** A fragment the stored distance places behind a surface seen from the capture point (the far side of an object in or beside the water) draws no probe term, faded over a pinned margin; the capture holds nothing it could reflect there. The compiler warns, naming the probe, when the capture point sees less than half of a probe's bound water area."
- **Non-goals**: add "Layered (multi-hit) distance or colour, and the hidden faces of objects near the water: a single capture point never sees them."

**Acceptance**

- Row 1: "box, capture point and tile per kept probe" → "box, capture point, tile and distance tile per kept probe".
- Row 2: append "a distance payload whose length mismatches the header fails the load naming the section".
- Row 16: append "The distance tile shares that mapping and border."
- Row 17 (box projection) → "Box projection and distance search: in a room with no interior geometry the lookup equals plain box projection within one capture texel; the box hit is independent of the capture point; the lookup is the search's hit minus the capture point, clamped to the horizon below it; with the stored distance beyond the box hit the lookup is the box hit's; a capture miss never ends the search early."
- Row 30: "gains exactly one sampled texture (the probe array) and one uniform buffer (the share table)" → "gains exactly two sampled textures (the probe array and the distance array) and one uniform buffer (the share table); sampler count unchanged".
- Row 33: add "with a distance tile holding 0 (no surface), so the lookup is the box hit's".
- Manual row 38: append "Objects standing in the water reflect attached at their base on the side facing the capture point; behind them the water shows no false reflection."

**New rows**

- Compiler: "Distance tile: in a box room a probe reads 1/|wall − capture point|₁ toward each wall within f16 rounding; a ray that hits nothing stores 0; border texels follow the colour tile's rule."
- Runtime, CPU reference pinned to the shader: "Crate fixture (test-map pool room, analytic tile built by the test): the reflected crate's top edge on the pool centreline, seen from the west coping at eye heights 0.5, 1.6 and 3 m, lies within 0.5° of the true mirror's; at east-rim fragments x 300–336 the top elevation is within 2° (box projection: 4.5–13.5° low)."
- Runtime: "Pillar fixture (room B): a fragment behind the pillar as seen from the capture point draws no probe term; a fragment whose reflected ray meets the pillar's capture-facing face samples it within one texel of the true hit; from the west walkway at 1.6 m no more than 10% of fragments show a false pillar."
- Runtime: "The search is a function of the fragment (or cell), the probe's box, point and distance tile alone: every fragment of a pixel-style cell reads one lookup."
- Adapter-backed: "A probe whose distance tile holds a near wall: the GPU's sampled texel matches the CPU reference's lookup."
- Compiler: "Capture-visibility warning on an L-shaped pool whose bound area is mostly hidden from its capture point."

**Wire format**

- "Header: internal version 1, … atlas format tag (the lightmap's irradiance format tags), …" → "internal version 2, …, atlas format tag (…), distance format tag (f16 inverse L1 distance, the only value), …".
- "Then the atlas payload, layer-major, each tile's bytes in the stated format." → "… Then the distance payload, layer-major, one tile per probe at the same dimension and border, little-endian f16; 0 means no surface."

**Context library at landing**
- `rendering_pipeline.md` §7.3a Reflection: the search, the capture shadow, Limits.
- `rendering_pipeline.md` §10: "10 of 16".
- `research.md` §Budget: 9 → 10, VRAM.
- `resource_management.md` §5.
- `build_pipeline.md` id 54.

**Tests that change**
- level-format `reflection_probes` round trip, version and refusal tests.
- Loader payload checks; `installable_probe_section`.
- Compiler `capture_tests` and `stage_key` version.
- render-cpu `reflection_tests` row 17.
- Renderer `fluid_surface_tests` WGSL↔CPU pins.
- `fluid_pipeline_fragment_*` budget tests and `pipeline_budget_tests` (sampled 9 → 10).
- The four adapter tests in `fluid_surface_frame_test.rs` (group 4 gains binding 3; a dummy 1×1 R16F when absent).
- `fluid_reflection_probes.rs` layout entries; the DX12 FXC run.

## 7. IP and prior art (not legal advice)

**Lineage.** This is the published distance-map lineage:
- Szirmay-Kalos, Aszódi, Lazányi & Premecz, "Approximate Ray-Tracing on the GPU with Distance Impostors", CGF 24(3), 2005: per-texel distance from the reference point, iterative hit search.
- Umenhoffer, Patow & Szirmay-Kalos, GPU Gems 3 ch. 17, 2007: linear search plus secant over distance maps.
- Bjorke 2004 and Lagarde & Zanuttini 2012 for box projection.

**Against the avoid-list:**
- ARM US10636213B2: nothing is precomputed per pair of volumes, and every intersection is computed per fragment.
- Microsoft US10586375B2: there is no per-pixel choice by ray direction. Miss and occluded are terminal values of one search against stored distance, and the probe layer stays binding-only.
- EA (SSR exit to ray trace) family: no SSR involved.
- Autodesk US9355475B1: no history buffer.

**Needs the owner's claims check, now more pressing.** Tencent WO2026113651A1 (priority 2024-11-28). Read from the Google Patents description, not the claims text:
- Its claim 1 broadly covers parallax-correcting a probe from the object's position and the camera.
- Claims 2–3 sample the probe toward the reflected ray's hit, found by hardware ray query.
- Claim 16 reads the probe's depth in the sampling direction and flags an occluder when the probe-to-hit distance exceeds it.

Our h(0) occlusion test and the search's sign test are depth-in-direction comparisons of the same kind. The 2005 and 2007 publications predate it, but a pending application's scope is unknown.

**Also check:**
- NVIDIA filings around McGuire et al., "Real-Time Global Illumination using Precomputed Light Field Probes" (I3D 2017: octahedral radial distance with marching).
- Any post-2006 patents in the distance-impostor family (pre-2006 filings have expired).

A quick patent search found no direct hit, but Google Patents coverage was partial.

## 8. Recommendation and tasks

**Recommendation.** Build it in this brief, as an amendment, before task 10. Manual row 38 ("brush-built floor props appear at the right height") cannot pass with box projection. The crate and pillar are the brief's own test-map features.

The owner decides three things at the amendment:
1. The capture-shadow rule (no term, recommended; or box fallback).
2. The budget row going to two sampled textures.
3. The Tencent claims check before shipping.

If the owner would rather land probes first, ship as built and restate row 38 to coping-only. That is the only alternative that avoids a format bump later.

| # | Task | Depends on |
|---|---|---|
| 11a | Section 54 v2: distance tag and payload, round trip, refusals, loader length check | — |
| 11b | Compiler: mean inverse-L1 distance per texel from the existing hits, border, f16 encode, `STAGE_VERSION` 4, capture tests (box room, miss = 0, jobs determinism); capture-visibility warning | 11a |
| 11c | render-cpu reference: `reflection_search` (two parallel taps, K = 3 sphere steps, M = 2 false position, statuses, shadow fade κ), analytic-tile crate / pillar / empty-room fixtures; row 17 restated; new rows | 11a |
| 11d | Renderer: R16Float distance array (dummy when absent), group 4 binding 3, budgets 9 → 10, shader port in the **reflecting variant only**, WGSL↔CPU pins, FXC | 11c, 8b |
| 11e | Adapter rows 33–36 fixtures plus the near-wall GPU row | 11d |
| 11f | Measure: 1660 capture A/B of the reflecting variant (e24 pool, test-map pool), stress-map VRAM; owner 5300M trace and in-engine look at the crate and pillar; tune K / κ | 11e |
| 11g | Context docs and `research.md` §Distance at landing | 11f |
