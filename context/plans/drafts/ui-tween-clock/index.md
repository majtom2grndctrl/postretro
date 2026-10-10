# ui-tween-clock

Brief · compact · reads: `context/lib/ui.md` §3, §4 · `context/lib/boot_sequence.md` §1, §4 · `context/lib/rendering_pipeline.md` §7.8 · `context/lib/player_options.md` §5 · read at 82a008480

## Problem
A defect found while drafting `drafts/ui-timed-reactions`. Frontend menus do not animate: bound tweens, `Bar` `exitFade` and `styleRanges` pulse and flash stand still in the world-less frontend and the first-launch hold. The cause: UI time is `App::script_time`, a level clock that advances only on Running frames and resets to zero at level install and unload. The same reset runs the clock backward under any retained tree that survives a load, so its tween or fade holds until the clock catches up. When done, every UI timer reads one UI time that advances on every frame once the session exists and never pauses or resets. Frontend, hold, Loading and in-level UI all animate, a hitch delays an animation rather than skipping it, and reduce motion behaves as today.

## Decisions
- **One clock, UI time: a session clock that never pauses.** It replaces `ui.md` §3's "pausing game logic pauses presentation": under a future true pause, in-level HUD tweens settle on their last targets, pulses keep running and the pause menu keeps animating; a tree that must freeze can gain an additive opt-in when one exists. Each frame adds its elapsed time, capped at 250 ms (the tick accumulator's cap). The clock exists once the session does and advances on every frame from then on: Frontend, the first-launch hold, Loading and Running. Readers: bound tweens, `exitFade`, `styleRanges` pulse and flash, presentation-template tweens, presentation-instance lifetimes and fades, the loading tree, and `uiWait` (`drafts/ui-timed-reactions`). Rivals: `research.md` §Clock.
- **This reverses `ui.md` §3's "pausing game logic pauses presentation".** That line comes from `done/M13--ui-value-tweening`, written before the engine had a frontend, a Loading screen or a pause menu. No true pause exists (`ui.md` §4). Under a future true pause, sim slots stop changing, so HUD tweens settle on their last targets while pause-menu tweens keep running (`research.md` §Future pause). Owner confirmation is open question 1.
- **Per-tree clocks are a non-goal.** No consumer needs a tree that freezes with the game. An opt-in on the tree envelope would be an additive change later; one clock is the rule an author can predict.
- **Loading uses UI time.** The per-load clock and its zero start are removed. The zero start existed only so Loading never touched `script_time` (`boot_sequence.md` §1). Tweens and fades measure from their own start, so a load looks the same. A pulse's phase is not reset, which the player cannot see.
- **Nothing resets at install, unload, frontend return, staged reload or suspend.** The clock never decreases, so no retained tree sees time run backward. Display state follows the existing rule (`ui.md` §3). A tree that leaves composition rebuilds when it returns: Loading draws its tree alone, so the HUD snaps or plays its `from` flourish after each load. A tree that stays composed keeps easing.
- **The hitch cap applies to UI.** A 2 s frame advances UI by 250 ms, so a staged animation stretches instead of finishing unseen. Tweens also stay in step with a `uiWait` on the same clock. Today `script_time` takes raw frame time. The flash limiter keeps uncapped aging (`rendering_pipeline.md` §7.8), because it measures exposure, not choreography.
- **`script_time` stays the level clock.** The GPU time uniform, light bridge, emitters, fog and lightmap streaming keep it, along with its resets and the dev-tools freeze. The UI time ignores that freeze. The freeze holds world light phase aligned between CPU and GPU, and UI plays no part in that.
- **Reduce motion is unchanged.** Under it, a tween reaches its target on the frame it starts, a running tween included. `exitFade` still fades and pulse still runs (`ui.md` §3, `player_options.md` §5). The switch stays separate from the clock.
- **Placement.** The clock is session-owned and survives suspend. The App advances it once at the frame head, before UI logic and before any snapshot. Every UI snapshot site and the presentation pool read that value. It is machine-local and never replicated, so the net wire is unchanged. `drafts/ui-timed-reactions` uses the same clock, and whichever brief builds first owns it.
- **Non-goals.** A true pause. A script-visible clock value. UI time scaling. Hold-to-repeat and slider acceleration stay on input frame time, because they set input cadence and do not animate (`ui.md` §4).
- **Docs owed at promotion.** `ui.md` §3: replace the UI-time bullet. `ui.md` §4: name UI time in the pause paragraph. `rendering_pipeline.md` §7.8: fix the limiter's "UI time, which pauses with game logic" aside. `boot_sequence.md` §1 Loading screen: replace the own-clock sentence. `docs/scripting-reference.md`: one sentence at the tween bind saying tweens and fades run in menus and while paused. `player_options.md` §5 is unchanged.

