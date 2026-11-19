# E20--scripted-run-capture — plan of record

mode: compact
status: active
read at: 8489a1fed

## Corrections
- Brief Path names the headless and capture exit paths as plain `process::exit` → since ecd9489e, `E20--streaming-telemetry` brackets every exit with `crate::telemetry::end_run_before_exit` and settles GPU timing when an offscreen renderer is created (`capture/prepared.rs`, `observability/driver.rs`); planning around it by routing the capture run's exits through the headless driver's existing bracketing and calling `telemetry::settle_gpu_timing` on its offscreen renderer.
- Brief Path says `redraw` is "past ~800 lines" → it is one ~2,900-line function (`frame_loop/mod.rs`); the presentation inputs live in its `RenderPrep` stage (lines ~1905–2400). Planning around it by splitting out the `RenderPrep` presentation block first, behavior-preserving, in its own commit, before the shared step is extracted from it.

## Delegated answers
- None. The brief's Open questions section is empty.

## Environment
- This session's machine: 2 cores, 7 GB RAM, no hardware GPU. A software adapter (Mesa llvmpipe, Vulkan) is installed, so `POSTRETRO_REQUIRE_GPU` rows run here on llvmpipe; same-adapter byte identity is proven on that adapter.
- The workspace pins Rust 1.99; this machine has 1.97 and `static.rust-lang.org` is blocked by the session's network policy. Builds run with `--ignore-rust-version` where the code compiles on 1.97; any row that cannot is marked `blocked: toolchain` in Results, never silently passed.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 two-tick captures byte-identical across fresh processes | `tests/scripted_capture.rs` `capture_pngs_byte_identical_across_runs` (GPU) | achievable as stated |
| A2 two capture runs, identical unfiltered dumps (P8) | `scripted_capture.rs` `capture_run_dumps_byte_identical` on the fixture | achievable as stated |
| A3 capture-free runspec, no adapter, dump equals base commit's | `headless_byte_identity.rs` existing test + `capture_free_runspec_matches_base_binary` (base binary built from 8489a1fed) | achievable as stated |
| A4 build without `capture` rejects capture section before load (P4) | `observability::runspec` unit test under `#[cfg(not(feature = "capture"))]`, run via `cargo test -p postretro --features observability`; existing batch byte-identity passes | achievable as stated |
| A5 parse failures by field; tick `ticks` and tick-0/`ticks: 0` succeed (P12) | `runspec` unit tests | achievable as stated |
| A6 dump equal across modes, residency options, capture lists; dump names capture run (P20) | `scripted_capture.rs` `dump_independent_of_mode_and_captures` | achievable as stated |
| A7 second capture fails, first PNG stays (P17) | `scripted_capture.rs` `failing_capture_keeps_earlier_png` | achievable as stated |
| A8 tick-0 capture = revealed frame, levelLoad light (P1) | `scripted_capture.rs` `tick_zero_draws_installed_level` | achievable as stated |
| A9 emitter / burst lifetime (P15) | `scripted_capture.rs` `emitter_billboards_follow_ticks` | achievable as stated |
| A10 dynamic light and fog at k+1 not k (P12) | `scripted_capture.rs` `light_and_fog_enable_next_tick` | achievable as stated |
| A11 mover at tick-N position (P14) | `scripted_capture.rs` `mover_draws_at_tick_transform` | achievable as stated |
| A12 skinned entity two poses, same tick same bytes | `scripted_capture.rs` `skinned_pose_follows_clip` | achievable as stated |
| A13 player pose = override at eye/yaw/pitch | `scripted_capture.rs` `player_pose_matches_override` | achievable as stated |
| A14 aim at tick N applies after capture N; tick 0 spawn pitch 0 (P13) | `scripted_capture.rs` `aim_on_capture_tick_applies_after` | achievable as stated |
| A15 no local pawn: player pose fails, override succeeds (P5) | `scripted_capture.rs` `player_pose_without_pawn_fails` | achievable as stated |
| A16 two captures one tick same pose identical (P2) | `scripted_capture.rs` `same_tick_same_pose_identical` | achievable as stated |
| A17 viewmodel on/off (P6) | `scripted_capture.rs` `viewmodel_opt_in` | achievable as stated |
| A18 isolated, two distant areas = single-capture runs (P3, P16) | `scripted_capture.rs` `isolated_streamed_areas_equal_single_runs` | achievable as stated |
| A19 isolated, promotable light full weight in both (P7) | `scripted_capture.rs` `isolated_promotion_whole` | achievable as stated |
| A20 isolated, promoted light absent from later view (P9) | `scripted_capture.rs` `isolated_drops_prior_promotion` | achievable as stated |
| A21 isolated, fog and planar from first view absent in second (P10) | `scripted_capture.rs` `isolated_drops_fog_and_planar` | achievable as stated |
| A22 isolated, far skinned resample (P11) | `scripted_capture.rs` `isolated_far_skinned_resamples` | achievable as stated |
| A23 continuous, shadow fades in (P18) | `scripted_capture.rs` `continuous_promotion_fades` | achievable as stated |
| A24 continuous, override list position irrelevant (P21) | `scripted_capture.rs` `continuous_override_order_irrelevant` | achievable as stated |
| A25 continuous tick 0 = isolated tick 0 (P19) | `scripted_capture.rs` `continuous_tick_zero_equals_isolated` | achievable as stated |
| A26 continuous tick-60 independent of earlier captures | `scripted_capture.rs` `continuous_capture_independent_of_earlier` | achievable as stated |
| A27 continuous with no pawn fails naming the mode | `scripted_capture.rs` `continuous_without_pawn_fails` | achievable as stated |
| A28 settled continuous on streamed level draws nothing missing; streamed residency completes | `scripted_capture.rs` `settled_continuous_no_missing`, `streamed_continuous_completes` | achievable as stated |
| A29 streaming env vars ignored with one warning each | `scripted_capture.rs` `streaming_env_vars_ignored` (log capture, `test-log-capture`) | achievable as stated |
| A30 one presentation step; grep gate | `frame_loop` unit test `presentation_collectors_have_one_caller` (source grep) | achievable as stated |
| A31 static capture byte-identical, windowed frame-loop tests pass | existing capture tests + `frame_loop` tests | achievable as stated |
| A32 capture rows run under `POSTRETRO_REQUIRE_GPU` | `scripted_capture.rs` gated with the capture tests' existing `require_gpu` helper | achievable as stated |
| A33 SDK bundle runs fixture via `docs/` command; release and player engines lack both features | `xtask` dist test + `sdk-dist` run | achievable as stated |
| M1 owner visual read beside windowed screenshots | owner, in-engine | manual |
| M2 bone-socket capture attached to the PR | owner / executor with fixture, attached at landing | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Split `redraw`'s `RenderPrep` presentation block into `frame_loop/presentation.rs`, behavior-preserving, own commit | integrating executor | — | |
| 2 | One `PresentationState` bundle (pool, particles, emitter/light/fog bridges, mesh render, clip tables) held by `Session`; a host-agnostic `PresentationStep` that `redraw` calls; grep gate (A30, A31) | integrating executor | 1 | |
| 3 | Runspec `capture` section: schema, validation by field, feature rejection, dump marker (A4, A5) | integrating executor | — | |
| 4 | Capture host on `HeadlessSession`: renderer-backed model sweep, light-bridge install, presentation state, offscreen renderer, per-tick presentation at alpha 1, PNG write, failure naming (A2, A3, A6–A12, A16) — **first slice: one emitter, one dynamic light, one fog volume, one promotable light near a skinned mesh, tick 0 and a later tick** | integrating executor | 2, 3 | |
| 5 | Pose and viewmodel: lift `pawn_eye_position`, last-applied aim pitch, override pose, offscreen first-person pass (A13–A15, A17) | integrating executor | 4 | |
| 6 | Isolated mode: renderer drop-view-history (light slots, fog hysteresis, skinned palette), app planar clear, per-capture preload (A18–A22) | integrating executor | 4 | |
| 7 | Continuous mode: per-tick held-view frame, override extra draws, settled and streamed residency, env-var warnings (A23–A29) | integrating executor | 4, 6 | |
| 8 | Fixture map, runspec and README under `content/dev`; GPU-gated suite (A1, A32) | integrating executor | 4 | |
| 9 | SDK: authoring engine features, `docs/` page, xtask headless sidecar (A33) | integrating executor | 8 | |
| 10 | Context docs: `rendering_pipeline.md` §7.8 scripted run, §7.5 drop `frame_index` | integrating executor | 7 | |
