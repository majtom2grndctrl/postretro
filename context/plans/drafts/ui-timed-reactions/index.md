# ui-timed-reactions

Brief · compact · reads: `context/lib/ui.md` §3, §4 · `context/lib/scripting.md` §12 · `context/lib/boot_sequence.md` §1 · `context/lib/player_options.md` §5 · read at a76d99e15

## Problem
A requested capability, raised by the owner. Menus need timed beats: a click that rings out before the pause menu quits, a briefing staged line by line, a cascade of menu items. The only `wait` counts fixed sim ticks on a level-lifetime, host-only scheduler. In the world-less frontend no tick runs, so a frontend press's tail parks, then lands inside whichever level loads next; on a connected client it is refused. When done, a UI wait is its own step on a machine-local, session-lifetime UI clock. It lands with or without a level, on every machine, keeps pacing while paused, cancels with its screen when asked to, and collapses under reduce motion when it only paces motion.

## Decisions
- **A UI wait has its own spelling: `uiWait(durationMs, opts?)`, from `postretro/ui`.** The step names its clock, so a reaction means the same thing from every source (`scripting.md` §12, a reaction is sourceless). Choosing the clock by firing source would give one `wait()` different clocks, lifetimes and cancel rules per caller; rivals in `research.md` §Spelling.
- **The UI clock is session presented time.** It sums each presented frame's elapsed time, capped at 250 ms per frame, on every frame once the session exists: Frontend, the first-launch hold, Loading and Running. It never pauses, so a future true pause freezes tick-counted `wait` and leaves `uiWait` running, as the flash limiter runs on presented time (`rendering_pipeline.md` §7.8). It is neither the tween clock of `ui.md` §3, which is level-relative and stands still in the world-less frontend, nor the loading tree's own clock (`research.md` §Clock).
- **Landing.** A tail lands at the UI drain of the first frame after its enrolling frame by which the clock has advanced by its duration. The UI drain runs, before the system-command drain, on every frame that runs UI logic; Loading frames run none, so a tail due during Loading lands on the first frame after it. Tails due on one frame land in enrollment order.
- **Lifetime is the session.** A tail survives level load, unload, return to frontend and platform suspend. A committed staged reload drops every parked UI tail with one warning naming the count, level or not, because a tail is a copy of a body the author may have edited (`plans/done/E18--timed-reaction-steps` O40); a failed or stale reload keeps them.
- **`interruptible` ties the wait to its screen.** It anchors the wait to the topmost pushed tree once the steps before the wait have applied; when that tree instance leaves the stack, by any path, the rest of the body is cancelled. Anchoring to what is showing rather than to who fired keeps the rule sourceless; with no pushed tree the wait runs uninterruptible and warns once naming the reaction (E18 O55). Default `false`, as for `wait`; a trigger Exit never cancels a UI wait (`research.md` §Cancellation).
- **Re-fire and caps follow E18.** Each clock holds at most one pending tail per reaction body. A re-fire parked at an interruptible UI wait restarts from the top; parked at an uninterruptible one it is ignored, so a second Quit press cannot quit twice. The instance and chain-depth caps apply per session (E18 O6, O7, O10, O28).
- **A UI tail runs session-scoped steps only.** Between a `uiWait` and the next wait of either kind, a body may hold system steps, `fire`, `wait` and `uiWait`. Install drops a reaction with a member, group or subject-token step there, naming it and the step and pointing to `wait`, because those steps address a level and the UI clock outlives it. E18 V4a and V4b treat a `uiWait` as a wait; V2, V3 and V5 concern trigger-cancelled waits and ignore it.
- **A wait changes when a step lands, never where.** The UI clock runs on host and clients alike. A step after a `uiWait` lands on the machines it would land on with the wait removed, game-flow verbs included (`drafts/reaction-body-composition`). Where `E16--player-events` lands, its install rejection of machine-local effects it cannot forward covers `uiWait`.
- **Reduce motion collapses UI waits unless marked functional.** A `uiWait` is decorative by default: with reduce motion on it collapses, so the steps after it apply in the frame of the steps before it, a parked one included (`player_options.md` §5). `decorative: false` opts a pause out — a click's ring-out, a line's reading time — because shortening it changes behavior, not motion. Menu choreography is mostly motion, so the common case needs no option. The App applies the switch at the UI drain; no simulation code reads it (`ui.md` §3).
- **A level `wait` needs a level.** Reached with no level installed, it parks nothing and warns once per reaction, naming it and `uiWait`; that closes the frontend defect above. The client warn-once in `reaction-body-composition` names `uiWait` too.
- **Durations** must be finite and positive; install drops the reaction otherwise, naming it, in both runtimes (E18 V1).
- **Placement.** A session-owned UI scheduler sits beside the level scheduler in sim scripting systems; the App advances and drains it; the builder ships in both SDKs; checks join install Pass A. Builds on `reaction-body-composition` (system steps, array bodies). Net wire unchanged.
- **Non-goals.** Looping choreography: a self-firing loop ends at the chain cap, as for `wait`. A cancel verb. Decorative level waits. Moving tweens onto the UI clock (`research.md` §Doors). Reserved `ui.*` actions stay instant.

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

