# lightmap-penumbra-floor — plan of record

mode: compact
status: active
read at: 52d726196 (no `crates/`, `docs/` or `sdk/` change since the brief's a0a2c0239)

## AC 25 wall-time gate (resolved by owner)

Cold `--no-cache` bakes, dev profile, default 5 workers, Windows 1660 Super machine (SATA SSD + Defender), one bake at a time. Baseline `prl-build` built from c587d8e74 (pre-change); floored from f0ed5c08c. Detector not yet wired, so its share is unmeasured.

| Map | Lightmap Bake base → floor | Ratio | Total compile base → floor |
|---|---|---|---|
| `shadowmask-groups-capture` (hard lights) | 0.93 s → 3.40 s | 3.66× | 2.27 s → 4.38 s |
| `stress-warren-hallway-inspection-mini` | 178.9 s → 555.9 s | 3.11× | 448 s → 823 s (1.84×) |

The cheap path now traces nine footprint probes plus eight clip segments per (light, texel), against four shadow rays (soft) or one (hard) before. Levers for the owner:
- A. Pure optimisation, no contract change: clip results depend only on the texel, so compute them once per chart and share across lights, and skip clip segments for texels with no non-coplanar geometry within one pitch. Removes most of the eight clip segments; light rays stay at nine.
- B. Five probes (centre plus quarter-turn ring at radius ≥ 0.72): light rays 4 → 5 (soft), 1 → 5 (hard). Keeps P3's centre-blocked clause and P16, but the model shows lit gaps on 0.3–0.45-texel lines (0.1–4% of centreline points), so AC 5's "no lit gaps" would need restating.
- C. Accept the cost: raise the gate.

Owner (2026-10-23): do A, re-measure both maps, and come back if still over 1.5×.

After A (151ba1f3f: light-independent clip pre-pass, bit-identical output; sharing across lights not built because the skip leaves nothing measurable to share):

| Map | Lightmap Bake base → floor | Ratio | Total compile |
|---|---|---|---|
| `shadowmask-groups-capture` | 0.99 / 1.01 / 1.01 s → 2.13 / 2.44 / 2.24 s (medians 1.01 → 2.24) | 2.22× | — |
| `stress-warren-hallway-inspection-mini` | 178.9 s → 320.7 s | 1.79× | 448 s → 592 s (1.32×) |

Still over 1.5× on both. A scratch run with no clip work at all measured the same as A, so the remaining cost is the nine light rays per lit texel (soft 4 → 9, hard 1 → 9). Remaining levers: B or C.

Owner (2026-10-23): accept the ~1.8× / 2.2× lightmap-stage cost (C); raise the gate and carry on with Tasks 2–4. AC 25 now reports the final ratios against the accepted figures and flags any rise above them, with the 5% detector-share stop unchanged.

After the soft-light pairing fix (b301faec5), hallway-mini cold: Lightmap Bake 398.2 s (2.23× the 178.9 s baseline, up from the accepted 1.79×), `TexelSteps` 18.7 s summed over workers (≤ 4.7% of the stage, an upper bound), total 673 s (1.50×), no texel-step warnings (AC 28). Groups-capture is hard-light only and unchanged (~2.2×). Flagged to the owner as a rise above the accepted figure.

Clean re-run (owner away, no other load, Task 4 agent paused; baseline c587d8e74 vs b301faec5): groups-capture Lightmap Bake medians 0.96 s → 2.31 s (2.41×; runs 0.99/0.96/0.96 vs 2.31/2.16/2.43); hallway-mini 178.1 s → 389.4 s (2.19×), `TexelSteps` 18.1 s (≤ 4.6%), total 440 s → 659 s (1.50×), no texel-step warnings. The earlier runs overlapped owner gaming but agree within noise.

Owner (2026-10-23): accept the cost for now; a later brief (surface radiance cache) is expected to bring total bake time down, and it removes SH probe rays, not lightmap texel rays, so lightmap-stage speedups stack with it. After Task 4, make the emitter-pairing search branch-free (bytes unchanged) and re-time hallway-mini. Tile shaft culling (skip every ray for a tile whose padded shaft to the emitter no triangle crosses; exact, helps floor-off too) goes to the roadmap as a follow-up idea, sized first by counting the share of (light, texel) pairs that early-out fully lit.

After the branch-free pairing (ac9015fb3; claim 518 → 281 ns at 32 samples, bytes unchanged): hallway-mini Lightmap Bake 370.3 s (2.08× the clean 178.1 s baseline), `TexelSteps` 16.4 s (≤ 4.4%), total 637.7 s (1.45× of 440 s), no texel-step warnings; `soft_shadow_test` −3%, `gate-heavily-lit` −6% on the lightmap stage.

## Corrections

- Path: "the four probes form a designed ring at every count" → a four-point ring cannot meet P3. A strip thinner than half a texel through the texel centre slips between ring points 0.75 texel out, the probes agree, and the texel early-outs to 1.0 with its centre ray blocked. A 2D model (cone kernel, per-texel seeded rotation, angles 0–45°, nine phases, counts 5–64) gave: ring-4 ~300 centre-blocked texels stored 1.0 per sweep; centre + ring-4, none, but bilinear gaps along 0.3–0.45-texel lines (0.1–4% of centreline points); centre + ring-8 at radius ≥ 0.7, no gaps down to 0.4 texel, worst axis-adjacent difference ≤ 0.875 and no texel with exact visibility in (0.1, 0.9) snapped. Planning around it with **nine footprint probes: the centre plus an eight-point ring (radius ≥ 0.6 texel, 45° steps)**. An eight-ring needs only R ≥ 0.52 / cos 22.5° ≈ 0.563 to split every edge within the cone's 10% point (the research's 0.72 is for a four-ring), and at nine samples the cone's E[r²] = 0.3 wants R ≈ 0.58; 0.7 widened the minimum-count ramp past AC 6's bound. Script: `footprint_sim.py` beside this plan (`uv run --with numpy python footprint_sim.py v2`); the Rust tests are the proof.
- Consequence: the texel path's effective sample count is `max(--soft-shadow-samples, 9)`; at the CLI minimum (4) the texel set is the nine probes, so "the probe set is the whole set and nothing escalates" (P13) holds at every count up to 9. Probe-point callers (SH, scatter) keep the existing four-probe emitter path untouched, so their bytes do not move.
- Cost risk follows: the texel cheap path traces nine shadow rays (today four for soft lights, one for hard) plus footprint clip tests. Task 1 measures lightmap-stage wall time before building further; the brief's 1.5× / 5% gate stops for the owner if exceeded.

