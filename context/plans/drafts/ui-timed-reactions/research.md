# ui-timed-reactions — research

Facts read at a76d99e15. These inform the brief without deciding it.

## Timed reactions today
- `ReactionScheduler` (`crates/sim/src/scripting_systems/reaction_scheduler.rs`) counts whole fixed ticks. The `wait` control handler registered by `register_reaction_control_primitives` converts ms with `ms_to_ticks` (rounds up, at least one tick) and calls `enroll`. `evaluate` decrements `remaining_ticks` once per sim tick, called from the fixed-tick loop in `main.rs`; `drain_landings` resumes tails at the Running frame-end drain. `begin_frame` stamps enrollments so none advances in its own frame.
- The control handler sees `(address, body_ordinal, tail, args)` and nothing about the firing source. Origin comes from scoped scheduler state (`effective_origin`), populated only for trigger residuals. A clock chosen by source would need new plumbing into every dispatch site.
- `dispatch_sequence` (`scripting-core` `reaction_dispatch/sequence_dispatch.rs`) hands a `SequenceTarget::Wait` step to `get_control(&step.primitive)`, so the clock can already be chosen by primitive. The parsers in `data_descriptors/{js,lua}/reactions.rs` and `runtime_sequence_step_shape_is_valid` in `light_membership.rs` require `@wait` to pair with primitive `wait`. `partition_direct_reaction` stops at the first `SequenceTarget::Wait`, whatever its primitive.
- Lifetime: `clear()` runs from `clear_surface_lifetime_level_state` (`startup/lifecycle_net.rs`), reached by `unload_level` and the suspend path, and from the staged-manifest commit in `startup/staged_manifest_lifecycle.rs`, gated on `has_installed_level()`.
- Role: `Session` construction (`session/mod.rs`) calls `set_enabled(false)` for a `NetEndpoint::Client`; `enroll` then refuses and warns once per reaction.
- Caps: `MAX_PENDING_REACTION_INSTANCES` and `MAX_REACTION_CHAIN_DEPTH`, both 256.

## The frontend defect, sharpened
- `RedrawRequested` in `main.rs` returns early for `BootState::Frontend | BootState::FirstLaunchHold` after `run_frontend_ui_logic` and `render_frontend_frame`, before the tick loop. `begin_frame` runs, `evaluate` never does, so a tail enrolled from a world-less frontend press never counts down there.
- `drain_level_requests` (`startup/lifecycle_boot_state.rs`) calls `unload_level` only from `BootState::Running`. A load from the world-less frontend therefore never reaches `clear()`, and a parked frontend tail survives into the next level, where its ticks advance and it lands. The handoff's "parks and never lands" holds only until a level loads.
- With a frontend backdrop level, the frontend runs through Running, so the same tail lands in the backdrop on the host and is refused on a client.
- The hot-reload drop is skipped with no installed level, so a frontend tail also survives a staged reload.

## UI time today
- `ui.md` §3 calls UI time dt-accumulated game time. In source it is `App::script_time`, fed to `build_ui_read_snapshot` and from there to tween, `exitFade` and presentation-layout clocks (`time_seconds`). `script_time` advances only on the Running path, under the dev-tools freeze gate, and resets to zero at level install (`startup/lifecycle.rs`) and unload (`startup/lifecycle_net.rs`). `render_frontend_frame` and the world-less frame pass the same value, so UI time stands still in the world-less frontend and the first-launch hold.
- Loading keeps its own `ui_time` in `startup/loading_screen.rs`, from zero each load, advanced by `frame_dt`.
- Hold-to-repeat and slider acceleration run on `frame_dt` passed to the focus engine, not on UI time.
- `frame_dt` (`FrameTiming::begin_frame`, `crates/sim/src/sim/frame_timing.rs`) is raw elapsed time; only the tick accumulator is capped (`MAX_ACCUMULATOR`, 250 ms).
- Reduce motion reaches the UI as `UiReadSnapshot::reduce_motion`, built from the `accessibility.reduceMotion` slot by `reduce_motion_from_slots`.
- Pause is a capturing modal, not a sim pause (`ui.md` §4); E18 chose that waits advance while it is open.

