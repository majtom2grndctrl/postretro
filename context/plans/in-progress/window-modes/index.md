# window-modes

Brief · resumable · reads: `context/lib/boot_sequence.md` §1, `context/lib/player_options.md` §2–§5, `context/lib/ui.md` §1.1, §3, §4.1 · read at 67fddd0af

## Problem
Requested capability, raised by the developer. A player can only play in a window: there is no fullscreen of any kind. When done, a player chooses windowed, borderless fullscreen, or exclusive fullscreen at a display mode they pick. The saved choice is in effect from boot, with no mid-splash switch from windowed, and follows changes the OS makes.

## Decisions
- **Three modes.** Windowed, borderless, exclusive, persisted as top-level `window_mode` (machine-scoped); an unknown value falls back to windowed for that field alone (`player_options.md` §2). First launch is windowed.
- **Exclusive earns its place by changing the display mode.** Rival: borderless plus `render_resolution`, which already gives the low-resolution look. Rejected because only a real mode change gives monitor-side scaling and a refresh-rate choice. winit's Windows exclusive is a display-settings change plus a topmost window, not DXGI exclusive ownership, so it is not claimed as a latency or presentation win.
- **Exclusive display mode is re-found, never remembered.** It persists as size, refresh, bit depth and monitor name, and every apply matches it against a fresh enumeration of the window's current monitor, so an un-enumerated mode never reaches winit. No match → borderless for the session with a warning; the stored mode stays, so the monitor's return restores it at the next apply. winit panics when the OS refuses a mode change, listed or not; that crash is accepted, because the live path's confirm never persisted the mode and `--windowed` recovers the boot path.
- **No exclusive on Wayland.** Wayland lists modes but ignores an exclusive request, so there the enumeration reads empty and exclusive takes the fallback.
- **Picker default and Apply (owner amendment, 2026-10-04).** With no saved display tuple, seed the picker from an enumerated mode matching the monitor's resolution at launch, preferring its current refresh rate. This does not overwrite an unavailable saved tuple or save before first presentation. Next/previous only browse a session-local selection; Apply Resolution commits it while windowed or requests it with the existing confirmation while exclusive. Selecting Exclusive itself uses the picked/default tuple and confirms. Borderless does not change display resolution, so its picker and Apply button are disabled and drawn at 80% opacity.
- **Revert countdown.** A live change into exclusive, or between exclusive modes, shows an engine-owned confirm. Unconfirmed after 15 s, or cancelled, the prior mode returns and nothing persists. A misreported mode would otherwise strand the player on a black screen. While a confirm is pending, every other mode change is refused and writes nothing, so "the prior mode" stays defined. Changes to windowed or borderless apply and persist without a prompt, as does the saved mode at boot. The confirm is a `core/ui/` descriptor under a reserved, non-shadowable registry name, so no mod tree can replace the dialog whose focus guards a blind confirm. Diverges from `ui.md` §1.1's single exempt name; amended at promotion. It follows U3's confirmation-dialog convention, focus opening on revert (`ready/E23--accessibility` AC 19); built before U3, it keeps that rule and U3 aligns its shape.
- **One settings read, before the window.** `PlayerOptions` loads once before the window exists and passes to `Session::build` through the pending-session owner; `player_id` generation and the first-launch save stay after the first pixel. Diverges from `boot_sequence.md` §1's "no session-lifetime work pre-window", and partly reverses `done/boot-session-boundary` Task 3, which moved `player_options` out of the pre-window services. The read is cheap and side-effect free, `Session` stays the sole owner, and the alternative switches modes mid-splash on every fullscreen launch. The doc is amended at promotion.
- **Mode applies after visibility, before the first redraw request.** Never as a creation attribute on a hidden window: winit leaves hidden + fullscreen unguarded on macOS, and U4 (`ready/E23--accessibility`) creates the window hidden. The rule holds with or without U4, preserving `boot_sequence.md` §Window visibility.
- **One window-mode chokepoint.** App-side, not in the render profile: a mode change is a winit call, not a renderer setter. Boot apply, live apply and readback all route through it; UI and the options store never call winit.
- **Readback follows the OS.** The actual mode is read back from the window once per frame and compared with a baseline the chokepoint keeps, not with the store. winit exposes no transition state, and macOS reports a request before its transition runs, so every request opens a bounded settle window. Readings inside it write nothing; when it closes, the reading becomes the baseline unwritten, so a failed or dropped entry (winit drops a failed entry made after visibility) is treated like the fallback and the stored mode stays. Outside a settle window and the fallback, a reading that differs from the baseline is OS-driven (macOS green button): it updates the store, reseeds the working copy, and persists through the settled save.
- **`--windowed` boot escape.** A startup flag forces windowed for that session and writes nothing to the store; a menu change later in the session persists as usual. A saved exclusive mode that still matches but no longer displays (driver, cable, KVM) otherwise blacks out every launch, and Alt+Enter is out of scope.
- **Placement.** Window mode and display mode are engine mechanism. A mod's menu chooses whether and where to offer them; the engine's confirm and fallback hold regardless.
- **Non-goals.**
  - Fullscreen hotkey (Alt+Enter): a toggle is a command, and U3 owns the command table.
  - Monitor choice: borderless and exclusive use the window's current monitor, where the player put the window; choosing another is a separate option surface.
  - Reacting when the exclusive monitor disconnects mid-session: winit's reading is cached and cannot see it; the next apply or launch re-finds.
  - An author-set first-launch mode: it would need a launch-time input before the window, and no game asks for one.
  - Persisted windowed size and position: leaving fullscreen restores the prior size within a session, and the default size covers launch.
  - Settings scope and the `[game."<mod_id>"]` layer: `drafts/E23--gamepad-input` owns them. Per-game user directories: `ready/game-user-dirs`.

