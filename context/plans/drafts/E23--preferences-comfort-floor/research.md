# E23--preferences-comfort-floor: Research

> Findings behind `index.md`, read at `683e363ba`. Decisions live in the brief. Epic-level research stays in `context/plans/ready/E23--accessibility/research.md`; this file adds what the draft session found and re-grounds that file's U1 pins.

---

## Limiter mechanism

**Prior art.**

| Source | What it does | Taken |
|---|---|---|
| EA IRIS (github.com/electronicarts/IRIS; `Flash.cpp`, `TransitionTracker.cpp`) | Offline detector. Sums the frame-mean change in linear relative luminance while its sign holds, resets the sum on a sign flip, and flags a transition once it reaches 0.1 with the darker frame below 0.8. Red: pixels with R/(R+G+B) ≥ 0.8, measure max(0, (R−G−B)×320), transition at Δ > 20. Area: whole-frame changed-pixel fraction ≥ 25%. Windows are frame counts derived from fps. | Transition model. It catches smooth strobes, and it is WCAG's peak-to-valley definition. |
| FFmpeg `vf_photosensitivity` | The only open real-time attenuator. Measures an 8×8 grid of gamma-encoded channel means. Over budget, it blends the last output frame toward the new one by the remaining budget, and repeats the last frame once the budget is gone. The history is a frame count. | Grid measurement, and limiting against the last output. Not frame hold: a game cannot repeat frames without freezing play. Not frame-count history. |
| Harding FPA / Ofcom / ITU-R BT.1702 | Opposing changes of ≥ 20 cd/m² with the darker state < 160 cd/m²; 25% screen area; extended-fail rules. FPA itself is proprietary. | Thresholds cross-check only. |
| Apple "Dim Flashing Lights", Zoom | Real-time dimming; the algorithm is unpublished. | Confirms dimming, not holding, as shipped practice. |
| ACM TACCESS 2024 gap analysis (PMC11872230) | No guideline bounds the time over which a transition develops. Frame-by-frame deltas fail at high refresh. | Why transitions accumulate from the last extremum. |

No shipped game applies an automatic full-frame limiter; games edit their own content.

**Definitions.** WCAG 2.2 general flash: "a pair of opposing changes in relative luminance of 10% or more of the maximum relative luminance (1.0) where the relative luminance of the darker image is below 0.80"; at most three in any one second; area ≤ 0.006 sr, 25% of any 10° field (341×256 px at 1024×768). WCAG 2.2 red flash, Note 3, per ISO 9241-391: one state has R/(R+G+B) ≥ 0.8, and the states differ by > 0.2 in CIE 1976 u′v′. The WCAG 2.0 test, (R−G−B)×320 > 20, is superseded but still used by IRIS. The limiter desaturates any saturated-red transition, which satisfies both tests; the independent test counter uses 2.2.

**Measurement placement.** The resolve (`ScreenEffectsPass::encode_resolve`, `screen_effects.wgsl`) samples `scene_color` at `uv + shake`, then applies `soft_knee_tonemap`, the vignette mix and the flash mix, in that order, and writes the swapchain with `LoadOp::Clear`. The swapchain is `RENDER_ATTACHMENT` only, and `scene_color` is `RENDER_ATTACHMENT | TEXTURE_BINDING`, so the presented frame cannot be read or reduced. Two shapes measure what is presented:
- *Shared composite (chosen).* A compute pass samples `scene_color` per cell through the same composite function the resolve uses, and writes per-cell gain and desaturation the resolve applies. The gain is applied in display-linear space after the composite, so the cell's limited luminance is gain × measured. That is known exactly and becomes next frame's "last output" with no readback. The cost is a small dispatch. The risk is the composite drifting between the two passes; sharing one WGSL function removes it.
- *Intermediate target (rival).* The resolve writes an offscreen target, and a limiter pass reduces it and writes the swapchain. It is exact by construction, but costs a full-resolution target plus a full-screen pass every frame.

- *Count screen-effect flashes into the frame budget (hub `research.md` §Brief pins, luminance source).* The channel clamp's flash transitions feed a counter the frame limiter shares, and the measure reads `scene_color` alone. Rejected: it measures flash before the tonemap and vignette the presented frame applies, and a scene step and a flash step that combine into one presented transition, or cancel, are counted as two or none.

