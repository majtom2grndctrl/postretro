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
- `ui.md` §3 calls UI time dt-accumulated game time. In source it is `App::script_time`, fed to `build_ui_read_snapshot` and from there to tween, `exitFade` and presentation-layout clocks (`time_seconds`). `script_time` advances only on the Running path, under the dev-tools freeze gate, and resets to zero at level install (`startup/lifecycle.rs`) and unload (`startup/lifecycle_net.rs`). `render_frontend_frame` and the world-less frame pass the same value, so UI time stands still in the world-less frontend and the first-launch hold. Frontend tweens are therefore frozen today; `ui-tween-clock` owns that defect. The title cascade's steps stagger on presented time either way, but its rows' tweens animate there only once that brief lands.
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
| Firing source decides; `wait()` from a UI press uses presented time | the caller | two clocks, lifetimes and cancel rules | Rejected: the caller-dependent meaning `scripting.md` §12 forbids; `fire(r)` from a press leaves `r`'s clock ambiguous |
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
| Rule | Pause-menu `[uiWait(300), npcs().damage(5)]` | Trigger-fired `[uiWait(300), updateState(sharedSlot, 1)]` | Verdict |
|---|---|---|---|
| Any step, resolved at landing | hits whatever level is up when it lands, maybe the next one | lands earlier on a reduce-motion host | Rejected: outcome depends on lifecycle timing and on one player's preference |
| Session-scoped steps, a later level `wait` re-opening level steps | install error | installs; the shared write moves with the host's switch | Rejected: authority leak (ruling 1) |
| Machine-local steps, game-flow verbs and `uiWait` only (chosen) | install error | install error | Chosen: a load-time error; no preference reaches shared state |

`fire(r)` after a `uiWait` is checked through every body its chain reaches, each reached body held to the same step class, since it all lands after the wait. The walk visits each reaction name once, so a self-firing chain terminates; every active reaction of a fired name is walked, since same-name entries all fire (`scripting.md` §2). The chain resolves against the active set at install; what a fire landing after a level change does when its target has left the set is the executor's to report.

## Reduce motion
| Rule | Click ring-out | Reading-paced briefing | Cascade | Verdict |
|---|---|---|---|---|
| Never collapse | kept | kept | still staggers (rows snap, timing stays) | Rival: safe, under-serves the switch |
| Always collapse | cut | dumps every line | instant | Rejected: changes behavior, not motion |
| Functional default, `decorative` opts in | kept | kept | instant | Rival: a forgotten flag leaves a cascade staggering |
| Decorative default, `decorative: false` opts out (chosen, owner ruling) | kept when marked | kept when marked | instant | Chosen: menu choreography is mostly motion, so the common case needs no option; a forgotten opt-out cuts a ring-out or reading time for reduce-motion players only |

WCAG 2.3.3 (animation from interactions) concerns motion, not delay. A collapsed wait behaves as if absent; a parked decorative tail lands at the next UI drain when the switch turns on, as a running tween snaps (`ui.md` §3).

## Interactions
- `reaction-body-composition`: system steps in sequences and array bodies are prerequisites; the example uses both. Its "a wait changes when, never where" rule is adopted for `uiWait`. Its client warn-once for UI-press `wait` tails stands, with `uiWait` named as the fix. Its machine-local predicate decides what may follow a `uiWait` (ruling 1); cell writes count as presentation, since `cellWrite` routes to the app-side presentation cell store, never the slot table (`system_reactions.rs`).
- E18: V1 covers `uiWait` durations. V4a and V4b key on a wait, so they cover `uiWait` once the sentinel admits it. V2, V3 and V5 must match primitive `wait`; read by sentinel alone, V3 would drop every interruptible `uiWait` in a UI-fired reaction. Re-fire rules, caps and enrollment-frame protection carry over.
- A trigger-bound body partitions at its first wait of either kind, so pre-`uiWait` consequential steps still bind in-tick.
- `E16--player-events`: a player-event reaction's presentation targets one player's machine. A UI tail is machine-local, so on the host it would land on the host's screen; E16's machine-local rejection covers it.

## Ordering pins
| Id | Scenario | Expected |
|---|---|---|
| U1 | Press on frame F, `uiWait(5)` | lands on frame F+1's UI drain, never F |
| U2 | Interruptible wait, anchor pops on the frame the clock reaches the duration | cancelled; the anchor check precedes landing |
| U3 | Two presses of different reactions on one frame, equal durations | land in press order |
| U4 | With a menu open, name-fired `[a, wait(500), b, uiWait(100, { interruptible: true }), c]`, re-fired while parked at each wait | parked at the level wait: ignored (O7). Parked at the `uiWait`: the one instance is cancelled and a fresh one runs from the top, re-applying `a`. One instance exists throughout; the parked-at wait governs (O18) |
| U5 | `[returnToFrontend(), uiWait(500, { interruptible: true }), showDialog(t)]` | the anchor resolves after the return applies, to the frontend menu; `t` shows |
| U6 | `[loadLevel(x), uiWait(200), playSound(y)]` where Loading outlasts 200 ms | `y` plays on the first frame after Loading, not during it |
| U7 | Reduce motion turned on with decorative and functional tails parked | decorative lands at the next UI drain; functional unchanged |
| U8 | 10-minute suspend mid-wait | the resume frame advances the clock by at most 250 ms |
| U9 | Level unload while a level-defined reaction is parked at an interruptible `uiWait` anchored to a level-tier tree | dropped by the unload and counted in its one warning; the anchor cancel at the next UI drain finds nothing |
| U10 | A mod-global and a level-defined tail parked, then `restartLevel` | the level-defined tail drops with one warning naming one; the mod-global tail lands after the reload |
| U11 | `[wait(300), playSound(a), uiWait(100), playSound(b)]` on loopback from `levelLoad` | each machine enrolls the `uiWait` when it lands the level tail, and plays `b` 100 ms of its own presented time after `a` |