### Scripting surface
```ts
import { defineReaction } from "postretro";
import { Button, HStack, Text, bindState, displayModeAction, getGameState, stateEquals, updateState } from "postretro/ui";

const { options, window } = getGameState();

// Window mode: a writable working copy, written like renderResolution.
defineReaction("frontend.options.windowMode.exclusive", updateState(options.windowMode, "exclusive"));
const isExclusive = stateEquals(options.windowMode, "exclusive"); // "windowed" | "borderless" | "exclusive"

// Display mode: engine-stepped over the enumerated modes. Script never sees the list;
// it reads the picked mode's fields and shows the ones it wants.
HStack({}, [
  Button({ id: "displayModePrev", label: "<", onPress: displayModeAction("previous") }),
  Text({ content: "", bind: bindState(window.displayModeWidth, { format: "{}x" }) }),  // readonly number, 0 when none
  Text({ content: "", bind: bindState(window.displayModeHeight) }),
  Text({ content: "", bind: bindState(window.displayModeRefreshHz, { format: " @ {} Hz", decimalPlaces: 0 }) }),
  Button({ id: "displayModeNext", label: ">", onPress: displayModeAction("next") }),
  Button({ id: "displayModeApply", label: "APPLY RESOLUTION", onPress: displayModeAction("apply") }),
]);
// Also readonly: window.displayModeBitDepth (number), window.displayModeMonitor (string, "" when none).

// The engine confirm fires these; a mod may too.
displayModeAction("keep");   // "ui.displayMode.keep"
displayModeAction("revert"); // "ui.displayMode.revert"
// window.displayModeRevertSeconds: readonly number, 0 while no confirm is pending.
```
`ui.displayMode.<op>` joins the closed reserved `ui.*` set (`ui.md` §4); the Luau mirror ships with it (Boundary inventory). The `window.displayMode*` fields describe the picked mode: the saved choice, launch default, browsed draft or pending confirm candidate. Stepping changes only the draft; Apply accepts it while windowed or applies through the confirm while exclusive. Borderless refuses stepping and Apply. The `content/dev` menu shows size and refresh.

