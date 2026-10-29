# ui-timed-reactions

Brief · compact · reads: `context/lib/ui.md` §3, §4 · `context/lib/scripting.md` §2, §10.4, §12 · `context/lib/boot_sequence.md` §1, §4 · `context/lib/rendering_pipeline.md` §7.8 · builds on `ready/reaction-body-composition` · read at b6b85754c

## Problem
A requested capability, raised by the owner. Menus need timed beats that do something: a click that rings out before the pause menu quits, a briefing staged line by line with a sound per line, a menu that opens after a delay. The only `wait` counts fixed sim ticks for the life of a level. In the world-less frontend no tick runs, so a frontend press's tail parks, then lands inside whichever level loads next. When done, a UI wait is its own step counted in UI time on each machine. It lands with or without a level, outlives a level when its reaction does, and cancels with its screen when asked to.

## Decisions
- **A UI wait has its own spelling: `uiWait(durationMs, opts?)`, from `postretro/ui`.** The step names its clock, so a reaction means the same thing from every source (`scripting.md` §12, a reaction is sourceless). Rivals: `research.md` §Spelling.
- **Visual timing belongs to the UI tree; functional pacing belongs to reactions.** A tween's entry delay and a list's stagger are tree vocabulary that ends with the tree instance (`drafts/ui-tween-clock`, `ui.md` §3). A `uiWait` paces behavior — a sound, a menu, a state write — so reduce motion never shortens it (`research.md` §Placement).
- **The clock is UI time,** the never-pausing session clock `ui-tween-clock` defines. It sums each presented frame's elapsed time, capped at 250 ms per frame, on every frame once the session exists (Frontend, the first-launch hold, Loading, Running), whether or not a sim tick runs. It is not `script_time`, the level clock that resets per level (`research.md` §Clock).
- **Landing.** A tail lands at the UI drain of the first frame after its enrolling frame by which UI time has advanced by its duration. The UI drain runs, before the system-command drain, on every frame that runs UI logic; Loading frames run none, so a tail due during Loading lands on the first frame after it. Tails due on one frame land in enrollment order.
- **UI time is one more per-instance clock in the one `ReactionScheduler`** that `reaction-body-composition` decides. Every instance keeps that brief's source class, tick clock and key, and counts on the clock of the wait it is parked at. A key holds one pending instance whichever wait it is parked at, so that wait decides a re-fire across mixed bodies (E18 O18): interruptible restarts from the top, uninterruptible ignores it. The instance and chain-depth caps cover the whole table (E18 O6, O7, O10, O28).
- **Lifetime follows the definition site.**
  - A `uiWait` instance of a mod-global reaction (`ModManifest`, whatever its `levels`) lives for the session: it survives level load, unload, return to frontend and suspend. Quit and frontend-to-level presses are mod-global, so this covers them.
  - One of a level-defined reaction (`setupLevel`) drops at level unload and at suspend, with one warning naming the count, at debug on a client (`boot_sequence.md` §4; E18 O38, O39).
  - A committed staged reload drops every `uiWait` instance with one warning naming the count, level or not (E18 O40); a failed or stale reload keeps them. Tick-clocked instances keep their teardown (`reaction-body-composition`).
- **A `uiWait` changes when a step lands, never where.** Steps after it land under `reaction-body-composition`'s rules for steps after a `wait` (`scripting.md` §12): a client lands only machine-local ones, and an own-state drop warns once. A client keeps a `uiWait`'s `interruptible`, which anchors to its own stack (below). `E16--player-events`' install rejection of machine-local effects it cannot forward covers steps after a `uiWait`.
- **A mod-global tail never outlives its target.** Install drops a mod-global reaction that holds, after a `uiWait`, a step whose target level unload tears down — a group step, a level `wait` or `setGhostAudio` — naming it and the step. Slot writes, sentiment, game-flow verbs and machine-local steps outlive a level and install (`research.md` §Step class). Subject tokens, `on.emitter` and dispatch inputs stay refused after any wait (`reaction-body-composition`), and map members are unreachable from mod-global scripts (`scripting.md` §12).
  - A post-`uiWait` `fire` is walked through the mod-global bodies its chain reaches, as V4b walks targets, and drops the firing reaction if any holds such a step. A level-defined target resolves at landing against the active set, like any `fire`.
  - A level-defined tail drops with its level, so a level-defined reaction may hold those steps after a `uiWait`.
  - A `uiWait` after a level `wait` enrolls on a machine's UI clock when that machine lands the level tail. A late joiner that converges a due tick segment counts every later segment of that tail as due too: state steps converge in authored order and transient presentation is skipped, per `reaction-body-composition`'s late-joiner rule.
