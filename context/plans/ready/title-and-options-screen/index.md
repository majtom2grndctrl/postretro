# title-and-options-screen

Brief · compact · reads: `context/lib/ui.md` §1,§3,§4 · `context/lib/player_options.md` · `context/lib/boot_sequence.md` §4 · read at a6ebb79

## Problem
The developer building the dev mod raised it: the mod's front end is a bare
level-select grid (`frontend.devLevelSelect`), and the shipped player options
(sensitivity, invert-Y, view-feel scale, crouch mode) persist to `settings.toml`
but are unreachable in-game — the settings-menu seam `player-options` and
`mod-frontend-hub` both deferred is still unbuilt. This is a requested
capability: a title screen (Play / Options / Exit) and an options screen that
reads current settings and, on change, applies them live and persists them,
plus two new player-facing graphics tiers (real-time shadow quality, fog
quality). Done: launching the dev mod opens a title screen; Play reaches the
level-select grid, Exit shuts down cleanly, Options opens a screen whose
controls reflect the persisted values and, when changed, take effect (live
where the subsystem supports it) and are written back to `settings.toml`.

## Decisions
- **Title screen is content, not engine.** The dev mod's new title tree becomes
  `frontend.menuTree`; Play → `openMenu` the existing `frontend.devLevelSelect`
  grid as a submenu, Options → `openMenu` the new options tree, Exit →
  `ui.exitToDesktop`, Back → `ui.closeDialog`. `mod-frontend-hub` shipped the
  `frontend` block and reserved actions and deferred "the 'Options' button whose
  action is wired later" to a dedicated settings plan — this is that plan. No new
  engine menu machinery.
- **Surface the core four existing options:** `mouse_sensitivity` (Slider),
  `invert_y` (bool), `view_feel_scale` (Slider), `crouch_mode` (Hold/Toggle enum).
  `scroll_notch_pixels` and `switch_cycle_dwell_ms` are non-goals — advanced knobs
  a reader would not assume this brief owes.
- **Controls are pure composition; no engine UI change.** Numeric controls use the
  existing Slider; bool/enum controls are Button with the existing a11y machinery
  (`role` checkbox/radio/tab + a `checked`/`selected` `Predicate` + `styleRanges`
  highlight) plus one authored `setState` reaction per value. Mirrors the shipped
  tabs demo and `ui.md` §1.1 "components are plain functions." No new `Widget` kinds,
  no value-carrying write primitive.
- **Options write/apply/persist seam (engine).** New engine-owned *writable* option
  slots (an `options.*` namespace, the `ui.textEntry` precedent), exposed to the SDK
  on the `getGameState()` tree beside the existing engine slots (`screen.*`,
  `session.*`), are seeded from `PlayerOptions` when the options screen opens;
  controls write them via `setState` at the game-logic stage (`ui.md` §3: no store
  write originates in the UI module); an app-side options bridge observes a changed
  slot, updates the `PlayerOptions` field, applies it, and saves on change (atomic
  `save`, debounced). `PlayerOptions` stays the persistence home (`player-options`
  §4: the store the settings menu reads and writes; the options-vs-saves boundary §1
  makes quality tiers options, not saves).
- **Apply model: fog live, shadow on reload.** Input and accessibility options apply
  live. `fog_quality` applies live **and is re-applied on renderer full-init** (the
  idempotent rebuild — suspend/resume — reconstructs the fog pass at its default, so
  a change-only apply would silently revert). `shadow_quality` is persisted and
  applied only at renderer full-init / next level load — never live — with an
  "applies after reload" hint on the control. Graphics setters reach the renderer
  through an app-side chokepoint (`startup/render_profile.rs`), never `UiReadSnapshot`;
  the renderer stays sole GPU owner.
- **Graphics tiers are new `PlayerOptions` enum fields** (`shadow_quality`,
  `fog_quality`), `serde(default)` + `sanitize`, persisted in `settings.toml`. The
  renderer-side `GraphicsSettings` struct named by `graphics-mode-toggle` is a
  design reference for the renderer's applied state, not the persisted home.
- **Shadow tier varies only the spot shadow-map resolution** (`SHADOW_MAP_RESOLUTION`
  → a renderer-construction field seeded from `PlayerOptions`). `SHADOW_POOL_SIZE`,
  PCF taps, and cube-face resolution are WGSL-pinned — they size shader arrays and
  are drift-tested — so they stay fixed; varying them needs shader permutation, a
  non-goal.