## Acceptance
### Automated
**Store**
- [ ] `window_mode` and the display mode round-trip through save and load; an absent, unknown or malformed value loads windowed for that field alone and every other setting loads intact.
- [ ] The slot vocabulary matches the store's modes, and the chokepoint maps each mode exhaustively.
- [ ] No fullscreen, monitor or video-mode call exists outside the window-mode chokepoint (grep gate).
**Re-find**
- [ ] A stored mode present in the current monitor's enumeration is chosen; an absent one, or the same size and refresh stored for another monitor, yields borderless and leaves the stored mode unchanged.
- [ ] Duplicate enumeration entries and a 0 Hz report each resolve to one choice.
- [ ] An empty enumeration yields borderless, zeroed and empty picked-mode fields, and step actions that write nothing.
- [ ] On Wayland the enumeration reads empty and exclusive takes the fallback.
**Revert**
- [ ] Keep before expiry persists the new mode; expiry or revert restores the prior mode and writes nothing (P4).
- [ ] An OS readback during a pending confirm does not persist (P3).
- [ ] A change to windowed or borderless persists with no confirm.
- [ ] While a confirm is pending, no save writes the unconfirmed mode, whether it came from the window-mode row or display-mode Apply: not the menu-close flush its opening triggers, not a settled save, not the exit flush; a relaunch after quitting mid-confirm boots the prior mode (P5, P6, P7).
- [ ] A confirm removed by a level load, restart or return to the frontend never confirms; the prior mode returns within 15 s of the change, Loading frames counted, and nothing persists (P8).
- [ ] Keep or revert with no confirm pending writes nothing and changes no mode (P9).
- [ ] While a confirm is pending, a display-mode step or a script write of the window mode is refused and writes nothing; revert restores the mode from before the confirm.
- [ ] Stepping while windowed or exclusive changes only the session-local picked display mode: no window request and no save. Apply while windowed accepts it without a window request; Apply while exclusive opens the confirm, and the stored display mode changes only on keep. Borderless refuses both stepping and Apply.
- [ ] The confirm opens with focus on revert.
- [ ] A mod or level tree registered under the confirm's name is rejected with a load-time diagnostic, and the engine confirm still shows.
**Boot and readback**
- [ ] Settings load once per launch, before the window; no settings write precedes the first presented frame (P1, P2).
- [ ] A saved borderless or exclusive mode is requested once through the chokepoint at boot, after the window is visible and before the first redraw request; the window is never created with a fullscreen attribute (grep gate) (P1).
- [ ] Outside a settle window, a readback differing from the baseline persists it and reseeds the working copy; an equal readback writes nothing.
- [ ] The readback reseed is not observed as a menu write and raises no live apply or second mode request.
- [ ] While the fallback is active, readback writes nothing and the stored exclusive mode survives across frames and a relaunch.
- [ ] Readings taken mid-transition write nothing; the menu's requested mode persists, not the transition's stale reading.
- [ ] A request whose entry fails inside its settle window, the reading returning to windowed, writes nothing and keeps the stored mode; an OS-driven change after the window closes persists as usual.
- [ ] `--windowed` boots windowed over a saved fullscreen mode and leaves the store unchanged; a menu change in that session persists.
- [ ] Under `--windowed` or the fallback, choosing the saved mode itself applies it: exclusive with the confirm when the mode matches, the fallback and its warning when it does not (P10).
**Surface**
- [ ] The Scripting surface example runs as `content/dev` options-menu rows: stepping changes the shown size and refresh, and the window-mode rows write the slot.
- [ ] The Luau SDK carries the display-mode action helper and the window-mode and display-mode slots with the same ops, values and types as TypeScript.
- [ ] With no saved display tuple, launch seeds an enumerated monitor-resolution default; entering Exclusive uses it and shows the confirm. An unavailable saved tuple keeps its fallback behavior, and no boot seeding writes settings.
- [ ] The dev menu disables display-mode arrows and Apply while Borderless is selected, with the entire display-mode setting drawn at 80% opacity; returning to Windowed or Exclusive enables it.
### Manual
- [ ] Windows: boot into each saved mode; the first splash frame shows that mode.
- [ ] macOS: boot into each saved mode with no windowed splash frame after the first; a native Space transition starting at the first frame passes.
- [ ] macOS and Windows: switch live among all three, both directions; leaving exclusive restores the desktop resolution and the prior window size.
- [ ] macOS and Windows: exclusive confirm keeps; expiry reverts.
- [ ] macOS: the green button enters and leaves fullscreen; the menu reflects it, and the next launch matches.
- [ ] Windows: boot reaches the frontend in every saved mode; repeat once U4 lands.

