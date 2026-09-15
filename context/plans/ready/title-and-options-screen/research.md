# title-and-options-screen — research

Derivation and grounded findings behind the brief. Read at `a6ebb79`. Symbols
are re-verify targets for the executor, not binding line references.

## Prior commitments this brief consumes

- **`plans/done/mod-frontend-hub`** — shipped the `frontend` manifest block
  `{ menuTree, backgroundLevel?, camera }`, the game-flow verbs (`loadLevel`,
  `restartLevel`, `returnToFrontend`), and reserved `ui.quitToMenu` /
  `ui.exitToDesktop`. It **explicitly deferred** "Options/settings menu and
  `settings.toml` mutation … to a dedicated settings plan. The menu may carry an
  'Options' button whose action is wired later; `PlayerOptions` is untouched
  here." This brief is that dedicated settings plan. The Options button opens the
  options tree via the existing `openMenu` — no new game-flow verb.
- **`plans/done/player-options`** — shipped `PlayerOptions` + `settings.toml`;
  deferred the settings menu (the E13 seam) and keybind remapping. "v1 ships
  fixed bindings; remapping is its own later spec" (`index.md:21`); gamepad dead
  zone deferred (`index.md:24`).
- **`plans/done/graphics-mode-toggle`** — deferred "Player-facing / persisted
  graphics setting" (`index.md:25`); named a future
  `GraphicsSettings { texture_filtering, aniso_clamp, … }` as the renderer-side
  durable home, introduced "once a second independent knob exists"
  (`index.md:113`). Shadow/fog quality are those second knobs. Note: `GraphicsMode`
  shipped, then was deliberately removed by `plans/done/retire-true-retro` — the
  `GraphicsMode`/`set_graphics_mode` symbols are absent because retired, not because
  unshipped. There is no live graphics-mode seam to reuse; treat the struct name as
  design-reference guidance for the persistence-vs-applied split only.

## Frontend / menu (title screen)

- Dev frontend today = `frontend.devLevelSelect`, a level-select grid, one
  `loadLevel(id)` reaction (`frontend.start.<id>`) per catalog map. Authored in
  `content/dev/scripts/frontend-menu.ts`; registered via
  `ModManifest.frontend.menuTree` in `content/dev/start-script.ts`. No
  Play/Options/Exit. Engine fallback = `content/base/ui/frontendMenu.json`
  (`FRONTEND_MENU_NAME`), a bare "NO MOD FRONTEND REGISTERED" placeholder.
- Menu-push primitives: `openMenu(tree)` and `showDialog(tree, onCommit?)` push a
  named registered tree as a modal (`PushTree`), system reactions registered in
  `crates/sim/src/scripting_systems/system_reactions.rs`. `ui.closeDialog` pops;
  `ui.exitToDesktop` = clean shutdown; `ui.quitToMenu` = return to frontend.
  Dev pause menu (`content/dev/scripts/pause-menu.ts`) already uses all three.
- Menu camera pose (mod-frontend-hub Task 4) is reapplied per frame **gated on
  the frontend menu being the top modal**. Pushing a submenu on top may drop that
  gate — executor must confirm the pose still holds when Play/Options submenus are
  pushed (likely fine once the backdrop is installed and no per-frame writer moves
  the camera, but verify).

## PlayerOptions store

- `crates/postretro/src/options/mod.rs`: `struct PlayerOptions` fields —
  `player_id: Option<[u8;16]>`, `mouse_sensitivity: f32` (reset if ≤0/non-finite),
  `invert_y: bool`, `view_feel_scale: f32` (clamp [0,1]), `crouch_mode: CrouchMode`
  (Hold/Toggle, snake_case wire), `switch_cycle_dwell_ms: Option<u32>` (≤60_000),
  `scroll_notch_pixels: f32` (≤4096). All `serde(default)`; `sanitize()` clamps.
- API: `save(&self, path) -> io::Result<()>` (serialize TOML → sibling `.tmp` →
  rename, atomic); `settings_path() -> Option<PathBuf>`; `load_with_status`.
  Corruption → warn + in-memory defaults, file untouched.