- **E18's install rows.** V1 covers `uiWait` durations: finite and positive, else install drops the reaction naming it, in both runtimes. V4a and V4b treat a `uiWait` as a wait; V2, V3 and V5 concern trigger-cancelled waits and ignore it.
- **`interruptible` ties the wait to its screen.** It anchors the wait to the topmost pushed tree on that machine's own stack, a client's included, once the steps before the wait have applied; when that tree instance leaves the stack, by any path, the rest of the body is cancelled. With no pushed tree the wait runs uninterruptible and warns once naming the reaction (E18 O55). Default `false`, as for `wait`; a trigger Exit never cancels a UI wait (`research.md` §Cancellation).
- **A level `wait` needs a level.** Reached with no level installed, it parks nothing and warns once per reaction, naming it and `uiWait`.
- **Placement.** The scheduler is session-owned, in sim scripting systems. The App advances UI time and drains `uiWait` landings on every machine. `uiWait` returns one step, joining `reaction-body-composition`'s step union, and ships in both SDKs; the net wire is unchanged. The checks join install Pass A, which runs at level install and staged commit, and also where the world-less frontend's active set composes.
- **Non-goals.** Looping choreography: a self-firing loop ends at the chain cap, as for `wait`. A cancel verb. Reserved `ui.*` actions stay instant.

### Scripting surface
```ts
import { defineReaction } from "postretro";
import { openMenu, playSound, returnToFrontend, ui, uiWait } from "postretro/ui";

// Pause menu QUIT: the click rings out, then the level unloads. Uninterruptible:
// closing the menu mid-wait still quits, and a second press is ignored.
export const quitToMenu = defineReaction("pause.quitToMenu", [
  playSound("sfx/test_tone"),
  uiWait(300),
  returnToFrontend(),
]);

// A staged briefing: each line holds long enough to read and arrives with its own
// sound. Interruptible: the wait belongs to the briefing, which is on top, so
// closing the briefing cancels the remaining lines.
export const briefingLines = ui.createLocalState({ line: 1 }); // spliced into the briefing tree
export const showBriefing = defineReaction("hub.showBriefing", [
  openMenu("hub.briefing"),
  uiWait(1500, { interruptible: true }),
  briefingLines.cells.line.set(2),
  playSound("fixtures/pellet_hit"),
  uiWait(1500, { interruptible: true }),
  briefingLines.cells.line.set(3),
  playSound("fixtures/pellet_hit"),
]);

// Title PLAY: the whoosh plays, then the level select opens. Its rows cascade in
// through the tree's own stagger (`ui-tween-clock`).
export const openPlay = defineReaction("frontend.openPlay", [
  playSound("fixtures/jump"),
  uiWait(250),
  openMenu("frontend.devLevelSelect"),
]);
```
Luau mirrors it: `UI.uiWait(1500, { interruptible = true })`; an omitted `interruptible` emits `false`.