## Acceptance
### Automated
- [ ] Frontend with no level: a tween easing 0→100 over 500 ms reads strictly between 0 and 100 after 250 ms of frames, and exactly 100 from 500 ms. A bar's 300 ms exit fade has alpha strictly between 0 and 1 at 150 ms and draws nothing from 300 ms. Today the tween reads 0 and the bar never fades.
- [ ] Frontend with no level: a pulse with an 800 ms period draws different alphas at 0 ms and 400 ms.
- [ ] First-launch hold: a tween in a hold-frame tree meets the same two predicates as the frontend row.
- [ ] Loading: a loading-tree tween eases over its authored duration across Loading frames. The UI time on the first Running frame after install exceeds its value when the load began.
- [ ] Level install mid-tween, loading tree registered: a HUD tween in flight when the load begins shows, after install, the HUD's first resolution (its target, or the start of its `from` flourish), never the pre-load display value.
- [ ] Level install mid-tween, no loading tree registered (splash fallback): a surviving tree's in-flight tween resumes from its displayed value and settles within its remaining duration, never holding.
- [ ] Unload mid-tween: quitting to a world-less frontend while a pause-menu tween runs leaves the frontend menu's tweens easing over their authored durations. The clock after the unload frame is at least its value before.
- [ ] Pause menu open over a level: HUD and pause-menu tweens settle at their authored durations whether each frame carries 0, 1 or 3 sim ticks.
- [ ] Reduce motion on: in frontend, hold, Loading and Running, a starting or running tween shows its target that frame, and a 300 ms exit fade still takes 300 ms. Off: the tween eases.
- [ ] A 250 ms spike: one 2 s frame advances the clock by exactly 250 ms. A 1000 ms tween 400 ms in reads short of its target afterwards and settles 350 ms later. A presentation instance ages 250 ms, not 2 s.
- [ ] A ten-minute suspend: the resume frame advances the clock by at most 250 ms.
- [ ] Dev-tools freeze on: world light animation holds while a HUD tween eases.
- [ ] Monotonic: across boot → frontend → load → Running → staged reload → unload → load, the clock never decreases.
- [ ] One clock: on a single frame, the published UI snapshot and the presentation pool read the value UI time holds. A grep gate confirms no other UI-time accumulator remains.

### Manual
- [ ] Dev mod title menu, with and without a backdrop level: entry tweens and exit fades animate the same in both. The pause menu and loading bar feel unchanged.

## Path
- **Seams.** Advance the clock at the `RedrawRequested` frame head beside `advance_seat_hold_clock`. The `build_ui_read_snapshot` callers (Running, `render_frontend_frame`) take it in place of `script_time`. `loading_screen_snapshot` reads it, and `ActiveLoad::ui_time` and its advance in `advance_loading_screen` go. `PresentationPool::advance_and_collect_inputs` reads it in place of its own `frame_dt` sum. The cap shares `MAX_ACCUMULATOR` (`frame_timing.rs`). Restate the doc comments on `UiReadSnapshot::time_seconds`, `TweenClock`, `StyleEffectState` and `App::script_time`.
- **Shape.** One session field, advanced once per frame and read everywhere. Rival: keep per-surface accumulators fed the capped delta. That needs less plumbing, but the accumulators drift apart on frames one of them skips.
- **First slice.** Frontend with no level: a tween and an exit fade complete. This falsifies the riskiest assumption, that nothing downstream relies on frontend UI time being zero.
- **Split first.** `main.rs`, `session/mod.rs` and `presentation_pool.rs` are past ~800 lines. Split only the ones this change extends, behavior-preserving, each in its own commit. Coordinate with `ui-timed-reactions`, which splits the same files.

## Open questions
- None.