## UI dispatch today
- A press reaches `fire_focused_button_activation_with_display_mode` (`app/ui_actions.rs`). Reserved `ui.*` actions are intercepted; anything else is `UiButtonAction::NamedReaction` → `fire_named_event_with_sequences` → `dispatch_deferred_named_events_with_sequences`. A text-entry commit takes the same path in `commit_text_entry`, then pops the keyboard.
- The session exists in Frontend and the hold, and `run_frontend_ui_logic` drains `dispatch_system_commands`, so system steps queued by a frontend press already apply there. Loading frames run `run_loading_frame` and drain nothing.
- `ModalStack` gives every push a `ModalInstance` (`active_instance`, `contains_instance`). Clears: `clear_pushed` (level start, first-launch hold), `replace_with_frontend_menu` (frontend return), `pop`, level-tier drops on unload. `replace_pushed_descriptor` keeps instance identity.
- `cellWrite` (`ui.createLocalState` cells) is a system reaction, so local presentation state is the natural target of menu choreography.

## Spelling
| Rule | How the clock is known | Same reaction from two sources | Verdict |
|---|---|---|---|
| Firing source decides; `wait()` from a UI press uses the UI clock | the caller | two clocks, lifetimes and cancel rules | Rejected: the caller-dependent meaning `scripting.md` §12 forbids; `fire(r)` from a press leaves `r`'s clock ambiguous |
| Declaration site decides: reactions under a UI manifest key, or referenced by a button | which list returned it | a handle returned through both lists forks | Rejected: structural but non-local, and splits the registry |
| Per-reaction option, `defineReaction(name, body, { clock: "ui" })` | an option away from the step | same everywhere | Rival: lexical, but the reader of `wait(300)` must check the definition; no mixed bodies |
| Per-tree declared clock | the tree | a reaction is not owned by a tree | Rejected: reactions are referenced by trees, not owned |
| Distinct step, `uiWait` (chosen) | the step itself | same everywhere | Chosen: lexical and loud at the delay; mixed bodies work; precedents below |

Name: `uiWait` pairs with `wait`, and the prefix names the clock. Exporting it as `wait` from `postretro/ui` would make meaning depend on an import; `delay`, `hold` and `pause` collide with existing terms (hold-to-repeat, first-launch hold, pause menu).

## Clock
| Clock | World-less frontend | Future true pause | Hitch | Verdict |
|---|---|---|---|---|
| Fixed ticks (`wait`) | never advances | freezes | stretches | level beats only |
| Tween UI time (`script_time`) | stands still | pauses with game logic | raw | Rejected: never lands in the frontend, resets per level |
| Raw presented time | advances | runs | lands a staged sequence unseen | Rejected: a 2 s hitch skips a 1 s dialog |
| Presented time capped per frame (chosen) | advances | runs | stretches by the excess | Chosen: menus are used while paused; the flash limiter's presented-time and hitch-clamp precedent |

The cap equals the tick accumulator's cap. The clock advances on Loading frames because it measures presented time, but nothing lands there: Loading drains nothing and accepts no input. Rival: freeze it during Loading, so `[loadLevel(x), uiWait(500), …]` counts from the first playable frame. Rejected: the clock's meaning would depend on boot state, and `levelLoad` already owns "after the level is up".

## Cancellation
| Anchor for an interruptible wait | `[openMenu(briefing), uiWait(i), …]` | From `levelLoad` | Verdict |
|---|---|---|---|
| The tree whose button fired | anchored to the hub, so closing the briefing cancels nothing | no anchor | Rejected: source-dependent, and wrong for the commonest case |
| Any stack change | a submenu push cancels | cancels on any push | Rejected: unpredictable |
| Stack resets only (frontend return, level start) | survives closing the briefing | a game-flow step in the body cancels its own later tail | Rejected |
| Top pushed tree once the steps before the wait applied (chosen) | anchored to the briefing | anchored to whatever shows | Chosen: matches what the player sees; sourceless |

- Pre-wait system steps queue during the walk and apply at the frame's system drain, so the anchor resolves after that drain, not at enrollment.
- A later interruptible wait in the same tail re-resolves its anchor after its own preceding steps.
- A tree hidden by `hideBelow` stays on the stack and keeps its anchors. A rebuilt-in-place tree keeps its instance.
- Uninterruptible is the default, matching `wait`: click-then-quit should be committed.
- An interruptible wait with no pushed tree has nothing to belong to; it runs uninterruptible and warns once, like E18's sourceless demotion. Install cannot tell which reactions a tree will show.
- A text-entry commit fires its reaction, then pops the keyboard before the frame's system drain, so an interruptible wait there anchors to the tree beneath, the one that opened the keyboard. No special case.