// Level select cascades in. The pauses only pace motion, so with reduce motion
// on every row appears in the press frame.
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
- [ ] No level loaded: a frontend press firing `[playSound(click), uiWait(300), openMenu(sub)]` plays the click that frame and opens `sub` on the first frame with at least 300 ms of UI clock since, never earlier. With `wait(300)` instead, the click plays, one warning names the reaction and `uiWait`, `sub` never opens, and a level loaded afterwards does not open it either.
- [ ] Pause: the same tail lands after the same UI clock time whether its frames deliver zero, one or three sim ticks each, with the pause menu open or closed.
- [ ] Hitch: a single 2 s frame mid-wait advances the UI clock by 250 ms; a `uiWait(1000)` enrolled 400 ms earlier lands only after 350 ms more.
- [ ] Host pause menu over a level: the example's QUIT press plays the click and reaches the frontend at least 300 ms after the press. A second press while waiting plays the click again and returns once.
- [ ] Loopback client: the same press returns the client to its own frontend 300 ms after the press, in the same end state as the body without the wait reaches at once, logging nothing above debug; the host is unaffected. A `levelLoad` `[uiWait(500), playSound(x)]` plays `x` on each machine.
- [ ] Tree popped mid-wait: an interruptible wait after `openMenu(briefing)` cancels when the briefing leaves the stack by cancel, `closeDialog`, frontend return or level start; it lands when the briefing stays or a tree pushed above it pops. An uninterruptible wait lands in every case. A pop and an expiry on one frame cancel.
- [ ] Re-press: an interruptible re-press restarts timing from the second press and lands once.
- [ ] Level transition mid-wait: a tail enrolled in a level lands in the frontend after unload; one enrolled in the frontend lands across a load, on the first frame after Loading if due during it. No Loading frame lands a tail.
- [ ] Hot reload: a committed staged reload drops parked UI tails with one warning naming the count, with and without a level; after a failed one they land.
- [ ] Suspend: a parked tail survives suspend and resume and lands after its remaining time.
- [ ] Two on one frame: tails of two reactions due on one frame land in press order; two presses of one uninterruptible reaction in one frame leave one tail.
- [ ] Reduce motion on: steps after a plain `uiWait(80)` apply in the press frame in authored order, while `uiWait(80, { decorative: false })` still waits. Turning it on lands a parked decorative tail at the next UI drain and leaves a functional one. Off, both wait.
- [ ] Install, both runtimes: a member, group or subject-token step after a `uiWait` drops the reaction, naming it and the step; after a later `wait` in the same body it installs and lands on ticks. `at: on.emitter` or a dispatch input after a `uiWait` drops the reaction; before it, the reaction installs.
- [ ] An interruptible `uiWait` in a UI-fired reaction with no trigger binding installs. Reached with no pushed tree, it runs uninterruptible and warns once naming the reaction. A trigger Exit never cancels a UI tail.
- [ ] Trigger-bound `[updateState(alarm, 1), uiWait(300), playSound(x)]` writes `alarm` in the firing tick and plays `x` 300 ms of UI clock later.
- [ ] Durations 0, -5, NaN and Infinity drop the reaction at install, naming it, in both runtimes.
- [ ] Caps: past the session cap a UI enrollment warns and drops; a self-firing `uiWait` loop ends at the chain cap with one warning.
- [ ] The net wire version is unchanged. `uiWait` with and without options emits byte-identical manifest data from TS and Luau, both flags explicit.
- [ ] The Scripting surface example runs as a `content/dev` script wired into the dev pause menu, title and a hub tree; its TS and Luau twins emit byte-identical wire data, and each press behaves as the rows above.

