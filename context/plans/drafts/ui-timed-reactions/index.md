# ui-timed-reactions

Brief · compact · reads: `context/lib/ui.md` §3, §4 · `context/lib/scripting.md` §2, §12 · `context/lib/boot_sequence.md` §1 · `context/lib/player_options.md` §5 · `context/lib/rendering_pipeline.md` §7.8 · read at a76d99e15

## Problem
A requested capability, raised by the owner. Menus need timed beats: a click that rings out before the pause menu quits, a briefing staged line by line, a cascade of menu items. The only `wait` counts fixed sim ticks on a level-lifetime, host-only scheduler. In the world-less frontend no tick runs, so a frontend press's tail parks, then lands inside whichever level loads next; on a connected client it is refused. When done, a UI wait is its own step counted in UI time on each machine. It lands with or without a level, on every machine, keeps pacing while paused, outlives a level when its reaction does, cancels with its screen when asked to, and collapses under reduce motion when it only paces motion.

## Decisions
- **A UI wait has its own spelling: `uiWait(durationMs, opts?)`, from `postretro/ui`.** The step names its clock, so a reaction means the same thing from every source (`scripting.md` §12, a reaction is sourceless). Choosing the clock by firing source would give one `wait()` different clocks, lifetimes and cancel rules per caller; rivals in `research.md` §Spelling.
- **The clock is UI time,** the never-pausing session clock `ui-tween-clock` defines. It sums each presented frame's elapsed time, capped at 250 ms per frame, on every frame once the session exists (Frontend, the first-launch hold, Loading, Running), and never pauses: a future true pause freezes tick-counted `wait` and leaves `uiWait` running, as the flash limiter runs on UI time (`rendering_pipeline.md` §7.8). It is not UI time, the tween clock of `ui.md` §3, which pauses with game logic and is level-relative today, nor the loading tree's own clock (`research.md` §Clock).
- **Landing.** A tail lands at the UI drain of the first frame after its enrolling frame by which UI time has advanced by its duration. The UI drain runs, before the system-command drain, on every frame that runs UI logic; Loading frames run none, so a tail due during Loading lands on the first frame after it. Tails due on one frame land in enrollment order.
- **One scheduler, one instance table.** Each instance carries its own clock, tick-counted or UI time, set by the wait it is parked at. A body has at most one pending instance per E18 key whichever clock it waits on, so the parked-at wait decides a re-fire across mixed bodies (E18 O18): interruptible restarts from the top, uninterruptible ignores it. The instance and chain-depth caps cover the whole table (E18 O6, O7, O10, O28).
- **Lifetime follows the definition site.**
  - A presented-time instance of a mod-global reaction (`ModManifest`, whatever its `levels`) lives for the session: it survives level load, unload, return to frontend and suspend. Quit and frontend-to-level presses are mod-global, so this covers them.
  - One of a level-defined reaction (`setupLevel`) drops at level unload with one warning naming the count, so a cut beat reads as teardown, not a bug: its definition leaves with the level (`boot_sequence.md` §4; E18 O38, O39).
  - A committed staged reload drops every presented-time instance with one warning naming the count, level or not, because a tail is a copy of a body the author may have edited (E18 O40); a failed or stale reload keeps them. Tick-clocked instances keep E18's teardown.
- **After a `uiWait`, a body holds only machine-local steps, game-flow verbs and further `uiWait`s.** One player's reduce motion must never move when a shared, authoritative effect lands (E23 I9, `player_options.md` §5), and a sourceless reaction lets world sources reach a `uiWait`.
  - Machine-local is `reaction-body-composition`'s predicate: presentation, cell writes included; UI-stack verbs; text edits; `setState` on a non-replicated slot. Game-flow verbs are `loadLevel`, `restartLevel` and `returnToFrontend`; exit to desktop is a reserved action, never a step.
  - Install drops a reaction holding anything else after a `uiWait` — replicated `setState`, `setSentiment`, `adjustSentiment`, a member, group or subject-token step, a level `wait` — naming it and the step.
  - A `fire` after a `uiWait` is walked through its target chain at install, as V4b walks its targets, and drops the firing reaction if any body it reaches holds a disallowed step.
  - A `uiWait` after a level `wait` is legal: it enrolls on each machine that lands that level tail, and the steps after it obey this rule.
