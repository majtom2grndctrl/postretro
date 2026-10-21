# ui-tween-clock — research

Facts read at 82a008480. These inform the brief without deciding it.

## UI time today
- `UiReadSnapshot::time_seconds` (`crates/ui/src/output.rs`) is the only UI time. It reaches the retained trees and presentation templates as `TweenClock::now` (`renderer_ui_layer.rs`). Readers:
  - tween drivers `drive_tween_f32` and the RGBA variant (`tree/bindings.rs`);
  - `BarExitFadeState` (`tree/node_context.rs`, driven in `ui_tree.rs` and `presentation_layout.rs`);
  - `styleRanges` pulse and flash, `StyleEffectState` (`scripting-core/src/ui/style_ranges.rs`). `pulse_factor` takes its phase from absolute `now`.
- There are three production snapshot sites. `build_ui_read_snapshot` takes `self.script_time` on the Running path and in `render_frontend_frame` (Frontend and the first-launch hold). `loading_screen_snapshot` takes `ActiveLoad::ui_time`.
- `App::script_time` (`main.rs`) advances by raw `frame_dt` only on the Running path. Its gate is `if !frozen` (the dev-tools `freeze_time`). It resets to zero at the end of install (`startup/lifecycle.rs`) and in unload (`startup/lifecycle_net.rs`). Its other readers are world-side: the GPU `time` uniform via `render_frame_indirect`, the light bridge, the emitter sim, the map-light collector, fog, and lightmap streaming `monotonic_seconds`.
- `RedrawRequested` returns early for `BootState::Frontend | BootState::FirstLaunchHold` after `run_frontend_ui_logic` and `render_frontend_frame`. That return comes before `script_time` advances, so frontend UI time holds at whatever value it had. After an unload that value is 0, and at boot it is 0 until a level runs.
- A frontend with a backdrop level runs through Running, so its menu animates. Without a backdrop the same menu tree stands still: whether a mod declares a backdrop changes whether its menu animates.
- `ActiveLoad::ui_time` starts at 0 in `begin_loading_screen` and grows by raw `frame_dt` in `advance_loading_screen`.
- `PresentationPool` (`crates/sim/src/sim/presentation_pool.rs`) keeps a third accumulator, `frame_time_seconds`. It grows by raw `frame_dt` (`valid_frame_delta`) on Running frames only. It drives instance age, lifetime fade, overlay linger and eviction. Its templates' tweens read the snapshot's `script_time`, so one instance runs on two clocks.
- `frame_dt` is raw. Only the tick accumulator is capped (`MAX_ACCUMULATOR`, 250 ms, `frame_timing.rs`). `about_to_wait` requests a redraw every loop, so world-less frames present continuously.
- Other clocks already measure rendered time: `advance_seat_hold_clock(frame_dt)`, which runs each redraw so Frontend and Loading keep seat holds moving, and the flash limiter's `next_limiter_frame`.

## Backward-clock defect
The tween sample is `(now - start_time) / duration`, clamped to `[0, 1]` in `style::apply`. If the clock jumps back under a retained tween, t clamps to 0, and the display holds the segment's start value until `now` passes `start_time` again. `BarExitFadeState::alpha_at` clamps the same way, so the bar holds at alpha 1. Retained trees are per layer in `RetainedGameplayTree` (`render/ui/retained_layers.rs`) and rebuild only when the descriptor or theme generation changes. Loading composes its tree alone at layer 0, which rebuilds layer 0 and truncates the rest. This defect is therefore reachable only when Loading falls back to the splash (no registered loading tree, or no full-ready renderer), since that path leaves the retained layers in place.

## Handoff premises checked
- UI time is `script_time`, level-relative, reset at install and unload, and frozen in the frontend and hold: true.
- Loading keeps its own `ui_time` from `frame_dt`: true.
- `ui.md` §3 says "UI time is dt-accumulated game time … pausing game logic pauses presentation": true.
- "`rendering_pipeline.md` §8.7 repeats it": false. §8 is Shader Module Composition and has no §8.7. The restatement is in §7.8, in the photosensitivity limiter: "not UI time, which pauses with game logic while frames keep presenting".
- Today's pause is not a true sim pause: true (`ui.md` §4; non-goal in `done/production-pause-menu`).
- Not in the handoff:
  - dev-tools freeze also freezes UI;
  - `script_time` takes raw (uncapped) frame time, so a hitch can finish a tween in one frame;
  - presentation instances keep a third accumulator;
  - `styleRanges` pulse and flash share the defect.