### Manual
- [ ] Frontend and pause menu by hand: the click is heard before the return, the briefing reads at pace and stops when closed, the cascade staggers with reduce motion off and appears at once with it on.
- [ ] Host plus client playtest: the client quits to its own frontend with the click; the host keeps playing.

## Path
- **Seams.** `register_reaction_control_primitives` and `SequencedPrimitiveRegistry::register_control` for the `uiWait` handler. `dispatch_sequence` already routes a `SequenceTarget::Wait` step by primitive. The `@wait` sentinel checks in `data_descriptors/{js,lua}/reactions.rs` and `light_membership.rs` admit `uiWait`. Pass A in `reaction_validation.rs`; V2, V3 and V5 match `wait` by primitive. App drain in `run_frontend_ui_logic` and at the Running system drain beside `drain_landings`; anchors through `ModalStack::active_instance` and `contains_instance`; the reload drop beside the existing `scheduler.clear()` in `staged_manifest_lifecycle.rs`, without its installed-level gate; the no-level refusal in `ReactionScheduler::enroll`.
- **Shape.** Extract the instance, re-fire and landing core of `ReactionScheduler` into a clock-generic core run twice: ticks, host-only, level lifetime; and presented seconds, every machine, session lifetime, modal-instance anchor. Rival: a standalone UI scheduler, simpler but free to drift from E18's re-fire rules.
- **First slice.** A world-less frontend press `[playSound(click), uiWait(300), openMenu(sub)]` opening `sub`. It falsifies the riskiest assumption, that a frame with no tick loop can advance and drain a reaction tail.
- **Split first.** `reaction_scheduler.rs`, `reaction_validation.rs`, `system_reactions.rs`, `session/mod.rs`, `modal_stack/mod.rs`, `staged_manifest_lifecycle.rs` and `main.rs` are past ~800 lines. Split only those extended, behavior-preserving, each in its own commit.

## Open questions
- Clock-generic core or standalone UI scheduler. **delegated**

## Boundary inventory
Both runtimes ship every row.
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| UI wait step | `uiWait` control primitive on the sequence registry; session UI scheduler | sequence entry `{ id: "@wait", primitive: "uiWait", args: { durationMs, interruptible, decorative } }` | `uiWait(durationMs: number, opts?: { interruptible?: boolean; decorative?: boolean }): SequenceStep[]` from `postretro/ui` | `UI.uiWait(durationMs, opts?)` | n/a |
| `interruptible` | bool, default false | `"interruptible"`, always emitted | `interruptible` | `interruptible` | n/a |
| `decorative` | bool, default true | `"decorative"`, always emitted | `decorative` | `decorative` | n/a |

## Wire format
Manifest JSON only; the net wire does not change.
- A `@wait` sequence entry may name primitive `wait` or `uiWait`; the sentinel marks a wait, so every consumer that stops at a wait stops at both, and the primitive names the clock. `uiWait` args carry `durationMs`, `interruptible` and `decorative`; `wait` gains no new arg. Any other primitive under `@wait` is still rejected at parse.