- Task 3 finding: on `soft_shadow_test`, floor on, light #1 (0.30 m soft) steps 0 → 1 on face 6. All nine probes clear while the full set reads 0.79–0.83. For a soft light the ray's crossing point at an occluder shifts by (1 − t)·footprint offset + t·emitter offset, so a footprint ring paired with arbitrary emitter samples is no ring in crossing space, and P16's straddle argument fails. Fixed by per-texel emitter pairing (`EmitterPairing` in `texel_footprint.rs`): each ring probe takes the farthest emitter sample whose offset, projected onto the receiver along the light, falls in that probe's 45° sector, so the probes ring the texel in every crossing plane. The centre probe traces the emitter's exact centre (the lattice sample nearest it is dropped), which keeps P3 exact for soft lights. Eight interior strata take samples pointing against their offsets (a counter ring), so the escalated fraction is not biased wide (AC 3: 0.672-texel change). Hard lights are byte-identical. A greedy perpendicular-to-light search was tried first and cost 45–55%; the sector form costs ~0.6–0.75 µs per soft (light, texel), about +20% lightmap-stage time on soft-light maps. Proof: `soft_light_penumbra_edges_never_snap_at_any_sample_count` (0.3/1/3-texel penumbrae, point and sun, counts 4–64: the old pairing snapped up to 222 texels; now 0 snapped, 0 steps, worst pair ≤ 0.875); `soft_shadow_test`, `shadowmask-groups-capture` and `gate-heavily-lit` compile with no texel-step warning.