- Only save site is boot (`session/mod.rs` `load_player_options`): defaults-write /
  device-identity-write. **No runtime save-on-change path exists.**
- Boot seeds input from options: `set_mouse_sensitivity`, `set_invert_y`,
  `set_scroll_notch_pixels` on `InputSystem` (`session/mod.rs`). `view_feel_scale`
  is read at render assembly (`main.rs`); `crouch_mode` read at use (`main.rs`).

## UI widgets + write path

- Widget vocabulary = 12 kinds (`crates/scripting-core/src/ui/descriptor/widgets.rs`,
  `enum Widget`, `#[serde(tag="kind", rename_all="camelCase")]`): Text, Panel,
  Image, VStack, HStack, Grid, Spacer, Button, Slider, Bar, Ring, Announce.
- Interactive set is closed by `enum NodeInteraction` (`crates/ui/src/tree/draw.rs`):
  **only** `Button { on_press, repeat_on_hold }` and
  `Slider { slot, min, max, step, captures_nav }`. No toggle/checkbox, radio,
  stepper, or dropdown/enum-picker.
- Slider self-emits the value: `capture_slider_step`
  (`crates/postretro/src/input/ui_focus.rs`) computes the stepped value; the app
  synthesizes a `setState { slot, next }` command on the N+1 frame. This is the
  model the new Toggle/Segmented widgets follow (widget supplies the value, no
  per-value authored reaction).
- ButtonWidget carries reactive `selected` / `checked` predicate fields
  (aria-selected / aria-checked) — display-only read-back of a slot, they do not
  write. Button `on_press` is a bare string and **cannot carry an argument**;
  composing an enum from buttons needs one reaction per value.
- `setState` (`crates/sim/src/scripting_systems/system_reactions.rs`) writes
  **store slots**, not `PlayerOptions`. `write_state_slot_json`
  (`crates/scripting-core/src/store_bridge.rs`) rejects unknown / per-owner /
  readonly slots (warn+no-op), coerces to slot native type (Number/Boolean/String/
  Enum/Array) and clamps. Engine-owned writable slot precedent: `ui.textEntry`.

## Renderer seam (graphics apply)

- No `RenderConfig`. `Renderer::new(window)` (boot-ready). Full renderer built by
  `finish_full_init` / `build_full_renderer`; it is **idempotent** — drops and
  rebuilds all `FullRenderer` pipelines/resources in-process (no process restart),
  used on suspend→resume. Device features/limits requested once at device
  creation (only true restart-required change).
- Renderer→settings channel = typed `Renderer::set_*` setters + an app-side
  translation chokepoint (`crates/postretro/src/startup/render_profile.rs`:
  `renderer_bloom_profile` → `App::apply_mod_bloom_render_profile` →
  `renderer.set_bloom_render_profile`). NOT `UiReadSnapshot` (that carries UI/slot
  display data only). App owns the renderer (boot-lifetime field).
- Live precedents (no restart): `toggle_vsync` (present mode; reached from user
  input, not dev-tools gated), `set_fog_pixel_scale` / `set_bloom_render_profile`
  (rebuild a pass's own textures), `resize` (rebuilds depth/screen-effects/bloom/
  fog targets), and per-frame-uniform scalars.

### Fog quality — runtime setters exist

- `Renderer::set_fog_step_size` (`crates/renderer/src/render/renderer_state.rs`):
  per-frame uniform write, cheap, no rebuild. Already on a dev-tools slider.
- `Renderer::set_fog_pixel_scale` → `FogPass::set_pixel_scale`
  (`crates/renderer/src/render/fog_pass.rs`): recreates the scatter target +
  rebuilds group-6 bind group. Clamp [1,8]. Currently set at level load from
  worldspawn (`crates/postretro/src/startup/lifecycle.rs`). **Per-map authored
  today** → a global player fog-quality override needs a precedence policy vs the
  map's worldspawn value (open question).
- Fixed (compile/shader): `max_steps` (256), transmittance cutoff, MAX_* caps,
  scatter format, composite sampler. No fog quality tier/preset enum today.

### Shadow quality — no runtime setter