- **E18's install rows.** V1 covers `uiWait` durations: finite and positive, else install drops the reaction naming it, in both runtimes. V4a and V4b treat a `uiWait` as a wait; V2, V3 and V5 concern trigger-cancelled waits and ignore it.
- **`interruptible` ties the wait to its screen.** It anchors the wait to the topmost pushed tree once the steps before the wait have applied; when that tree instance leaves the stack, by any path, the rest of the body is cancelled. With no pushed tree the wait runs uninterruptible and warns once naming the reaction (E18 O55). Default `false`, as for `wait`; a trigger Exit never cancels a UI wait (`research.md` §Cancellation).
- **A wait changes when a step lands, never where.** A step after a `uiWait` lands on the machines it would land on with the wait removed, game-flow verbs included, since UI time runs on host and clients alike (`drafts/reaction-body-composition`). Where `E16--player-events` lands, its install rejection of machine-local effects it cannot forward covers `uiWait`.
- **Reduce motion collapses UI waits unless marked functional.** A `uiWait` is decorative by default: with reduce motion on it collapses, so the steps after it apply in the frame of the steps before it, a parked one included (`player_options.md` §5). `decorative: false` opts a pause out — a click's ring-out, a line's reading time — because shortening it changes behavior, not motion. Every `uiWait` before a game-flow verb in its body is functional whatever its flag, so one machine's reduce motion never moves a level change its peers follow. The App applies the switch at the UI drain; no simulation code reads it (`ui.md` §3).
- **A level `wait` needs a level.** Reached with no level installed, it parks nothing and warns once per reaction, naming it and `uiWait`; that closes the frontend defect above. The client warn-once in `reaction-body-composition` names `uiWait` too.
- **Placement.** The reaction scheduler becomes session-owned in sim scripting systems: the fixed tick advances tick-clocked instances, host-only, and the App advances UI time and drains its landings on every machine. The builder ships in both SDKs; the net wire is unchanged; it builds on `reaction-body-composition` (system steps, array bodies). The checks join install Pass A, which today runs only at level install and staged commit, so it also runs where the world-less frontend's active set composes.
- **Non-goals.** Looping choreography: a self-firing loop ends at the chain cap, as for `wait`. A cancel verb. Decorative level waits. Reserved `ui.*` actions stay instant. Frontend tween time: tweens run on a level-relative clock and stand still in the world-less frontend; `ui-tween-clock` owns it.

### Scripting surface
```ts
import { defineReaction } from "postretro";
import { openMenu, playSound, returnToFrontend, ui, uiWait } from "postretro/ui";

// Pause menu QUIT: the click rings out, then the level unloads. Uninterruptible:
// closing the menu mid-wait still quits, and a second press is ignored.
export const quitToMenu = defineReaction("pause.quitToMenu", [
  playSound("ui/confirm"),
  uiWait(300, { decorative: false }),  // the ring-out is functional
  returnToFrontend(),
]);

// A staged briefing. Each line holds long enough to read, so reduce motion
// never shortens it. Interruptible: the wait belongs to the briefing, now on top, so
// closing the briefing cancels the remaining lines.
export const briefingLines = ui.createLocalState({ line: 1 }); // spliced into the briefing tree
export const showBriefing = defineReaction("hub.showBriefing", [
  openMenu("hub.briefing"),
  uiWait(1500, { interruptible: true, decorative: false }),
  briefingLines.cells.line.set(2),
  uiWait(1500, { interruptible: true, decorative: false }),
  briefingLines.cells.line.set(3),
]);

// Level select cascades in. The steps stagger on UI time; the rows' own
// tweens animate in the frontend only once `ui-tween-clock` lands. The pauses
// only pace motion, so with reduce motion on every row appears in the press frame.
export const cascade = ui.createLocalState({ shown: 0 }); // spliced into the level-select tree
export const openLevelSelect = defineReaction("title.openLevelSelect", [
  openMenu("title.levelSelect"),
  cascade.cells.shown.set(1),
  uiWait(80),
  cascade.cells.shown.set(2),
  uiWait(80),
  cascade.cells.shown.set(3),
]);
```
Luau mirrors it: `UI.uiWait(1500, { interruptible = true, decorative = false })`; an omitted `interruptible` emits `false` and an omitted `decorative` emits `true`.