## Precedents
Recalled, not fetched.
- Quake's menus animate on `realtime` while game code runs on `cl.time`, which stops when paused: the 90s split between a menu clock and a game clock.
- Unity spells the unscaled delay distinctly: `WaitForSecondsRealtime` beside `WaitForSeconds`.
- Godot `SceneTree.create_timer` takes `process_always` and `ignore_time_scale` per call; Unreal timers pause with the game unless ticked while paused. Both put the clock at the delay's call site.

## Doors
- Tween and `exitFade` time stands still in the world-less frontend because it is `script_time`. `ui-tween-clock` owns it; moving tweens onto presented time would reverse `ui.md` §3's "pausing game logic pauses presentation".
- Repeating choreography (attract loops, blinking prompts) wants a loop step or a cap exemption for presented-time loops.
- A cancel verb naming a reaction's pending tail.
- `decorative` on level `wait`s, if a gameplay cascade wants it.

## Consumers
- `content/dev/scripts/pause-menu.ts` (QUIT), `frontend-menu.ts` (title, level select) and a new hub briefing tree host the example.
- Docs: `docs/scripting-reference.md` sequences and waits; `context/lib/scripting.md` §12, `ui.md` §3, §4 and `rendering_pipeline.md` §7.8 at promotion.

## Direction review rulings
Owner rulings after `/validate-plan`. Facts below read at 82a008480.

**1. Authority after a `uiWait`.** A reaction is sourceless, so a trigger, crossing or `levelLoad` can reach a `uiWait`, and a decorative one collapses per machine under reduce motion. Any authoritative step after it would land earlier on a host whose player turned the switch on, so one player's preference would move a shared effect, which E23 I9 and `player_options.md` §5 forbid. The rule therefore admits only steps whose timing is machine-local (`reaction-body-composition`'s predicate), game-flow verbs and further `uiWait`s.
- Rejected: session-scoped steps, the earlier draft. It barred level addressing but let replicated `setState` and sentiment through.
- Rejected: admitting authoritative steps after a functional (`decorative: false`) `uiWait`. Legality would hinge on a flag whose default is decorative, and a shared effect would ride a clock that keeps running through a future true pause while the world freezes.
- Game-flow verbs stay legal. They change the session, not replicated state, and a client's `returnToFrontend` is its own. A host `loadLevel` after a decorative `uiWait` would land sooner under the host's reduce motion, and every peer follows it. Settled structurally: every `uiWait` before a game-flow verb in its body is functional. Rival: require `decorative: false` there as an install error — explicit, but it errors on the default and burdens every author for a rule the engine can apply itself.
- Exit to desktop has no step form: `ui.exitToDesktop` is a reserved button action (`sdk/lib/ui/reactions.ts`), and reserved actions stay instant.
- Mixed bodies, same rule. A level `wait` after a `uiWait` is authoritative timing behind a per-machine clock, so it is barred. A `uiWait` after a level `wait` is legal: the level tail lands on the machines `reaction-body-composition` gives it, and each enrolls the `uiWait` there.
- The `fire` walk mirrors V4b's reach into targets, but over step class, so it needs only the registry and slot declarations and fits Pass A.
- Pass A runs only from `lifecycle_world_cpu.rs` (level install) and `staged_manifest_lifecycle.rs` (staged commit). A frontend-only reaction is never validated there, so the check must also run where the world-less frontend's active set composes.

**2. One scheduler.** One instance table, each instance carrying its clock, keeps E18 O18 whole: the wait a body is parked at decides a re-fire, whether that wait counts ticks or presented time.
- Rejected: two schedulers, one per clock. Each would hold its own tail per body, so a re-fire parked at one clock would enroll a duplicate on the other. The earlier U4 pinned exactly that divergence.
- Rejected: a clock-generic core run twice. It shares code, not the instance table, and keeps the duplicate.
- One table also means one instance cap across both clocks.

**3. Lifetime by definition site.** A level-defined reaction's definitions clear at unload while mod-global ones are kept (`boot_sequence.md` §4), so its tail outliving the level would be a beat from a level no longer loaded, the case E18 O38 rules out. O39's warning naming the count keeps a cut beat distinguishable from a bug. O40's reload reasoning applies to every presented-time tail, whatever its site. Mod-global reactions persist across levels, so their tails do too; quit and frontend-to-level presses are mod-global in practice (the dev pause menu registers through `content/dev/start-script.ts`).
- Rejected: session lifetime for every tail. A level's briefing would play its last line over the next level.
- Rejected: level lifetime for every tail. It would cut the Quit ring-out and every frontend-to-level transition beat.
- Suspend is not an unload, so it keeps level-defined presented-time tails, though it drops tick-clocked ones (O39).
- `levels` scoping does not make a mod-global reaction level-defined; its tail survives even when the next level's tags exclude it.

**4. Clock name.** "Presented time" matches `rendering_pipeline.md` §7.8, which already distinguishes presented-frame time from UI time for the flash limiter. "UI clock" would collide with UI time, the tween clock that pauses with game logic. Promotion amends `ui.md` §3 and `rendering_pipeline.md` §7.8 to name both. Frontend tweens are frozen today because `App::script_time` is level-relative; `ui-tween-clock` owns that, and the cascade example says so.