- All quality params are compile-time consts, allocated into GPU textures at full
  init: `SHADOW_MAP_RESOLUTION` (1024), `SHADOW_POOL_SIZE` (96)
  (`crates/renderer/src/lighting/spot_shadow.rs`); `CUBE_FACE_RESOLUTION` (512),
  `CUBE_COUNT` (`crates/renderer/src/lighting/cube_shadow.rs`); PCF 3×3 in
  `crates/renderer/src/shaders/shadow_sample.wgsl`. Cube face resolution + PCF are
  **WGSL-pinned by tests** (`forward_cube_sampling_constants_match_pool`) —
  varying them needs shader permutation → out of scope.
- No runtime setter for any shadow quality param. Tier can vary only the Rust-side
  allocation params (spot map resolution, pool size), seeded at renderer
  construction. Owner chose reload-apply for shadow (not live rebuild).

## Ordering pins (from /review-brief rows lens)

| id | scenario | ordering | expected |
|---|---|---|---|
| ord-seed-fresh | Options reopened after a change, before the debounced disk save flushes | bridge writes the in-memory `PlayerOptions` field on slot change → screen reopens → slots re-seed from in-memory `PlayerOptions`, never re-reading `settings.toml` | reopened controls show the just-changed value, not the last-saved-to-disk value |
| ord-first-boot | Options opened on a first boot with no `settings.toml` | boot writes defaults atomically → in-memory `PlayerOptions` = defaults → slots seed from defaults; a change writes `settings.toml` via the atomic save (creating file + parent dir) | controls show defaults; the first change creates `settings.toml` with the new value and the new `shadow_quality`/`fog_quality` keys present |
| ord-title-to-level | Sensitivity/invert_y/crouch changed at the title (no level loaded), then a map loads | bridge applies via session-lifetime `InputSystem` setters + updates `PlayerOptions`; `InputSystem` survives level install; render assembly re-reads `PlayerOptions.view_feel_scale` each frame | the changed value is in force on the first gameplay frame of the newly-installed level |
| ord-shadow-reload | `shadow_quality` changed mid-session, then a level reload / renderer full-init runs | reload-apply seeds the shadow allocation from the CURRENT `PlayerOptions.shadow_quality`, not the value captured at first renderer construction | the rebuilt pool uses the newly-persisted tier, not the construction-time tier |
| ord-save-fail | A slot change whose debounced disk save fails (io error) | field + slot updated and applied live BEFORE the save; a save `Err` logs and leaves field/slot at the changed value (no rollback); atomic write leaves no corrupt file | change stays applied in-memory; `settings.toml` untouched/uncorrupted; a later change retries the save |
| ord-debounce-last | Two slot changes on one tick / a slider drag across N ticks / screen closed or app exits mid-debounce | apply-live per change; the disk save is debounced to the settle boundary; the LAST settled value is saved; a pending save flushes on options-screen close and clean exit | persisted value == final settled value (never a stale mid-drag value); exactly one save after settling; no lost write on a normal close/exit |
| ord-fog-rebuild | `fog_quality` set live (persists across level load via `full.fog.step_size`), then an idempotent renderer full-init rebuild (suspend→resume) runs | `FogPass` is reconstructed at `DEFAULT_FOG_STEP_SIZE`, dropping the applied value; the bridge re-applies `fog_quality` on full-init, not only on change | after suspend/resume, fog march density still reflects the persisted `fog_quality` |

Owner-settled during /review-brief: shadow tier varies `SHADOW_MAP_RESOLUTION`
only (`SHADOW_POOL_SIZE` is WGSL-pinned); unknown enum values fall back
whole-file to defaults; fog re-applies on full-init.

## Input mapping (deferred — confirms non-goal)

- Bindings are hardcoded tables: `default_keyboard_mouse_bindings`,
  `default_gamepad_bindings`, `default_bindings` (`crates/postretro/src/input/defaults.rs`).
  `InputSystem.bindings` is private with an explicit "no rebind write site outside
  `new()`" note (`crates/postretro/src/input/mod.rs`). No binding persistence in
  `PlayerOptions`. Genuinely unbuilt → own brief. Gamepad dead zone is a hardcoded
  const (`crates/postretro/src/input/gamepad.rs`).