## Clock
| Clock | Frontend, hold | Loading | Future true pause: HUD | Future true pause: pause menu | Hitch | Verdict |
|---|---|---|---|---|---|---|
| `script_time` (today) | stands still | separate clock | freezes | freezes | raw | Rejected: the defect |
| (b) UI time that runs in the frontend but pauses with game logic in a level | runs | runs | freezes | freezes, unless the pause menu is special-cased | capped | Rejected: freezes the one menu a pause shows; a "pauses with game logic" rule has no source until a true pause exists |
| (c) clock chosen per tree | runs | runs | author's choice | author's choice | capped | Rejected for now: an author knob with no consumer; can be added on the tree envelope later |
| Raw presented time | runs | runs | settles | runs | skips | Rejected: a 2 s hitch skips a 1 s staged animation, out of step with `uiWait` |
| (a) Presented time capped per frame (chosen) | runs | runs | settles | runs | stretches | Chosen: one rule, matches `uiWait`, follows the limiter and seat-hold presented-time precedent |

## Future pause
The HUD shows authoritative slots (`ui.md` §3). A true pause stops the writes, so each HUD tween reaches its last target and stays there. The only HUD motion left running under a pause is a pulse or a flash already in flight. The pause menu is UI the player is using, so it must animate. Freezing every clock with game logic would leave the pause menu dead, so rule (b) would need the exception that (a) avoids. If a game later wants a frozen HUD under pause, it can hide the HUD, or the engine can add a tree-level opt-in.

## Precedents
Recalled, not fetched.
- Quake menus animate on `realtime` while `cl.time` stops when paused.
- Unity UI animates on `unscaledDeltaTime` so that menus run at `timeScale = 0`.
- Godot pause menus run with `process_mode = ALWAYS`.
- In-repo:
  - `done/M13--ui-value-tweening` non-goal: "pausing game logic pauses tweens, which is the wanted behavior for a presentational layer". Written before the engine had a frontend or Loading; this brief reverses it.
  - `done/mod-loading-screen-contract` invariant: "UI time advances on Loading frames". Kept.
  - `done/production-pause-menu` non-goal: freezing UI time. Kept.

## Loading fold
The per-load zero start exists only so Loading never touched `script_time` (`boot_sequence.md` §1). Tweens and exit fades measure from their own start, so starting a load at a later clock value changes nothing visible. A pulse is the exception, because its phase comes from absolute time. The loading screen test `loading_screen_tests.rs` advances by 0.016 and 1.0. With the cap, the 1.0 step advances 0.25 s, which still settles its 200 ms bar tween.

## Ordering pins
| Id | Scenario | Expected |
|---|---|---|
| T1 | Clock advance vs. UI logic in one frame | the clock advances at the frame head, before UI logic and every snapshot, so a tween retargeted by a press this frame starts at this frame's time |
| T2 | Install frame (one blocking frame) | the next frame advances the clock by at most 250 ms |
| T3 | Loading → Running with a loading tree | layer 0 rebuilds, so the HUD makes its first resolution: a snap, or a `from` flourish |
| T4 | Splash-fallback load | retained trees survive and keep easing; time never runs backward |
| T5 | Reduce motion turned on mid-tween | the tween reaches its target that frame; a fade in progress continues |

## Interactions
- `drafts/ui-timed-reactions` names the same clock for `uiWait`, and its Doors defer moving tweens onto it to this brief. One clock means a cascade of `uiWait` steps and the tweens they start stay in step under a hitch.
- The flash limiter keeps its own uncapped elapsed time for window aging. It measures exposure, so a hitch must age its window by the full time.
- Hold-to-repeat and slider acceleration take `frame_dt` in the focus engine. They set input cadence and are out of scope.

## Doors
- A tree-level `clock` opt-in, if a true pause gains a consumer that needs a frozen HUD.
- A true pause (sim, audio, particles), which is not built.
- A script-visible UI clock value, for authored time displays.
