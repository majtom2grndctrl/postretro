# lightmap-penumbra-floor — plan of record

mode: compact
status: active
read at: 52d726196 (no `crates/`, `docs/` or `sdk/` change since the brief's a0a2c0239)

## Corrections

- Path: "the four probes form a designed ring at every count" → a four-point ring cannot meet P3. A strip thinner than half a texel through the texel centre slips between ring points 0.75 texel out, the probes agree, and the texel early-outs to 1.0 with its centre ray blocked. A 2D model (cone kernel, per-texel seeded rotation, angles 0–45°, nine phases, counts 5–64) gave: ring-4 ~300 centre-blocked texels stored 1.0 per sweep; centre + ring-4, none, but bilinear gaps along 0.3–0.45-texel lines (0.1–4% of centreline points); centre + ring-8 at radius ≥ 0.7, no gaps down to 0.4 texel, worst axis-adjacent difference ≤ 0.875 and no texel with exact visibility in (0.1, 0.9) snapped. Planning around it with **nine footprint probes: the centre plus an eight-point ring (radius ≥ 0.6 texel, 45° steps)**. An eight-ring needs only R ≥ 0.52 / cos 22.5° ≈ 0.563 to split every edge within the cone's 10% point (the research's 0.72 is for a four-ring), and at nine samples the cone's E[r²] = 0.3 wants R ≈ 0.58; 0.7 widened the minimum-count ramp past AC 6's bound. Script: `footprint_sim.py` beside this plan (`uv run --with numpy python footprint_sim.py v2`); the Rust tests are the proof.
- Consequence: the texel path's effective sample count is `max(--soft-shadow-samples, 9)`; at the CLI minimum (4) the texel set is the nine probes, so "the probe set is the whole set and nothing escalates" (P13) holds at every count up to 9. Probe-point callers (SH, scatter) keep the existing four-probe emitter path untouched, so their bytes do not move.
- Cost risk follows: the texel cheap path traces nine shadow rays (today four for soft lights, one for hard) plus footprint clip tests. Task 1 measures lightmap-stage wall time before building further; the brief's 1.5× / 5% gate stops for the owner if exceeded.

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
| 13 SH/bounce/delta/direct SH/scatter byte-identical to pre-change | two-PRL compare against a pre-change `prl-build` | achievable as stated |
| 14 switch off = centre-point; weight maps = pre-change; toggling re-keys memos (P6) | unit + two-PRL compare + warm-cache log test | achievable as stated |
| 15 determinism across workers (P9), warm = cold, version bumps | repeat-bake tests + diff gate on `LAYER_FORMAT_VERSION` and weight-map stage version | achievable as stated |
| 16 production stage = reference bake, both settings (P7) | existing stage-vs-reference test extended | achievable as stated |
| 17 footprint-only occluder past padding re-bakes (P10) | `_lightmap_scale` cache-key test | achievable as stated |
| 18 detector reaches caller thread from production stage + weight maps; short edge, cut edge (P1, P17) | stage tests with `test-log-capture` / returned report | achievable as stated |
| 19 warm compile replays exactly (P2, P11, P12, P15, weight-map hit) | stage cache tests | achievable as stated |
| 20 floor on: no false warnings (P8) | border/terminator/cone/reach/corner/thin-wall fixture | achievable as stated |
| 21 BC5 / BC6H error + owner report before vs after | measurement run on three maps | measured, not gated |
| 22 default floored; `--help` lists switch; not in `docs/` | CLI test + grep | achievable as stated |
| 23 sub-texel warning gone; no "0 = hard" text | grep gate | achievable as stated |
| 24 nothing under `crates/level-format/` or `crates/renderer/src/shaders/` | diff gate | achievable as stated |
| 25 lightmap stage cold wall time ≤ 1.5×, detector ≤ 5% | timed bakes, hallway-mini + groups-capture | measured, stops for owner |
| 26 capture 2560×1440, floor off vs on, vs `_lightmap_density 0.01` | owner, in-engine | manual |
| 27 shadowmask specular, no step / no banding rise | owner, in-engine | manual |
| 28 hallway-mini full compile logs no texel-step warnings | compile + log grep (executor runs it) | achievable as stated |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Footprint module (layout, nine-probe early-out, clip test), wired into the static layer bake behind `LightmapConfig` switch; pure detector over a per-light grid; AC 1–8, 10 tests; early wall-time read on groups-capture and hallway-mini | delegated (Opus), integrator reviews | — | code done: `texel_footprint` (+ `tests.rs`, `test_fixtures.rs`), `texel_step_detector`; AC 1–8, 10 and switch-off pass (`cargo test --bin prl-build`: 1787 passed); wall-time read pending |
| 2 | Cache and keys: `LAYER_FORMAT_VERSION` bump, switch into `light_prefix` and CLI, footprint padding (P10), reference bake parity (P7), determinism (P9), scale region (P14), face cuts + estimate (P5) | delegated | 1 | |
| 3 | Detector in the production lightmap stage: report to caller, store/replay with `lightmap_section`, shadowmask path, key folds threshold; delete `warn_sub_texel_penumbra_lights` (P1, P2, P8, P11, P12, P15, P17) | delegated | 2 | |
| 4 | Animated weight maps: footprint in the weight-map bake, grid + detector, warnings in the `animated_lm_weight_maps` entry, stage version bump; AC 12 | delegated | 2 | |
| 5 | Text: docs, FGD, `MapLight` docs, default-size comments, `build_pipeline.md` rows; fixtures relying on exact authored `0`; AC 22–23 | delegated (no cargo) | 1 | author text done (`docs/level_design.md`, FGD, `map_data.rs`, `quake_map.rs`); remaining hits are the sub-texel warning and its tests (Task 3) and `probe_indices`/texel-loop comments in `lightmap_bake.rs`; grep gate at Task 6 |
| 6 | Byte-identity and measurement: SH/scatter two-PRL compare, switch-off weight maps, BC5/BC6H + owner report, wall time, hallway warnings; capture scene JSONs | integrator | 3, 4 | |
| 7 | Preflight, review panel, fixes, full gate | integrator | 1–6 | |