## Acceptance
### Automated
- [ ] No level loaded: a frontend press firing `[playSound(click), uiWait(300), openMenu(sub)]` plays the click that frame and opens `sub` on the first frame with at least 300 ms of UI time since, never earlier. With `wait(300)` instead, the click plays, one warning names the reaction and `uiWait`, `sub` never opens, and a level loaded afterwards does not open it either.
- [ ] Pause: the same tail lands after the same UI time whether its frames deliver zero, one or three sim ticks each, with the pause menu open or closed.
- [ ] Hitch: a single 2 s frame mid-wait advances UI time by 250 ms; a `uiWait(1000)` enrolled 400 ms earlier lands only after 350 ms more.
- [ ] Reduce motion on: `[playSound(a), uiWait(300), playSound(b)]` plays `b` 300 ms of UI time after `a`, as with it off; turning it on mid-wait moves nothing.
- [ ] Host pause menu over a level: the example's QUIT press plays the click and reaches the frontend at least 300 ms after the press. A second press while waiting plays the click again and returns once.
- [ ] Loopback client: the same press returns the client to its own frontend 300 ms after the press, in the same end state as the body without the wait reaches at once, logging nothing above debug; the host is unaffected. A `levelLoad` `[uiWait(500), playSound(x)]` plays `x` on each machine.
- [ ] Tree popped mid-wait: an interruptible wait after `openMenu(briefing)` cancels when the briefing leaves the stack by cancel, `closeDialog`, frontend return or level start; it lands when the briefing stays or a tree pushed above it pops. An uninterruptible wait lands in every case. A pop and an expiry on one frame cancel.
- [ ] Re-fire: an interruptible re-press restarts timing from the second press and lands once. With a menu open, name-fired `[a, wait(500), b, uiWait(100, { interruptible: true }), c]` ignores a re-fire parked at the level wait and restarts from the top on one parked at the `uiWait`; one instance exists either way.
- [ ] Level transition mid-wait: a mod-global reaction's tail enrolled in a level lands in the frontend after unload, one scoped by `levels` to that level included; a level-defined reaction's tail drops at unload with one warning naming the count and never lands. A frontend-enrolled tail lands across a load, on the first frame after Loading if due during it. No Loading frame lands a tail.
- [ ] Hot reload: a committed staged reload drops parked `uiWait` tails with one warning naming the count, with and without a level; after a failed one they land.
- [ ] Suspend: a mod-global reaction's parked `uiWait` tail survives suspend and resume and lands after its remaining time; a level-defined one drops with one warning naming the count.
- [ ] Two on one frame: tails of two reactions due on one frame land in press order; two presses of one uninterruptible reaction in one frame leave one tail.
- [ ] Install, both runtimes, host and client, with and without a level loaded: in a mod-global reaction, a group step, a level `wait` or `setGhostAudio` after a `uiWait` drops the reaction, naming it and the step; each installs before the `uiWait`. After it, replicated and non-replicated `updateState`, `setSentiment`, `adjustSentiment`, a cell write, `showDialog`, `appendText`, `loadLevel`, `returnToFrontend` and a further `uiWait` install.
- [ ] Install: mod-global `[uiWait(100), fire(r)]` drops when a mod-global `r`, or a mod-global reaction `r` fires, holds a group step, naming the firing reaction; it installs when the chain holds none, a self-firing chain included, and when `r` is level-defined. A subject token, `at: on.emitter` or a dispatch input after a `uiWait` drops the reaction; before it, the reaction installs.
- [ ] A level-defined reaction `[uiWait(100), m.start(), wait(200), npcs().update({ aggro: true })]` installs, and on the host starts `m` 100 ms of UI time after the fire and updates the NPCs 200 ms of ticks later.
- [ ] Mod-global UI press `[uiWait(300), updateState(shared, 1)]`, `shared` replicated, on a host with a joined client: the host's press writes `shared` 300 ms later and the client sees it by replication; quitting to the frontend mid-wait, the write lands there and the slot keeps it into the next level. The client's own press drops the write and warns once, naming the reaction and the step.
- [ ] Loopback host plus client: `levelLoad` `[wait(500), playSound(a), uiWait(100), playSound(b)]` plays `a` on each machine at host `script_time` 500 ms ± one tick, and `b` 100 ms of that machine's UI time after its own `a`.
- [ ] Late joiner: `levelLoad` `[wait(500), playSound(a), uiWait(100), showDialog(d), playSound(b), wait(200), playSound(c)]` in a level-defined reaction, a client whose first host-clock reading is at 10 s, and one at 550 ms: each shows `d` at that reading, skips `a`, `b` and `c` at debug, and lands nothing later.
- [ ] Loopback client: a crossing on `player.health` below 25 firing `[wait(300), playSound(a), uiWait(100), showDialog(d)]` plays `a` 300 ms after the client's own crossing and shows `d` 100 ms of its UI time later; the host does the same for its own player's crossing. An interruptible `uiWait` there cancels when that machine's anchor tree leaves its stack.
- [ ] An interruptible `uiWait` in a UI-fired reaction with no trigger binding installs. Reached with no pushed tree, it runs uninterruptible and warns once naming the reaction. A trigger Exit never cancels a UI tail.
- [ ] Trigger-bound `[updateState(alarm, 1), uiWait(300), playSound(x)]` writes `alarm` in the firing tick and plays `x` 300 ms of UI time later.
- [ ] Durations 0, -5, NaN and Infinity drop the reaction at install, naming it, in both runtimes.
- [ ] Caps: past the instance cap an enrollment on either clock warns and drops; a self-firing `uiWait` loop ends at the chain cap with one warning.
- [ ] The net wire version is unchanged. `uiWait` with and without options emits byte-identical manifest data from TS and Luau, `interruptible` explicit.
- [ ] The Scripting surface example runs as a `content/dev` script wired into the dev pause menu, title and a hub tree; its TS and Luau twins emit byte-identical wire data, and each press behaves as the rows above.