**Area.** Per-cell counting alone misjudges area both ways. A threshold-sized flash straddling cell boundaries covers no whole cell, and a cell-sized flash smaller than the threshold would count. Cells stay smaller than the threshold, and a flash counts when the cells transitioning together reach the threshold area. IRIS's changed-pixel fraction is the whole-frame version of that test.

**Grid and hitch.** A 16×9 grid gives cells of 1/144 of the frame; the 0.111 threshold spans about sixteen. A partly covered cell sees a diluted change: under a full-strength flash it still counts once coverage passes about 10%, so edge error leans toward counting. That is the fail-safe direction, and why a coarser 8×8 grid (FFmpeg's) also passes the quarter-threshold constraint but judges area more coarsely. The hitch ceiling of 1/30 s is the slowest cadence hub AC 4 proves. A longer frame is treated as a hitch for the intensity allowance alone.

**Fixture safety.** A player payload ships only the maps the mod catalog names (`build_pipeline.md` §Distribution packaging), so an uncatalogued strobe map never reaches players. The SDK bundle ships `.map` sources, which a modder must build and load by name.

**Time.** `FrameTickResult::frame_dt` is raw elapsed time; only the sim accumulator is clamped. The renderer receives `now_seconds = script_time`, which dev-tools freezes. Precedent for a renderer-side clamped delta: `renderer_light_slots.rs` computes `(now - prev).clamp(0, 0.25)`.

## OS reader (`mundy` 0.2.3)

- License Apache-2.0, MSRV 1.80. Each preference has its own feature (`contrast`, `reduced-motion`, …). The Linux runtime is `async-io` (default) or `tokio`.
- `Preferences::subscribe(Interest, callback)` builds the stream on the calling thread, then runs it on a thread it spawns. The first callback is the initial value. Dropping the `Subscription` cancels it.
- macOS: requires the main thread (`MainThreadMarker::new().expect`). Values come from `NSWorkspace` accessibility properties, and changes arrive on the main run loop, which winit's `run_app` drives. winit 0.30.13 tolerates a prior `sharedApplication` call. Subscribe after the `EventLoop` exists.
- Windows: installs a `WH_CALLWNDPROC` hook on the main thread for `WM_SETTINGCHANGE` and friends. Reads run on a "mundy COM thread". No reported winit conflict (medium confidence). `once_blocking` ignores its timeout here and joins without a bound, so the bounded wait must be the App's own deadline on a channel.
- Linux: zbus against the XDG settings portal. A missing portal is logged and yields `NoPreference`, and reduced motion falls back to GNOME `enable-animations`.

## Co-op screen effects

- `flashScreen`, `vignette` and `screenShake` push `SystemReactionCommand`s (`register_system_reaction_primitives`). `App::dispatch_system_commands` starts `flash_decay` / `vignette_decay` / `shake_decay`. The decay ticks run in game logic on every peer. `screen.flash`, `screen.vignette` and `screen.shake` declare `ReplicationScope::None`.
- Crossing-driven reactions (`arena-lights.ts` low health) run on the owning client, after owner-private slots apply (`run_crossing_stage` → `App::dispatch_state_crossings`, not role-gated).
- Trigger-binding reactions run only inside the host's full `simulate_tick`; clients run `simulate_client_wieldable_tick`. A trigger-fired screen effect never reaches a client. Scope: `drafts/coop-trigger-screen-effects`.
- No reaction carries an owner or seat. Scaling at the pack step affects only the presenting machine.

## Audio


`crates/audio` (moved by E12). `Audio::new` builds `AudioManagerSettings` (kira 0.12) with the default `main_track_builder`, then runs `BusTree::build` (`BusId { Sfx, Music, UI }` under the main track). `Audio::set_bus_volume(BusId, dB)` and `Audio::set_main_volume(dB)` have test-only callers. Positional voices are spatial sub-tracks of SFX (`spatial.rs` `start_voice`), so a main-track mono effect composes after panning. No UI-sound play path exists. Music plays only when a script names the `music` bus.

## Pin symbols

Current symbols for the evidence in `E23--accessibility/research.md` §Brief pins U1, whose line citations are stale. All pins hold. `M` = `crates/postretro/src/main.rs`, `S` = `crates/postretro/src/startup/`.

| Pin | Current symbols | Note |
|---|---|---|
| Panel write persistence (B1) | `OptionsBridge::update_with_save`, `observe_changes`, `changed_value`, `flush_with_save`, `seed_on_open`; `OPTIONS_MENU_TREE_NAME`; `App::seed_options_menu_slots`, `App::update_player_options`, `App::options_menu_is_top` | Seeding and close flush key on `frontend.options` only. |
| Limiter attribution | `export_top_focus_rects` (renderer `render/ui/mod.rs`, `gameplay_trees.last()`), `Renderer::export_ui_focus_rects`, `FocusRectList` (no name or tier), `StackedTree` (name and tier), `resolve_text_entry_intents` → `commit_text_entry` / `cancel_text_entry` pop, `fire_focused_button_activation` → `focused_button_on_press` → `route_ui_button_action` → `classify_ui_button_action`, `apply_slider_nav_capture` → `capture_slider_step` | The export is stored after render N and consumed at N+1. |
| Global input: text entry and repeat | `WindowEvent::KeyboardInput` handler; `input::text_entry_key` (returns `None` for F1); `nav_intent_for_key` only when no text entry is open; gate `pressed && (!repeat \|\| text_entry_open)` | The gamepad path has no text-entry branch. |
| Global input: Loading frames | `pending_menu_toggle`, set without a boot-state gate and cleared by the Running pause block, `run_frontend_ui_logic` and constructors; `gp.update` only in the Running input tail and `run_frontend_ui_logic` | |
| Global input: same frame | `toggle_pause_menu` → `apply_pause_menu_nav_policy`; `take_ready` / `advance_frame` | |
| Slider capture | `nav_intent_for_gamepad_button` (Select → `NavIntent::Options`, only in `UiCaptureMode::Capture`), `reconcile_ui_focus`; swallowed in `capture_slider_step` | |
| Close paths | Running: `focus_result.cancelled` pops `PAUSE_MENU_NAME` or a frontend submenu; Frontend: `run_frontend_ui_logic` pops any non-root | `input.md` "cancel closes only an active `pauseMenu`" is stale. |
| First-launch hold | `BootState` (`S/mod.rs`); `App::drive_boot_state_for_redraw` drains level requests for Loading, Frontend and Running; `poll_world_less_transport` called per state (`run_splash_frame_zero`/`_one`, `run_loading_frame`, the Frontend branch in `M`) | |
| `--connect` vs backdrop | `enqueue_level_request` keeps the latest `Load` and drops requests during a boot-map load; `populate_frontend`; host `send_relevel` on `HandshakeOutcome::Admitted` (`crates/net/src/transport.rs`); client `App::follow_relevel_catalog` | Today the backdrop starts before any poll, so `Relevel` wins; the hold reverses that. |
| Boot anchors | `run_splash_frame_one`: `install_pending_session` → `finish_renderer_full_init` → `run_deferred_mod_init` → clear + `populate_frontend` or boot-map enqueue, all in one frame | `S/lifecycle.rs` holds ≈1,470 production lines. |
| Missing-entry warning | `UiTreeRegistry::register_with_presentation`; `ModalStack::register_script_trees` (mod init in `run_deferred_mod_init`, level load in `S/lifecycle_world_cpu.rs`); `replace_script_tree_tier` → `replace_tier` via `commit_staged_ui_manifest` | Trees commit before `session.frontend`. The existing warning `replace_with_frontend_menu` "not registered; using fallback" is precedent. |
| Load-loop splash edges | `Renderer::render_splash_frame`, `Renderer::clear_splash` → `BootSplashPass::clear` (`renderer_splash.rs`, `splash_pass.rs`); `run_loading_frame` → `paint_splash`; `clear_splash` from `finish_level_payload`, `finish_level_failure` and the no-map branch | Boot-map Loading frames show the logo; runtime Loading frames show the clear alone. |
| Resize | `ScreenEffectsPass::resize` recreates the texture and bind group and keeps `effect_buffer` | |
| GPU timing | `TIMING_PAIR_*` + `TIMING_PAIR_COUNT` (`pipeline_layout.rs`), `pass_labels` (`renderer_init_resources.rs`), `FrameTiming::render_pass_writes` / `compute_pass_writes` | The resolve is not in the set. |
| Test harness | `render/ui/gpu_test_harness.rs` (private); per-test copies in `curve_eval_test.rs`, `shadowmask_sample_test.rs`, `sdf_light_select_test.rs` | No harness runs `ScreenEffectsPass` with readback. |
| Mono fold seam | kira 0.12 `AudioManagerSettings::main_track_builder`; `MainTrackBuilder::with_effect` / `add_effect`; public `Effect` / `EffectBuilder` traits | A custom main-track effect is supported. `Audio::new` sets only `capacities`; the fold goes in `main_track_builder`. No built-in mono effect exists. |

## Ordering pins

Orderings the hub leaves to this brief (hub index §Pinned orderings, closing paragraph) and those review found implied. Acceptance rows cite the ids.

| Id | Scenario | Ordering | Pinned outcome |
|---|---|---|---|
| UO1 | An OS reply and a player write to the same field land in one frame (a mod-menu working-copy write or a panel action) | The App polls the OS channel at the top of the frame, before the Input stage → panel action at activation → command drain → options bridge | The player's write wins whichever arrives first. The field ends player-set at the player's value. A reseed never throws away a working-copy write the bridge has not yet observed. If a panel cycle returns the field to System that frame, the field resolves to that frame's OS reply. |
| UO2 | Reduce motion toggles while a shake is decaying and a UI tween is running | The toggle writes the store → the next pack applies scale 0 (on) or the slider value (off). The decay systems keep advancing `screen.shake`, which nothing else writes. | On: the next packed shake offset and the presented view feel are zero, and a tween already running reaches its target that frame. Off before the decay ends: the shake presents at its remaining decayed amplitude that frame. It does not restart and does not stay at zero. |
| UO3 | Mono is toggled back before its crossfade finishes | Toggle at T → reverse at T plus less than the crossfade length | The fold reverses from its current mix position. It returns within the elapsed part of the crossfade. No captured sample step is larger than the largest step in the untoggled signal. |
| UO4 | Both limiter stages act in one frame | Pack → channel clamp → effect uniform written → the measure pass composes with that uniform → the resolve applies the same uniform plus gain | The frame limiter measures what the resolve presents. A flash onset the clamp suppressed uses none of the frame limiter's budget. Both stages read the same frame's enable flag. |
| UO5 | The limiter is turned off, then on again within 1 s | Off: both stages pass content through. On: history starts that frame. | The first limited frame compares against the frame presented just before it. It never compares against the per-cell output stored when the limiter was turned off. No transition from the earlier on-period counts. |
| UO6 | A stretch of splash frames, or a hitch, longer than 1 s falls between two resolve frames | Splash frames present without the resolve → first resolve frame after them | Window time advances by the whole elapsed presented-frame time, splash frames included, so transitions older than 1 s leave the window. Only the intensity allowance is clamped to the hitch ceiling. |
| UO7 | A panel write at the first-launch hold, then the process quits or crashes with the panel still open | Panel write schedules the settled save → hold frames count down the settle → close writes the record | The field persists after the 250 ms settle while the panel is still open. The record is written only on close, so the next launch shows the panel again, with the player's value. |
| UO8 | A staged reload changes `frontend.menuTree`, or changes a frontend tree's buttons | Trees commit → frontend declaration commits → missing-entry check | The check reads the committed declaration. It names the new frontend tree when that tree lacks the button. A tree that has gained the button no longer warns. |
| UO9 | Mod init takes longer than 150 ms and no reply has arrived when it finishes | Mod init finishes → the wait starts → reply or 150 ms → splash clears | The 150 ms is counted from the moment mod init finishes. It is not counted from reader start, and not from a frame delta that includes mod init. A slow mod init still gets its full 150 ms wait. |
