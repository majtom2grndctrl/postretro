# window-modes — research

Read at 67fddd0af. Derivation behind the brief's Decisions; nothing here binds.

## Boot and store

- `window_attributes()` (main.rs) sets title and logical 1280×720 only. `resumed` creates the window once, in `Booting`. No fullscreen or monitor API is called anywhere under `crates/`.
- `options::settings_path` is `ProjectDirs::from("", "", "postretro")` + `settings.toml`. `PlayerOptions::load_with_status(&Path)` is one read plus a `toml::Table` parse, per-field tolerant; no session dependency. `Session::build` → `load_player_options` loads, generates a missing `player_id`, and does the first-launch save.
- `PlayerOptions.stored: StoredDocument` keeps the parsed table for round-trip save. `DocumentWriter` overwrites only known keys; unknown top-level and nested tables survive (`save_keeps_unrecognized_values_and_unknown_keys_until_the_player_writes_them`).
- `OptionsBridge::update` has no `Window`. Its effects reach `App::update_player_options`, which can reach `App.window_state`. `App::commit_render_extents` runs once per frame after `Resized` / `ScaleFactorChanged` record their values; source-order tests in `app/render_extents.rs` pin the arm order.

## winit 0.30.13

- macOS: a creation-time fullscreen runs `set_fullscreen` before `makeKeyAndOrderFront` (`WindowDelegate::new`). Borderless is native fullscreen via `toggleFullScreen` — an animated Space. A failed initial entry retries every 0.5 s. Hidden + fullscreen is unguarded; AppKit's behavior on an unshown window is unknown.
- Windows: creation-time fullscreen applies after `CreateWindowExW`; a hidden window stays hidden but is sized to the monitor. Exclusive calls `ChangeDisplaySettingsExW` (`CDS_FULLSCREEN` plus a topmost window), not DXGI exclusive ownership — expect wgpu to see an ordinary swapchain (inference).
- Green button: `window_will_enter_fullscreen` sets `Borderless(current_monitor)`, so `fullscreen()` reports the target at transition start. No fullscreen event, only `Resized` / `Moved`. On exit the old value holds until the transition completes. `set_fullscreen` mid-transition is queued in `target_fullscreen`.
- `VideoModeHandle`: only from `MonitorHandle::video_modes()`; a `Window` alone reaches monitors (`current_monitor`). `Eq`/`Hash`/`Ord` over size, bit depth, refresh (mHz), monitor; not serializable. macOS substitutes the monitor's rate for a 0 Hz report; duplicates occur.
- A failed exclusive mode change hits `assert!` on both platforms — a mode must come from a fresh enumeration.
- Exiting exclusive restores the display configuration and the saved window bounds on both platforms.
- Wayland ignores exclusive.

## Picker surface

- `SlotValue`: `Number`, `Boolean`, `String`, `Enum` (catalog-fixed values), `Array(Vec<f32>)`. No string list, no runtime-valued enum.
- No widget repeats over data; menu lists are built in script at registration, and the VM drops after load (`scripting.md` §2, `ui.md` §1.1). No primitive returns engine data to a menu.
- Precedent for engine-stepped values: `ui.accessibility.<op>.<field>` (`options/panel_actions.rs`), intercepted by the App before named-reaction dispatch (`ui.md` §3, §4.1). The `ui.*` reserved set is closed and mirrors into the SDK.
- A `Text` bind reads any `ScalarStateValue` (`number | boolean | string`), so a readonly string slot can show the engine's label.
- Rival: a string-list slot plus a repeat widget. Needs a new `SlotValue` case and widget kind and breaks register-time menu construction. Rejected.
- Engine-owned modal precedent: `core/ui/accessibilityPanel.json` under a reserved, non-shadowable registry name (`ui.md` §1.1, §4.1).

## Precedent (model knowledge, not verified)

Display-mode revert countdowns are the PC convention (Windows display settings, most PC games).

## Ordering pins

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Saved fullscreen mode at boot | Window visible → mode applied → first redraw requested | First splash frame presents in the saved mode; nothing waits on a paint to a hidden window |
| P2 | First launch, no file | Pre-window load → window → first pixel → `player_id` generation and first-launch save | No file write precedes the first presented frame |
| P3 | OS changes the mode during a pending exclusive confirm | Confirm pending → OS readback differs | Readback does not persist; the confirm resolves first |
| P4 | Confirm expires on the frame the player presses keep | Keep and expiry on one tick | Keep wins; the new mode persists |
| P5 | Confirm opens over the options menu | Menu change into exclusive → confirm pushed → options menu no longer top → menu-close flush, same frame | The flush writes the prior mode; the file never holds the unconfirmed mode |
| P6 | A settled save comes due mid-confirm | Earlier change arms the 250 ms save → change into exclusive → confirm pending → save settles | The save writes the prior mode |
| P7 | Quit during a pending confirm | Confirm pending → window close, exit action or quit chord → clean-exit flush | The flush writes the prior mode; the next launch boots the prior mode |
| P8 | Level load, restart or return to frontend during a pending confirm | Confirm pending → script reaction or host relevel clears or replaces the modal stack → Loading frames skip the logic stage | Never confirms; the prior mode returns no later than 15 s after the change, Loading frames and unfocused time counted; nothing persists |
| P9 | Keep or revert arrives with no confirm pending | Confirm already kept, expired or reverted → keep or revert fires | Writes nothing and requests no mode change; a revert never undoes a kept mode |
| P10 | Player picks the saved mode while `--windowed` or the fallback is in effect | Store holds exclusive, window is windowed or borderless → menu writes exclusive, equal to the store | The change applies: exclusive with the confirm when the mode matches, the fallback and its warning when it does not |