- **Fog tier drives `fog_step_size` globally** (march density; the dev-tools fog
  quality slider's knob). Non-goal: a global override of per-map `fog_pixel_scale` —
  worldspawn keeps pixel-scale authority per the fog-volumes design (a bounded
  ceiling is an Open question).
- **Input/keybind remapping and gamepad map are non-goals** — genuinely unbuilt
  (hardcoded `default_bindings`, no rebind API, no binding persistence); own brief,
  per owner. Gamepad dead zone (a hardcoded const) likewise deferred.
- **Not doing:** live shadow rebuild on change (it would ride a full-renderer rebuild
  that drops and recreates *all* pipelines, not just shadow — out of proportion to a
  tier change); reworking pause menu / HUD onto the composed controls (a separate
  surface); a graphics-mode (TrueRetro/PostRetro) player surface — that renderer mode
  was shipped then retired (`retire-true-retro`), so no live seam exists; a future
  graphics option would rebuild it, out of scope here.

### Scripting surface
The options screen is authored in the mod SDK. The contract this brief adds is the
engine-owned `options.*` slot group, exposed on `getGameState()` beside `screen.*` /
`session.*`, and the composition pattern its controls bind to — no new SDK factory or
widget. The group ships a Luau mirror (drift-tested), so it adds an SDK-visible *slot*
surface; this is a witting, minimal departure from `player_options.md` §6 "no SDK
types," which still holds for the `PlayerOptions` fields themselves.

```ts
// Proposed design — options screen body (mod content). `getGameState().options` is the
// engine-owned option-slot group this brief adds, seeded from PlayerOptions on open.
import { Slider, Button, VStack, updateState, stateEquals, getGameState } from "postretro/ui";
import { defineReaction } from "postretro";
const opts = getGameState().options;

// Numeric: Slider reads and self-writes its bound option slot (bind takes a Ref<number>).
Slider({ id: "opt-sens", label: "Mouse Sensitivity", bind: opts.mouseSensitivity,
         min: 0.1, max: 10, step: 0.1 });

// Enum (crouch mode, shadow/fog tiers, and a bool as its two-value case): one Button per
// value, role "radio", highlighted from the same slot predicate via `checked`.
export const setCrouchHold = defineReaction({ name: "opt.crouchHold",
  steps: [updateState(opts.crouchMode, "hold")] });
Button({ id: "opt-crouch-hold", label: "Hold", role: "radio",
         checked: stateEquals(opts.crouchMode, "hold"), onPress: "opt.crouchHold" });
// ...one sibling Button per remaining value.
```

## Acceptance

### Automated
- [ ] `PlayerOptions` round-trips `shadow_quality` and `fog_quality` (snake_case in
      `settings.toml`); an absent field loads as its default. An unknown enum value does
      not corrupt the file: the load falls back to all-defaults and leaves the file
      untouched (whole-file fallback, matching the existing corruption path).
- [ ] A `setState` into each engine-owned option slot coerces and validates: bool for
      `invertY`, the enum set for `crouchMode`/`shadowQuality`/`fogQuality`, a clamped
      number for `mouseSensitivity` (>0) and `viewFeelScale` (in [0,1]). A write to a
      readonly engine slot warns and no-ops; a write to an unknown slot is rejected by
      the coerce path (unknown-slot error), the `setState` reaction logs it and applies
      nothing — asserted at the layer that produces the warning.
- [ ] Opening the options screen with a non-default `settings.toml` shows each control
      reflecting the persisted value, not the default (slots seed from `PlayerOptions`).
- [ ] One option-slot change updates exactly the matching `PlayerOptions` field and
      triggers exactly one save after settling; a slider drag across N ticks debounces to
      a single save whose persisted value equals the final settled value (last-write-wins),
      not an intermediate one; a pending save flushes on options-screen close and on clean
      exit; a save leaves no partial/truncated file. [pin: ord-debounce-last]
- [ ] Fog tier → `set_fog_step_size`: a high tier and a low tier each call the setter
      with that tier's step size (both ends).
- [ ] `fog_quality` is re-applied on renderer full-init, not only on slot change: after a
      full renderer rebuild (suspend→resume), `fog.step_size` reflects the persisted
      `fog_quality`, not `DEFAULT_FOG_STEP_SIZE`. [pin: ord-fog-rebuild]
- [ ] Shadow tier → resolution: a high tier and a low tier map to distinct spot
      shadow-map resolution values read at renderer construction; the pool's per-slot
      texture allocation reads the resolution from the field, not the const, while
      pool-slot count stays at the WGSL-pinned `SHADOW_POOL_SIZE`.
- [ ] A composed toggle/enum control: its `checked`/`selected` predicate resolves from
      the option slot, and its `onPress` reaction sets/flips that slot.
- [ ] Reopening the options screen after a change but before the debounced save flushes
      re-seeds each slot from the in-memory `PlayerOptions` field (the shown value is the
      just-changed one; the seed never re-reads `settings.toml`). [pin: ord-seed-fresh]
- [ ] An input option changed with no level loaded updates the session `InputSystem` (and
      `PlayerOptions`); after a level installs, the first gameplay frame reflects it —
      level install does not re-default it. [pin: ord-title-to-level]
- [ ] After `shadow_quality` changes and a renderer full-init / level reload runs, the
      shadow allocation is seeded from the current `PlayerOptions.shadow_quality`, not the
      value read at first renderer construction. [pin: ord-shadow-reload]
- [ ] When the debounced save fails (io error) the in-memory field and slot keep the
      changed, already-applied value (no rollback) and `settings.toml` is left intact; the
      failure logs and does not desync slot from field. [pin: ord-save-fail]
- [ ] GREP GATE: no `shadow_quality` slot-change path triggers a live renderer rebuild;
      shadow re-apply is reachable only from renderer full-init / level load.
- [ ] GREP GATE: graphics apply reaches the renderer only through the app-side chokepoint
      (`render_profile.rs` precedent), never `UiReadSnapshot`; no store or graphics write
      originates in the UI module.

### Manual
- [ ] Launch the dev mod → title screen with Play / Options / Exit. Play → level-select
      grid, Back → title. Options → options screen, Back → title. Exit → clean shutdown.
- [ ] Sensitivity slider changes look speed live; persists across relaunch.
- [ ] Invert-Y toggle flips pitch live, its highlight tracks state; persists.
- [ ] View-feel-scale at 0 suppresses bob/tilt/sway live (`player_options` §5); persists.
- [ ] Crouch-mode control switches hold/toggle behavior live, selected value highlighted;
      persists.
- [ ] Fog tier visibly changes fog march density live on a fogged map, no reload; persists.
- [ ] Shadow tier shows the "applies after reload" hint; after a level reload / relaunch,
      shadow-map resolution visibly changes on a map with a shadow-casting spot/point
      light; persists.
- [ ] Options changed from the title screen (no level loaded) persist and apply on the
      first map loaded.
- [ ] First boot with no `settings.toml`: every control shows its default; changing one
      creates `settings.toml` (with `shadow_quality`/`fog_quality` present) and persists
      across relaunch. [pin: ord-first-boot]
- [ ] The Options screen registered under `frontend.menuTree` contains one reachable,
      focusable control per surfaced option (sensitivity, invert-Y, view-feel, crouch,
      shadow quality, fog quality), operable by keyboard/gamepad from the title's Options
      button — the seed and reaction rows prove the wiring in isolation; this proves they
      reach a usable screen.
- [ ] A corrupt `settings.toml` falls back to defaults without overwriting the file, with
      the new fields present.
- [ ] Menu-stack discipline: Options opened from the title (a second modal) then Back
      returns to the title; `nav.cancel` / Start do not pop the wrong modal or leave the
      title without a capturing modal.

## Path
- **Seams/precedents (by symbol):** `mod-frontend-hub` frontend block +
  `openMenu`/`showDialog`/`ui.*` reserved actions; `production-pause-menu` modal
  envelope (`Tree` capture/`initialFocus`/`accessibleName`/`role`; `VStack`
  `focus: linear,wrap`). Engine-owned option slots follow `ui.textEntry`. Slider
  self-write via `capture_slider_step` (`input/ui_focus.rs`). Composed toggle/enum
  mirror the tabs demo (`role` + `checked` predicate + `styleRanges`). Graphics
  apply follows the `startup/render_profile.rs` chokepoint; input apply uses the
  existing `InputSystem::set_*` setters.
- **Shape vs rival (seam):** engine-owned option slots + app-side bridge to
  `PlayerOptions` with live apply and debounced save-on-change. Rival — an explicit
  Apply/Save button reading slots on submit — is simpler but abandons save-on-change
  (`player-options` §4) and reads worse; rejected.
- **Shape vs rival (controls):** the stronger rival to pure composition is a
  self-writing Toggle/Segmented `Widget` kind (the exact `Slider`/`capture_slider_step`
  precedent), which would collapse per-value reactions to zero and give a growing
  graphics menu a cleaner surface. Rejected for this brief: it expands the closed
  `Widget`/`NodeInteraction` enums plus the hand-written JS+Luau descriptor bridge and
  drift tests — an engine wire-format change out of proportion to a compact brief whose
  sole consumer is the dev mod, and Button already carries the toggle/segmented a11y
  machinery (`role`/`checked`/`styleRanges`). The per-value-reaction cost is bounded
  (2–3 values per enum) and the widget can be added later without reworking this seam.
- **First slice (falsifies the riskiest assumption):** wire ONE option end to end —
  `mouse_sensitivity` Slider → `options.mouseSensitivity` slot → bridge →
  `PlayerOptions` field + `InputSystem::set_mouse_sensitivity` + debounced `save` →
  reflected on reopen and after relaunch. Proves the whole seam (slot, seed-on-open,
  bridge, live apply, persistence) before adding breadth.
- **Menu-camera hold:** the frontend menu-camera pose is reapplied per frame gated on
  the frontend menu being the top modal (`mod-frontend-hub` Task 4). Verify the pose
  still holds when Play/Options submenus are pushed on top; if the gate drops it,
  widen the gate to "a frontend-owned modal is topmost."
- **Shadow const→field:** `SHADOW_MAP_RESOLUTION` (`spot_shadow.rs`) becomes a
  renderer-construction field seeded from `PlayerOptions`; reload-apply re-seeds and
  rebuilds via `finish_full_init` (or at next level load). `SHADOW_POOL_SIZE` and the
  WGSL-pinned consts stay fixed.
- **Per-option apply mechanism (re-verify):** `mouse_sensitivity`/`invert_y` →
  `InputSystem::set_*`; `view_feel_scale` → already read at render assembly each frame;
  `crouch_mode` → read at use; `fog_quality` → `set_fog_step_size` on change and on
  full-init. The bridge updates the `PlayerOptions` field and calls the setter wherever
  the value is cached rather than read live.
- **Placement:** the options bridge lands in its own module (near `options/`), not
  swelling `main.rs`.

## Open questions
- Shadow/fog tier count, labels, and concrete param values (resolution/pool for
  shadow; `step_size` for fog) — **delegated**: executor tunes with
  `POSTRETRO_GPU_TIMING` and by eye, reports in the plan of record.
- Whether `fog_step_size` alone gives adequate visible quality range or a
  `fog_pixel_scale` ceiling is also warranted — **delegated**: executor decides during
  build and reports. Boundary: `set_fog_step_size` has no per-map authority, so the
  global tier is clean; a bounded `pixel_scale` *ceiling* (player may request coarser,
  never finer than the map) is admissible, but a flat global override of worldspawn
  `fog_pixel_scale` is not — it would violate the fog-volumes authoring authority.
- Save-debounce mechanism — none exists today; add one (write-on-release vs short timer)
  so slider drag does not thrash disk — **delegated**.

## Boundary inventory
| Name | Rust | Wire / persist | UI / SDK | Notes |
|---|---|---|---|---|
| shadow quality | `PlayerOptions` enum field | `settings.toml` `shadow_quality` (snake_case) | n/a (not an SDK type) | serde(default)+sanitize; reload-apply |
| fog quality | `PlayerOptions` enum field | `settings.toml` `fog_quality` | n/a | live via `set_fog_step_size` |
| option slots | engine-owned writable slots | dotted slot names, camelCase (`options.mouseSensitivity`, …) | exposed on `getGameState().options`; bound by UI descriptor; written via `setState` | seeded from `PlayerOptions`; bridged back |
| options bridge | app-side module | n/a | n/a | slot change → `PlayerOptions` field + apply + debounced save |
