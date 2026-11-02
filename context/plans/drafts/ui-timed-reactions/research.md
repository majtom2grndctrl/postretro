# ui-timed-reactions — research

Facts read at b6b85754c. These inform the brief without deciding it.

## Timed reactions today
- `ReactionScheduler` (`crates/sim/src/scripting_systems/reaction_scheduler.rs`) counts whole fixed ticks. The `wait` control handler registered by `register_reaction_control_primitives` converts ms with `ms_to_ticks` (rounds up, at least one tick) and calls `enroll`. `evaluate` decrements `remaining_ticks` once per sim tick, called from the fixed-tick loop in `frame_loop/mod.rs`; `drain_landings` resumes tails at the Running frame-end drain. `begin_frame` stamps enrollments so none advances in its own frame.
- The control handler sees `(address, body_ordinal, tail, args)` and nothing about the firing source. Origin comes from scoped scheduler state (`effective_origin`), populated only for trigger residuals. A clock chosen by source would need new plumbing into every dispatch site.
- `dispatch_sequence` (`scripting-core` `reaction_dispatch/sequence_dispatch.rs`) hands a `SequenceTarget::Wait` step to `get_control(&step.primitive)`, so the clock can already be chosen by primitive. The parsers in `data_descriptors/{js,lua}/reactions.rs` and `runtime_sequence_step_shape_is_valid` in `light_membership.rs` require `@wait` to pair with primitive `wait`. `partition_direct_reaction` stops at the first `SequenceTarget::Wait`, whatever its primitive.
- Lifetime: `clear()` runs from `clear_surface_lifetime_level_state` (`startup/lifecycle_net.rs`), reached by `unload_level` and the suspend path, and from the staged-manifest commit in `startup/staged_manifest_lifecycle.rs`, gated on `has_installed_level()`.
- Role: `Session` construction (`session/mod.rs`) calls `set_enabled(false)` for a `NetEndpoint::Client`, and `enroll` refuses. `reaction-body-composition` replaces that with a client role that parks host-mirrored and own-state tails and lands their machine-local steps.
- Caps: `MAX_PENDING_REACTION_INSTANCES` and `MAX_REACTION_CHAIN_DEPTH`, both 256.

## The frontend defect, sharpened
- The redraw path in `frame_loop/mod.rs` returns early for `BootState::Frontend | BootState::FirstLaunchHold` after `run_frontend_ui_logic` and `render_frontend_frame`, before the tick loop. `begin_frame` runs, `evaluate` never does, so a tail enrolled from a world-less frontend press never counts down there.
- `drain_level_requests` (`startup/lifecycle_boot_state.rs`) calls `unload_level` only from an installed state (Running or Settling). A load from the world-less frontend therefore never reaches `clear()`, and a parked frontend tail survives into the next level, where its ticks advance and it lands.
- With a frontend backdrop level, the frontend runs through Running, so the same tail lands in the backdrop on the tick clock.
- The hot-reload drop is skipped with no installed level, so a frontend tail also survives a staged reload.

## UI time today
- `ui.md` §3 calls UI time dt-accumulated game time. In source it is `App::script_time`, fed to `build_ui_read_snapshot` and from there to tween, `exitFade` and presentation-layout clocks (`time_seconds`). `script_time` advances only on the Running path, under the dev-tools freeze gate, and resets to zero at level install (`startup/lifecycle.rs`) and unload (`startup/lifecycle_net.rs`). `render_frontend_frame` and the world-less frame pass the same value, so UI time stands still in the world-less frontend and the first-launch hold. Frontend tweens are therefore frozen; `ui-tween-clock` owns that defect.
- Loading keeps its own `ui_time` in `startup/loading_screen.rs`, from zero each load, advanced by `frame_dt`.
- Hold-to-repeat and slider acceleration run on `frame_dt` passed to the focus engine, not on UI time.
- `frame_dt` (`FrameTiming::begin_frame`, `crates/sim/src/sim/frame_timing.rs`) is raw elapsed time; only the tick accumulator is capped (`MAX_ACCUMULATOR`, 250 ms).
- Pause is a capturing modal, not a sim pause (`ui.md` §4); E18 chose that waits advance while it is open.