### Manual
- [ ] Frontend and pause menu by hand: the click is heard before the return, the briefing reads at pace with a sound per line and stops when closed, and the level select opens after the whoosh.
- [ ] Host plus client playtest: the client quits to its own frontend with the click; the host keeps playing.

### Promotion
- [ ] `uiWait` reads the same UI time as tweens and fades, the never-pausing session clock `ui-tween-clock` defines; whichever brief builds first introduces it, and that brief's promotion amends `ui.md` §3 and `rendering_pipeline.md` §7.8.

## Path
- **Seams.** `register_reaction_control_primitives` and `SequencedPrimitiveRegistry::register_control` for the `uiWait` handler. `dispatch_sequence` already routes a `SequenceTarget::Wait` step by primitive. The `@wait` sentinel checks in `data_descriptors/{js,lua}/reactions.rs` and `light_membership.rs` admit `uiWait`. Pass A (`validate_reaction_bodies_pass_a`) is called from `lifecycle_world_cpu.rs` and `staged_manifest_lifecycle.rs`; V2, V3 and V5 match `wait` by primitive. The fence and its `fire`-chain walk read only the registry and each reaction's definition site, so they fit Pass A. App drain in `run_frontend_ui_logic` (`main.rs`) and at the Running system drain beside `drain_landings` (`frame_loop/mod.rs`); anchors through `ModalStack::active_instance` and `contains_instance_id`; the reload drop beside the existing `scheduler.clear()` in `staged_manifest_lifecycle.rs`, without its installed-level gate; the unload and suspend drops in `clear_surface_lifetime_level_state`, made selective; the no-level refusal in `ReactionScheduler::enroll`.
- **Shape.** One `ReactionScheduler` whose instance carries a clock. Rival: a second scheduler for UI time, simpler but unable to hold O18 across a mixed body.
- **First slice.** A world-less frontend press `[playSound(click), uiWait(300), openMenu(sub)]` opening `sub`. It falsifies the riskiest assumption, that a frame with no tick loop can advance and drain a reaction tail.
- **Split first.** `reaction_scheduler.rs`, `reaction_validation.rs`, `system_reactions.rs`, `light_membership.rs`, `session/mod.rs`, `modal_stack/mod.rs`, `staged_manifest_lifecycle.rs`, `frame_loop/mod.rs`, `main.rs` and `content/dev/scripts/frontend-menu.ts` are past ~800 lines. Split only those extended, behavior-preserving, each in its own commit. Coordinate with `ui-tween-clock`, which splits some of the same files.

## Open questions
- Whether the tick and UI drains share one landing queue. **delegated**

## Boundary inventory
Both runtimes ship every row.
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| UI wait step | `uiWait` control primitive on the sequence registry; a UI-time clock on an instance in the one `ReactionScheduler` | sequence entry `{ id: "@wait", primitive: "uiWait", args: { durationMs, interruptible } }` | `uiWait(durationMs: number, opts?: { interruptible?: boolean }): UiWaitStep` from `postretro/ui` | `UI.uiWait(durationMs, opts?)`, one step table | n/a |
| Step type | n/a | n/a | `UiWaitStep` joins `SequenceStep`, legal unspread in an array body or `sequence` | Luau step type mirror | n/a |
| `interruptible` | bool, default false | `"interruptible"`, always emitted | `interruptible` | `interruptible` | n/a |

## Wire format
Manifest JSON only; the net wire does not change.
- A `@wait` sequence entry may name primitive `wait` or `uiWait`; the sentinel marks a wait, so every consumer that stops at a wait stops at both, and the primitive names the clock. `uiWait` args carry `durationMs` and `interruptible`; `wait` gains no new arg. Any other primitive under `@wait` is still rejected at parse.