## Acceptance
### Automated
- [ ] No level loaded: a frontend press firing `[playSound(click), uiWait(300), openMenu(sub)]` plays the click that frame and opens `sub` on the first frame with at least 300 ms of UI time since, never earlier. With `wait(300)` instead, the click plays, one warning names the reaction and `uiWait`, `sub` never opens, and a level loaded afterwards does not open it either.
- [ ] Pause: the same tail lands after the same UI time whether its frames deliver zero, one or three sim ticks each, with the pause menu open or closed.
- [ ] Hitch: a single 2 s frame mid-wait advances UI time by 250 ms; a `uiWait(1000)` enrolled 400 ms earlier lands only after 350 ms more.
- [ ] Host pause menu over a level: the example's QUIT press plays the click and reaches the frontend at least 300 ms after the press. A second press while waiting plays the click again and returns once.
- [ ] Loopback client: the same press returns the client to its own frontend 300 ms after the press, in the same end state as the body without the wait reaches at once, logging nothing above debug; the host is unaffected. A `levelLoad` `[uiWait(500), playSound(x)]` plays `x` on each machine.
- [ ] Tree popped mid-wait: an interruptible wait after `openMenu(briefing)` cancels when the briefing leaves the stack by cancel, `closeDialog`, frontend return or level start; it lands when the briefing stays or a tree pushed above it pops. An uninterruptible wait lands in every case. A pop and an expiry on one frame cancel.
- [ ] Re-fire: an interruptible re-press restarts timing from the second press and lands once. With a menu open, name-fired `[a, wait(500), b, uiWait(100, { interruptible: true }), c]` ignores a re-fire parked at the level wait and restarts from the top on one parked at the `uiWait`; one instance exists either way.
- [ ] Level transition mid-wait: a mod-global reaction's tail enrolled in a level lands in the frontend after unload, one scoped by `levels` to that level included; a level-defined reaction's tail drops at unload with one warning naming the count and never lands. A frontend-enrolled tail lands across a load, on the first frame after Loading if due during it. No Loading frame lands a tail.
- [ ] Hot reload: a committed staged reload drops parked presented-time tails with one warning naming the count, with and without a level; after a failed one they land.
- [ ] Suspend: a parked presented-time tail survives suspend and resume and lands after its remaining time.
- [ ] Two on one frame: tails of two reactions due on one frame land in press order; two presses of one uninterruptible reaction in one frame leave one tail.
- [ ] Reduce motion on: steps after a plain `uiWait(80)` apply in the press frame in authored order, while `uiWait(80, { decorative: false })` still waits. Turning it on lands a parked decorative tail at the next UI drain and leaves a functional one. Off, both wait.
- [ ] Reduce motion on, on a host with a joined client: `[uiWait(80), cascadeStep, uiWait(80), loadLevel(x)]` waits both delays before the level change, exactly as with reduce motion off; the same body without `loadLevel` collapses both.
- [ ] Install, both runtimes, with and without a level loaded: after a `uiWait`, a replicated `updateState`, `setSentiment`, `adjustSentiment`, a member, group or subject-token step, or a level `wait` drops the reaction, naming it and the step; each installs before the `uiWait`. After it, a non-replicated `updateState`, a cell write, `showDialog`, `appendText`, `returnToFrontend` and a further `uiWait` install.
- [ ] Install: `[uiWait(100), fire(r)]` drops when `r`, or a reaction `r` fires, holds a disallowed step, naming the firing reaction; it installs when the chain holds only allowed steps, a self-firing chain included. `at: on.emitter` or a dispatch input after a `uiWait` drops the reaction; before it, the reaction installs.
- [ ] Loopback host plus client: `levelLoad` `[wait(500), playSound(a), uiWait(100), playSound(b)]` plays `b` on each machine 100 ms of UI time after its own `a`.
- [ ] An interruptible `uiWait` in a UI-fired reaction with no trigger binding installs. Reached with no pushed tree, it runs uninterruptible and warns once naming the reaction. A trigger Exit never cancels a UI tail.
- [ ] Trigger-bound `[updateState(alarm, 1), uiWait(300), playSound(x)]` writes `alarm` in the firing tick and plays `x` 300 ms of UI time later.
- [ ] Durations 0, -5, NaN and Infinity drop the reaction at install, naming it, in both runtimes.
- [ ] Caps: past the instance cap an enrollment on either clock warns and drops; a self-firing `uiWait` loop ends at the chain cap with one warning.
- [ ] The net wire version is unchanged. `uiWait` with and without options emits byte-identical manifest data from TS and Luau, both flags explicit.
- [ ] The Scripting surface example runs as a `content/dev` script wired into the dev pause menu, title and a hub tree; its TS and Luau twins emit byte-identical wire data, and each press behaves as the rows above.