## UI dispatch today
- A press reaches `fire_focused_button_activation_with_display_mode` (`app/ui_actions.rs`). Reserved `ui.*` actions are intercepted; anything else is `UiButtonAction::NamedReaction` → `fire_named_event_with_sequences` → `dispatch_deferred_named_events_with_sequences`. A text-entry commit takes the same path in `commit_text_entry`, then pops the keyboard.
- The session exists in Frontend and the hold, and `run_frontend_ui_logic` drains `dispatch_system_commands`, so system steps queued by a frontend press already apply there. Loading frames run `run_loading_frame` and drain nothing.
- `ModalStack` gives every push a `ModalInstance` (`active_instance`, `contains_instance`). Clears: `clear_pushed` (level start, first-launch hold), `replace_with_frontend_menu` (frontend return), `pop`, level-tier drops on unload. `replace_pushed_descriptor` keeps instance identity.
- `cellWrite` (`ui.createLocalState` cells) is a system reaction, so local presentation state is the natural target of menu choreography.

## Spelling
| Rule | How the clock is known | Same reaction from two sources | Verdict |
|---|---|---|---|
| Firing source decides; `wait()` from a UI press uses UI time | the caller | two clocks, lifetimes and cancel rules | Rejected: the caller-dependent meaning `scripting.md` §12 forbids; `fire(r)` from a press leaves `r`'s clock ambiguous |
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
| Raw UI time | advances | runs | lands a staged sequence unseen | Rejected: a 2 s hitch skips a 1 s dialog |
| UI time capped per frame (chosen) | advances | runs | stretches by the excess | Chosen: menus are used while paused; the flash limiter's presented-frame time and hitch-clamp precedent |

The cap equals the tick accumulator's cap. The clock advances on Loading frames because it measures UI time, but nothing lands there: Loading drains nothing and accepts no input. Rival: freeze it during Loading, so `[loadLevel(x), uiWait(500), …]` counts from the first playable frame. Rejected: the clock's meaning would depend on boot state, and `levelLoad` already owns "after the level is up".

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

## Placement
Two kinds of timing reach a menu. Visual timing moves pixels: a row fading in after the one above it. Functional pacing gates behavior: a click heard before the quit, a line held long enough to read with its own sound, a menu opened after a sting.

| Rule | Cascade | Ring-out, briefing, delayed open | Reduce motion | Verdict |
|---|---|---|---|---|
| Both in reactions: `uiWait` with a `decorative` flag that collapses under reduce motion | `uiWait` between cell writes | `uiWait(…, { decorative: false })` | per-wait flag; a forgotten flag cuts a ring-out or staggers a cascade | Rejected: a wait that collapses on one machine moves every step after it, so steps after a `uiWait` need a fence against shared state, and gameplay verbs need a rule that forces the wait functional |
| Visual timing in the tree, functional pacing in reactions (chosen) | tween `delayMs` and container `stagger` (`ui-tween-clock`) | `uiWait` | the tween rule snaps tree timing; a `uiWait` never collapses | Chosen: no flag, no reduce-motion carve-out, and the only fence left is lifetime (§Step class); matches `ui.md`'s rule that what the UI animates is retained-UI-local presentation |

Why reactions still need `uiWait`:
- A sound per line. A tween cannot play a sound; a step can.
- Opening a menu after a delay, or returning to the frontend after a ring-out. Only a reaction runs a UI-stack verb or a game-flow verb.
- One authoring model with `wait`: a timed beat is a step in a body, whichever clock counts it.

WCAG 2.3.3 (animation from interactions) concerns motion, not delay, so a `uiWait` that paces behavior owes reduce motion nothing.

## Step class after a `uiWait`
A `uiWait` never collapses, so its timing is the same on every machine that counts it, up to frame cadence. The remaining hazard is lifetime: a mod-global tail survives level unload and lands in the frontend or the next level.

| Rule | Mod-global `[uiWait(300), npcs().damage(5)]` from the pause menu | Mod-global `[uiWait(300), updateState(shared, 1)]` | Verdict |
|---|---|---|---|
| Any step, resolved at landing | hits whatever level is up when it lands, maybe the next one | lands | Rejected: outcome depends on lifecycle timing |
| Machine-local steps, game-flow verbs and `uiWait` only | install error | install error | Rejected: refuses slot writes and sentiment, whose targets outlive the level; that fence answers a wait that collapses on one machine, which a `uiWait` never does |
| Refuse, in a mod-global reaction, what level unload tears down (chosen) | install error | installs; the write lands on the host, in the frontend if the level has gone | Chosen: refuses exactly the steps a surviving tail could outlive |