- Task 6 finding: on `gate-heavily-lit`, floor on, `light_sun` (entity 18) lights a sealed corridor: 5,257 texels in a sun-only bake (max 0.0144 linear) against 0 pre-change, mostly the first interior row along face borders. It shows as six new shadowmask drops (faces 0, 3, 6, 9, 22, 25) and one new owner (face 4). This violates the Decision that footprint samples never take visibility from inside solid or across a wall, so it is a bug, fixed under that Decision. Repro: `lpf-bin/task6/sunonly/`. Root cause: the shadow trace starts `RAY_EPSILON` along the ray and tests nothing before it; a footprint sample can sit under 1 mm from a perpendicular face (a texel centre never does), so its first millimetre lands inside a brush whose other faces were culled and the ray escapes (848 samples, all within 0.61 mm of a boundary). Fixed by a lead-in test on clear footprint rays (`lead_in_clear`, front faces in the clip band, overlapping both ends); clean maps byte-identical, `gate-heavily-lit` owner report back to peak 9 / 43 drops (= floor off), tests `a_sealed_room_takes_no_light_from_outside` and `floor_at_a_culled_wall_foot_takes_no_light_through_the_wall` fail without it.

## Delegated answers

- Kernel weights — cone `w(r) = 1 − r` on `r < 1` texel (radial CDF `3r² − 2r³`), the research's "round, radius 1" row (continuous 10–90 ramp 1.04; ESF 10% point at 0.52 texel).
- Detector threshold — `0.78 + 0.17 = 0.95` on raw visibility, a code constant folded into the `lightmap_section` and weight-map keys. The floored bake's measured worst pair is 0.917 (24 samples, mid-phase axis edge); any hard 0/1 step (1.0) exceeds it, and any margin under 0.22 keeps that.
- Clipped footprint samples — clip test is a segment from the centre ray origin to the offset ray origin; a blocked one drops that sample and the texel renormalizes over the survivors (research: keeps the pair bound at 0.78). Any clipped probe forces the full set (P18).
- Footprint frame — offsets are in parent-grid texel units, mapped through the chart's `u_axis`/`v_axis` grid pitch, rotated by an angle derived from `chart_texel_seed`, so sub-charts reproduce the parent's offsets bit for bit (P5).
- Switch name — `--no-penumbra-floor` on `prl-build`, default floored.
- Influence padding (P10) — the layer key's geometry bound widens by the largest footprint radius among the light's in-reach charts (their texel pitch), not by a global constant.
- Emitter pairing — nine-probe emitter indices come from a banded snap (probe k takes the nearest lattice sample within the k-th of nine index bands); a plain nearest-direction snap collapses onto samples 0–8 for narrow emitters. The four-probe `probe_indices` path is untouched.
- Contact fixtures — an occluder plate or pole 2 mm above an uncut floor (above the 1 mm ray lift). A solid resting on an uncut floor turns its base into an inside corner whose under-solid texels step 0/1 against open floor; Task 3's P8 handling must keep such pairs (texel centre outside its face polygon or inside solid) from warning.
- Lit gap (P3) — a point on the pole shadow's centreline where bilinear reconstruction of stored visibility returns 1.0.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 hard point/spot/sun + default-size contact ramps within 0.2 of floor width | `penumbra_floor` contact-ramp tests vs analytic cone-kernel ramp at same phase/angle | achievable as stated |
| 2 footprint offsets within one pitch; radial distribution matches cone kernel; wider/square fails | layout unit test (KS vs `3r²−2r³` at default and 64) + negative controls | achievable as stated |
| 3 clear-standing occluder: ramp change < 1 texel on vs off | fixture bake, floor on vs off | achievable as stated |
| 4 texels beyond one texel of any edge keep exact 0.0 / 1.0 | contact fixture assertion | achievable as stated |
| 5 thin pole (P3): centre-blocked < 1.0, no lit gaps, hard + default-size | thin-pole fixture, angle/phase sweep | achievable as stated (nine-probe ring; see Corrections) |
| 6 minimum count (P13): ramp, pole, detector silent | same fixtures at `--soft-shadow-samples 4` | achievable as stated |
| 7 P16 count sweep, no snap of (0.1, 0.9) texels | mid-phase hard contact edge at 4–8, 12, 16, 24, 32, 64 | achievable as stated |
| 8 inside-corner sweep (P18) | wall-foot fixture, min + default counts | achievable as stated |
| 9 `_lightmap_scale` region ramp (P14), both walk orders, one worker | two-face fixture, single-thread pool, both orders | achievable as stated |
| 10 inside corner / thin wall (P4) | wall-foot and thin-wall fixtures | achievable as stated |
| 11 face cuts bit-identical; estimate reads floored visibility (P5) | face-cut fixture + `shadowmask_bake/estimate.rs` test | achievable as stated |
| 12 static layer vs animated weight map agree | contact fixture baked both ways | achievable as stated |
| 13 SH/bounce/delta/direct SH/scatter byte-identical to pre-change | two-PRL compare against a pre-change `prl-build` | pass: `penumbra_floor_baseline_probe` (ids 27, 34, 35, 41, 45, 47, 48, 50) identical on soft_shadow_test, groups-capture, gate-heavily-lit, closet-reveal, animated-layer-spill; also `--release` |
| 14 switch off = centre-point; weight maps = pre-change; toggling re-keys memos (P6) | unit + two-PRL compare + warm-cache log test | pass: `--no-penumbra-floor` PRLs byte-identical to the pre-change bake on seven maps (`cmp`), ids 24/25 via `penumbra_floor_baseline_weight_maps` on closet-reveal; stage tests from Tasks 2 and 4 |
| 15 determinism across workers (P9), warm = cold, version bumps | repeat-bake tests + diff gate on `LAYER_FORMAT_VERSION` and weight-map stage version | achievable as stated |
| 16 production stage = reference bake, both settings (P7) | existing stage-vs-reference test extended | achievable as stated |
| 17 footprint-only occluder past padding re-bakes (P10) | `_lightmap_scale` cache-key test | achievable as stated |
| 18 detector reaches caller thread from production stage + weight maps; short edge, cut edge (P1, P17) | stage tests with `test-log-capture` / returned report | achievable as stated |
| 19 warm compile replays exactly (P2, P11, P12, P15, weight-map hit) | stage cache tests | achievable as stated |
| 20 floor on: no false warnings (P8) | border/terminator/cone/reach/corner/thin-wall fixture | achievable as stated |
| 21 BC5 / BC6H error + owner report before vs after | measurement run on three maps | measured (pre-change re-measured like for like; see Results) |
| 22 default floored; `--help` lists switch; not in `docs/` | CLI test + grep | achievable as stated |
| 23 sub-texel warning gone; no "0 = hard" text | grep gate | achievable as stated |
| 24 nothing under `crates/level-format/` or `crates/renderer/src/shaders/` | diff gate | achievable as stated |
| 25 lightmap stage cold wall time ≤ 1.5×, detector ≤ 5% | timed bakes, hallway-mini + groups-capture | gate raised by owner to the accepted 1.79× / 2.22×; final run reports against them; detector ≤ 5% still stops |
| 26 capture 2560×1440, floor off vs on, vs `_lightmap_density 0.01` | owner, in-engine | manual |
| 27 shadowmask specular, no step / no banding rise | owner, in-engine | manual |
| 28 hallway-mini full compile logs no texel-step warnings | compile + log grep (executor runs it) | pass at b301faec5 (0 warnings); re-run at the end |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Footprint module (layout, nine-probe early-out, clip test), wired into the static layer bake behind `LightmapConfig` switch; pure detector over a per-light grid; AC 1–8, 10 tests; early wall-time read on groups-capture and hallway-mini | delegated (Opus), integrator reviews | — | code done: `texel_footprint` (+ `tests.rs`, `test_fixtures.rs`), `texel_step_detector`; AC 1–8, 10 and switch-off pass (`cargo test --bin prl-build`: 1787 passed); wall-time read: over gate (Block) |
| 2 | Cache and keys: `LAYER_FORMAT_VERSION` bump, switch into `light_prefix` and CLI, footprint padding (P10), reference bake parity (P7), determinism (P9), scale region (P14), face cuts + estimate (P5) | delegated | 1 | done: v10, `--no-penumbra-floor`, per-light footprint padding; AC 9, 11, 14–17, 22 (CLI) pass in `penumbra_floor_tests.rs` and siblings; 1799 passed. Landing note: any charted-face move already re-keys every layer via `atlas_layout_fingerprint`, so P10 padding guards only edits charts miss; `build_pipeline.md` §Build Cache layer key and CLI table need the switch and padding |
| 3 | Detector in the production lightmap stage: report to caller, store/replay with `lightmap_section`, shadowmask path, key folds threshold; delete `warn_sub_texel_penumbra_lights` (P1, P2, P8, P11, P12, P15, P17) | delegated | 2 | done: `lightmap_stage/texel_steps.rs`; report stored in the section memo (`LIGHTMAP_SECTION_VERSION` 5), threshold folded into its key; P8 drops pairs a surface separates (`surface_separates`); `TexelSteps` summary sub-line; AC 6, 15 (warnings), 18 (layer half), 19, 20 pass; 1805 passed. Found: soft-light early-out snaps (below) |
| 4 | Animated weight maps: footprint in the weight-map bake, grid + detector, warnings in the `animated_lm_weight_maps` entry, stage version bump; AC 12 | delegated | 2 | done: `face_bake.rs` (one Rayon task per face, one grid in flight per worker), shared report helpers in `texel_step_detector/report.rs`; `STAGE_VERSION` 10; key folds switch, threshold and cut map; AC 12, 14, 15, 18 (weight-map half), 19, 20 pass; 1814 passed. Watch: per-face parallelism on many-chunk faces; second `TexelSteps` sub-line under AnimWeightMaps |
| 5 | Text: docs, FGD, `MapLight` docs, default-size comments, `build_pipeline.md` rows; fixtures relying on exact authored `0`; AC 22–23 | delegated (no cargo) | 1 | author text done (`docs/level_design.md`, FGD, `map_data.rs`, `quake_map.rs`); remaining hits are the sub-texel warning and its tests (Task 3) and `probe_indices`/texel-loop comments in `lightmap_bake.rs`; grep gate at Task 6 |
| 6 | Byte-identity and measurement: SH/scatter two-PRL compare, switch-off weight maps, BC5/BC6H + owner report, wall time, hallway warnings; capture scene JSONs | delegated | 3, 4 | compares and measurements done (`lightmap_bake/penumbra_ab_tests.rs`); found sun leak (below); AC 21 re-measure after the fix; captures pending |
| 3b | Soft-light emitter pairing fix (Task 3 finding) | delegated | 3 | done; 1806 passed; hallway-mini lightmap 2.23× (flagged) |
| 4b | Branch-free emitter pairing, bytes unchanged; re-time hallway-mini | delegated | 4 | done (ac9015fb3): reference-equivalence tests, four maps `cmp`-identical; hallway-mini lightmap 2.08× |
| 7 | Preflight, review panel, fixes, full gate | integrator | 1–6 | |