## Path
- macOS: borderless is the recommended fullscreen there; exclusive switches the display mode and locks out Spaces.
- `--windowed` parses at boot stage 1 with `--mod` (`mod_arg`, `startup/session.rs`).
- The settings path comes from `ready/game-user-dirs`' directory chokepoint once that lands.
- Precedents: `render_resolution` end to end for the enum slot; `ui.accessibility.<op>.<field>` and `options/panel_actions.rs` for the intercepted action family; `core/ui/accessibilityPanel.json` and its reserved-name handling for the confirm.
- Boot: `window_attributes()` and `resumed` (main.rs); `PendingSessionInit` carries the loaded store into `Session::build` → `load_player_options`.
- Live apply in `App::update_player_options`; readback beside `App::commit_render_extents`. Mind the source-order tests in `app/render_extents.rs`.
- Either order with `ready/game-user-dirs`: whichever lands second routes the pre-window read through its directory chokepoint.
- Modes differing only in bit depth step as separate entries; a menu showing size and refresh alone shows them as repeats.
- macOS borderless is native fullscreen, an animated Space; `set_simple_fullscreen` avoids Spaces but fails while the window is already in native fullscreen (green button). The first slice shows which the boot AC needs.
- Rivals rejected: a string-list slot plus a repeat widget for the picker (`research.md` §Picker surface); a narrow pre-window read of the window fields alone, which parses twice and leaves two documents to reconcile at save.
- First slice: the pre-window load plus borderless at boot on macOS. It falsifies the riskiest assumption, that applying after visibility and before the first redraw boots cleanly.
- Promotion amends: `boot_sequence.md` §1 (pre-window read), `ui.md` §1.1 (second exempt tree name), `ui.md` §4 (reserved `ui.*` set) and the engine-owned slot list beside the options working copies.
- `main.rs` is far past 800 lines; put the chokepoint and readback in their own `app/` module rather than growing it.

## Boundary inventory

| Name | Rust / store | TOML | TS | Luau |
|---|---|---|---|---|
| Window mode | engine enum | `window_mode` = `"windowed"` / `"borderless"` / `"exclusive"` | `options.windowMode`, same strings | same |
| Stored display mode | engine struct | top-level keys, shape delegated | n/a | n/a |
| Picked-mode fields | engine-owned readonly slots | n/a | `window.displayModeWidth`, `…Height`, `…RefreshHz`, `…BitDepth` (number), `…Monitor` (string) | same |
| Revert countdown | engine-owned readonly slot | n/a | `window.displayModeRevertSeconds` (number) | same |
| Display-mode actions | App-intercepted `ui.*` family | n/a | `"ui.displayMode.next"` / `previous` / `apply` / `keep` / `revert`; helper `displayModeAction(op)` | same strings; helper `displayModeAction` |
| Confirm tree | reserved registry name, `core/ui/` | n/a | not registrable | not registrable |
| Boot escape | stage-1 flag `--windowed` | n/a | n/a | n/a |

## Open questions
- Stored display-mode key shape. — **delegated**
- macOS borderless as native Space or simple fullscreen, from the first slice's evidence. — **delegated**: reported at the resumable plan review