What level unload tears down, read in `unload_level` and `clear_surface_lifetime_level_state` (`startup/lifecycle_net.rs`):
- The slot table is kept: "engine-global", with no clear method (the unload table's own comment), and `scripting.md` §Durable State Store says the store is never cleared on unload, suspend or hot reload. A `setState`, replicated or not, outlives the level.
- Faction sentiment lives in `ScriptCtx::faction_sentiment`, which unload does not touch; `data_registry.replace_factions` only rebases it, and it persists to disk beside the slot store. Sentiment steps outlive the level.
- `setGhostAudio` is level-scoped: `drop_script_ghost_audio` returns the bus to the manifest profile at unload. Refused.
- The entity registry is cleared (`clear_for_level_unload`), so groups resolve against the next level's entities. Refused. Map members cannot be named from a mod start script at all (`scripting.md` §12).
- A level `wait` counts sim ticks, which stop at unload. Refused.
- Presentation cells clear at unload, but a cell belongs to its tree's scope, not the level: `PresentationCellStore::reconcile` drops cells whose scope is no longer retained, so a write to a closed tree's scope is discarded at the next reconcile. Cell writes install.
- UI-stack verbs resolve a tree by name at landing; level-tier trees fall back to the mod or engine tier (`ui.md` §1.1). They install.

A level-defined tail drops at unload, so it never meets any of this, and suspend drops it too: suspend clears level surface state and re-drives the boot path, and the teardown's own comment says timed-reaction instances "hold level-A `EntityId`s". A level-defined `uiWait` tail may hold member steps, so it follows the same teardown.

The `fire` walk visits only mod-global bodies. A level-defined target resolves at landing against the active set: after unload it is absent and the fire does nothing, and in a later level it fires that level's reaction of the same name, as any `fire` by name does. The walk visits each reaction name once, so a self-firing chain terminates, and every active mod-global reaction of a fired name is walked, since same-name entries all fire (`scripting.md` §2).

## Interactions
- `reaction-body-composition` is the base: system steps, array bodies, the one `ReactionScheduler` with per-instance source class and tick clock, and the role-aware machine-local predicate. The example uses array bodies and system steps. Its "a wait changes when, never where" rule and its rules for steps after a `wait` cover `uiWait`. Cell writes are machine-local, since `cellWrite` routes to the app-side presentation cell store, never the slot table (`system_reactions.rs`).
  - UI time is a counting clock, not a key: the instance key stays by source class and tick clock, so a mixed body keeps one instance per key (ruling 2).
  - A client lands only machine-local steps after a `uiWait`, as after a `wait`, and an own-state drop warns once; that brief names `uiWait` as joining its warn-once.
  - A `uiWait` inside a tick tail enrolls when its machine lands that tail: on the host timeline for `levelLoad`, on the machine's own tick clock otherwise.
  - Late joiner: a joiner sees the level as it is now. When it converges a due tick segment, every later segment of that tail counts as due too: after a `uiWait` the host's landing times are per machine and never replicated, so the joiner has no due time to wait for. A client lands only machine-local steps, so the rest of the tail converges at the reading. Rival: count each later segment from the reading. Rejected: it replays presentation the host played long ago.
  - A level-defined tail cut at unload warns with its count so a cut beat reads as teardown, not a bug. A reload drops `uiWait` tails because a tail is a copy of a body the author may have edited.
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
| U7 | Reduce motion turned on with a tail parked | the tail lands at its due time, unchanged |
| U8 | 10-minute suspend mid-wait | the resume frame advances the clock by at most 250 ms |
| U9 | Level unload while a level-defined reaction is parked at an interruptible `uiWait` anchored to a level-tier tree | dropped by the unload and counted in its one warning; the anchor cancel at the next UI drain finds nothing |
| U10 | A mod-global and a level-defined tail parked, then `restartLevel` | the level-defined tail drops with one warning naming one; the mod-global tail lands after the reload |
| U11 | `[wait(300), playSound(a), uiWait(100), playSound(b)]` on loopback from `levelLoad` | each machine lands `a` at host `script_time` 300 ms ± one tick, enrolls the `uiWait` then, and plays `b` 100 ms of its own UI time after `a` |
| U12 | `[wait(300), playSound(a), uiWait(100), showDialog(d), playSound(b)]` from `levelLoad`, on a client whose first host-clock reading is at 10 s or at 350 ms | `d` shows at that reading; `a` and `b` skipped at debug; nothing counts from the reading |
| U13 | Own-state crossing `[wait(300), playSound(a), uiWait(100), showDialog(d)]` on a client | `a` 300 ms after its own crossing on its own tick clock; `d` 100 ms of its UI time later; the host neither sends nor lands the client's copy |

## Precedents
Recalled, not fetched.
- Quake's menus animate on `realtime` while game code runs on `cl.time`, which stops when paused: the 90s split between a menu clock and a game clock.
- Unity spells the unscaled delay distinctly: `WaitForSecondsRealtime` beside `WaitForSeconds`.
- Godot `SceneTree.create_timer` takes `process_always` and `ignore_time_scale` per call; Unreal timers pause with the game unless ticked while paused. Both put the clock at the delay's call site.

## Doors
- Repeating choreography (attract loops, blinking prompts) wants a loop step or a cap exemption for `uiWait` loops.
- A cancel verb naming a reaction's pending tail.

## Consumers
- `content/dev/scripts/pause-menu.ts` (QUIT), `frontend-menu.ts` (title PLAY opening the level select) and a new hub briefing tree host the example.
- Docs: `docs/scripting-reference.md` sequences and waits; `context/lib/scripting.md` §12, `ui.md` §3, §4 and `rendering_pipeline.md` §7.8 at promotion.

## Owner rulings

**1. Visual timing in the tree, functional pacing in reactions.** A tween delay and a list stagger are `ui-tween-clock`'s, and a `uiWait` paces behavior and never collapses (§Placement). What may follow a `uiWait` is then a lifetime question, not an authority one: a mod-global tail refuses only the steps level unload tears down (§Step class).
- Game-flow verbs stay legal after a `uiWait`. They change the session, not a level target, and a client's `returnToFrontend` is its own.
- Exit to desktop has no step form: `ui.exitToDesktop` is a reserved button action (`sdk/lib/ui/reactions.ts`), and reserved actions stay instant.
- The `fire` walk mirrors V4b's reach into targets, over definition site and step kind, so it needs only the registry and fits Pass A.
- Pass A runs only from `lifecycle_world_cpu.rs` (level install) and `staged_manifest_lifecycle.rs` (staged commit). A frontend-only reaction is never validated there, so the check must also run where the world-less frontend's active set composes.

**2. One scheduler.** One instance table, each instance counting on the clock of the wait it is parked at, keeps E18 O18 whole: the wait a body is parked at decides a re-fire, whether that wait counts ticks or UI time. `reaction-body-composition` rejects a second scheduler for the same reason.
- Rejected: two schedulers, one per clock. Each would hold its own tail per body, so a re-fire parked at one clock would enroll a duplicate on the other.
- Rejected: a clock-generic core run twice. It shares code, not the instance table, and keeps the duplicate.
- One table also means one instance cap across both clocks.

**3. Lifetime by definition site.** A level-defined reaction's definitions clear at unload while mod-global ones are kept (`boot_sequence.md` §4), so its tail outliving the level would be a beat from a level no longer loaded, the case E18 O38 rules out. O39's warning naming the count keeps a cut beat distinguishable from a bug. O40's reload reasoning applies to every `uiWait` tail, whatever its site. Mod-global reactions persist across levels, so their tails do too; quit and frontend-to-level presses are mod-global in practice (the dev pause menu registers through `content/dev/start-script.ts`).
- Rejected: session lifetime for every tail. A level's briefing would play its last line over the next level.
- Rejected: level lifetime for every tail. It would cut the Quit ring-out and every frontend-to-level transition beat.
- Suspend drops level-defined `uiWait` tails with its tick-clocked ones (O39), since a level-defined tail may hold member steps (§Step class). Mod-global tails survive it.
- `levels` scoping does not make a mod-global reaction level-defined; its tail survives even when the next level's tags exclude it.

**4. Clock name.** Every UI timer runs on one never-pausing session clock, named UI time, redefining `ui.md` §3 (`ui-tween-clock`). The flash limiter keeps presented-frame time, uncapped, because it measures exposure, not choreography (`rendering_pipeline.md` §7.8). Promotion amends `ui.md` §3 and `rendering_pipeline.md` §7.8 to name UI time.