## Step class after a `uiWait`
| Rule | Pause-menu `[uiWait(300), npcs().damage(5)]` | Frontend `[uiWait(300), door.start()]` | Verdict |
|---|---|---|---|
| Any step, resolved at landing | hits whatever level is up when it lands, maybe the next one | warn-skips a stale id | Rival: maximally enabling; outcome depends on lifecycle timing |
| Session-scoped steps only (chosen) | install error naming `wait` | install error | Chosen: a load-time error instead of a lifecycle-timing outcome |

`fire(r)` stays legal: `r` runs now by name, under its own rules. A level `wait` later in the body re-opens level-scoped steps after it, under level rules, and needs a level when reached.

## Reduce motion
| Rule | Click ring-out | Reading-paced briefing | Cascade | Verdict |
|---|---|---|---|---|
| Never collapse | kept | kept | still staggers (rows snap, timing stays) | Rival: safe, under-serves the switch |
| Always collapse | cut | dumps every line | instant | Rejected: changes behavior, not motion |
| Functional default, `decorative` opts in (chosen) | kept | kept | instant | Chosen: the forgotten flag degrades mildly |

WCAG 2.3.3 (animation from interactions) concerns motion, not delay. A collapsed wait behaves as if absent; a parked decorative tail lands at the next UI drain when the switch turns on, as a running tween snaps (`ui.md` §3).

## Interactions
- `reaction-body-composition`: system steps in sequences and array bodies are prerequisites; the example uses both. Its "a wait changes when, never where" rule is adopted for `uiWait`. Its client warn-once for UI-press `wait` tails stands, with `uiWait` named as the fix. Its machine-local predicate is not needed here, since every machine runs its own UI tails.
- E18: V1 covers `uiWait` durations. V4a and V4b key on a wait, so they cover `uiWait` once the sentinel admits it. V2, V3 and V5 must match primitive `wait`; read by sentinel alone, V3 would drop every interruptible `uiWait` in a UI-fired reaction. Re-fire rules, caps and enrollment-frame protection carry over.
- A trigger-bound body partitions at its first wait of either kind, so pre-`uiWait` consequential steps still bind in-tick.
- `E16--player-events`: a player-event reaction's presentation targets one player's machine. A UI tail is machine-local, so on the host it would land on the host's screen; E16's machine-local rejection covers it.

## Ordering pins
| Id | Scenario | Expected |
|---|---|---|
| U1 | Press on frame F, `uiWait(5)` | lands on frame F+1's UI drain, never F |
| U2 | Interruptible wait, anchor pops on the frame the clock reaches the duration | cancelled; the anchor check precedes landing |
| U3 | Two presses of different reactions on one frame, equal durations | land in press order |
| U4 | Mixed `[a, uiWait(100), b, wait(500), c]`, re-fired while parked at the level wait | the UI scheduler holds nothing for the body, so the re-fire enrolls a fresh UI instance; each clock holds one tail per body |
| U5 | `[returnToFrontend(), uiWait(500, { interruptible: true }), showDialog(t)]` | the anchor resolves after the return applies, to the frontend menu; `t` shows |
| U6 | `[loadLevel(x), uiWait(200), playSound(y)]` where Loading outlasts 200 ms | `y` plays on the first frame after Loading, not during it |
| U7 | Reduce motion turned on with decorative and functional tails parked | decorative lands at the next UI drain; functional unchanged |
| U8 | 10-minute suspend mid-wait | the resume frame advances the clock by at most 250 ms |

## Precedents
Recalled, not fetched.
- Quake's menus animate on `realtime` while game code runs on `cl.time`, which stops when paused: the 90s split between a menu clock and a game clock.
- Unity spells the unscaled delay distinctly: `WaitForSecondsRealtime` beside `WaitForSeconds`.
- Godot `SceneTree.create_timer` takes `process_always` and `ignore_time_scale` per call; Unreal timers pause with the game unless ticked while paused. Both put the clock at the delay's call site.

## Doors
- Tween and `exitFade` time stands still in the world-less frontend because it is `script_time`. Moving tweens onto the UI clock would fix frontend tweens but reverse `ui.md` §3's "pausing game logic pauses presentation"; a separate decision.
- Repeating choreography (attract loops, blinking prompts) wants a loop step or a cap exemption for presented-time loops.
- A cancel verb naming a reaction's pending tail.
- `decorative` on level `wait`s, if a gameplay cascade wants it.

## Consumers
- `content/dev/scripts/pause-menu.ts` (QUIT), `frontend-menu.ts` (title, level select) and a new hub briefing tree host the example.
- Docs: `docs/scripting-reference.md` sequences and waits; `context/lib/scripting.md` §12 and `ui.md` §3, §4 at promotion.