### Manual
- [ ] Frontend and pause menu by hand: the click is heard before the return, the briefing reads at pace and stops when closed, the cascade's rows appear one by one with reduce motion off and at once with it on.
- [ ] Host plus client playtest: the client quits to its own frontend with the click; the host keeps playing.

### Promotion
- [ ] `uiWait` reads the same UI time as tweens and fades, the never-pausing session clock `ui-tween-clock` defines; whichever brief builds first introduces it, and that brief's promotion amends `ui.md` §3 and `rendering_pipeline.md` §7.8.

## Path
- **Seams.** `register_reaction_control_primitives` and `SequencedPrimitiveRegistry::register_control` for the `uiWait` handler. `dispatch_sequence` already routes a `SequenceTarget::Wait` step by primitive. The `@wait` sentinel checks in `data_descriptors/{js,lua}/reactions.rs` and `light_membership.rs` admit `uiWait`. Pass A (`validate_reaction_bodies_pass_a`) is called from `lifecycle_world_cpu.rs` and `staged_manifest_lifecycle.rs`; V2, V3 and V5 match `wait` by primitive. The step-class check and its `fire`-chain walk read only the registry and slot declarations, so they fit Pass A. App drain in `run_frontend_ui_logic` and at the Running system drain beside `drain_landings`; anchors through `ModalStack::active_instance` and `contains_instance`; the reload drop beside the existing `scheduler.clear()` in `staged_manifest_lifecycle.rs`, without its installed-level gate; the unload drop in `clear_surface_lifetime_level_state`, made selective; the no-level refusal in `ReactionScheduler::enroll`.
- **Shape.** One `ReactionScheduler` whose instance carries a clock. Rival: a second scheduler for UI time, simpler but unable to hold O18 across a mixed body.
- **First slice.** A world-less frontend press `[playSound(click), uiWait(300), openMenu(sub)]` opening `sub`. It falsifies the riskiest assumption, that a frame with no tick loop can advance and drain a reaction tail.
- **Split first.** `reaction_scheduler.rs`, `reaction_validation.rs`, `system_reactions.rs`, `session/mod.rs`, `modal_stack/mod.rs`, `staged_manifest_lifecycle.rs` and `main.rs` are past ~800 lines. Split only those extended, behavior-preserving, each in its own commit.

## Open questions
- How an instance stores its clock, and whether the tick and UI drains share one landing queue. **delegated**

## Boundary inventory
Both runtimes ship every row.
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| UI wait step | `uiWait` control primitive on the sequence registry; presented-time instance in the one reaction scheduler | sequence entry `{ id: "@wait", primitive: "uiWait", args: { durationMs, interruptible, decorative } }` | `uiWait(durationMs: number, opts?: { interruptible?: boolean; decorative?: boolean }): SequenceStep[]` from `postretro/ui` | `UI.uiWait(durationMs, opts?)` | n/a |
| `interruptible` | bool, default false | `"interruptible"`, always emitted | `interruptible` | `interruptible` | n/a |
| `decorative` | bool, default true | `"decorative"`, always emitted | `decorative` | `decorative` | n/a |

## Wire format
Manifest JSON only; the net wire does not change.
- A `@wait` sequence entry may name primitive `wait` or `uiWait`; the sentinel marks a wait, so every consumer that stops at a wait stops at both, and the primitive names the clock. `uiWait` args carry `durationMs`, `interruptible` and `decorative`; `wait` gains no new arg. Any other primitive under `@wait` is still rejected at parse.
