# window-modes

Seed · needs `/draft-session` before a brief · read at 1527f5b26

Scope: window modes, and now settings scope. Both turn on what can be read before the window exists or before the game is known.

## Problem
A player can only play in a window. There is no fullscreen of any kind. The developer wants window-mode options; this is a requested capability, split out of `hidpi-render-scale` because it raises questions that work does not need.

**Settings scope.** `settings.toml` is one file per OS user, shared by every game and mod built on the engine. Some settings follow the machine or person; others follow the game. E23 U3 lets a game set its own default bindings (`drafts/E23--gamepad-input`), so without per-game scope a player's rebinds in one game would leak into another. A standalone shipped game should probably not write to a `postretro` config directory at all.

## Owner decisions so far
- Three modes: windowed, borderless fullscreen, and exclusive fullscreen with a display-mode picker.
- First launch stays windowed.
- On macOS, borderless is the recommended fullscreen choice. Exclusive fullscreen there switches the display mode and locks out Spaces and task switching.
- The engine reads the actual mode back from the window. The macOS green button and OS transitions change the mode without engine involvement.
- Settings split along "follows the machine or person" vs "follows the game". Direction: one file, GZDoom-style. A machine section is read at `Session::build`; per-game sections `[game."<mod_id>"]` are read at mod init, once the id is known. Game-scoped data needs no boot-order change.
- E23 U3 lands bindings at `[game."<mod_id>".bindings.<device_class>]`, the first game-scoped occupant (`drafts/E23--gamepad-input` decision 6). This work classifies the remaining fields; it does not migrate bindings.
- This seed owns settings scope, not E23 U3.

## Direction-review advice to weigh
Build windowed and borderless first. Add exclusive only if Windows testing makes the case for it.

## Questions the session must settle
- **Boot order.** Player options load after the first pixels (`boot_sequence.md`). A saved fullscreen mode must either switch mid-splash on every launch or be read narrowly before the window exists. The second is a divergence from the documented boot sequence.
- **E23 U4.** The ready plan `E23--accessibility` creates the window hidden for the `accesskit_winit` adapter. Window-mode application must order correctly against that reveal.
- **Option surface.** A `PlayerOptions` field needs a window-side chokepoint beside the render profile, because a mode change is a winit call, not a renderer setter. The `options.*` slot is a scripting-surface change.
- **Proof.** Mode switching needs a manual check on both macOS and Windows.
- **Field classification.** Candidate, not decided:
  - Machine/person: `render_resolution`; shadow, fog, and Surface Depth quality; window mode; the `[accessibility]` group with `view_feel_scale` (reduce motion, flash limiter, mono, bus volumes); `mouse_sensitivity`; `invert_y`; `player_id`.
  - Game: bindings (U3); mod-declared gameplay settings.
  - Arguable: `crouch_mode` and U3's planned `sprint_mode`; `accessibility_panel_shown` (per game, the first-launch panel shows once per game); `player_id` (seat reclaim is per host session).
  - Unplaced by the candidate: `switch_cycle_dwell_ms`, a local override of the mod's switching policy; `scroll_notch_pixels`, a per-device value.
- **One read or two.** Whether window mode's narrow pre-window read and the machine section are the same read.
- **Config directory identity.** Who names a standalone game's config directory: the `dist` payload, the launcher, or `postretro.toml` `[package]`? The SDK/dev case runs many mods on one install.
- **Mod id as a persistence key.** The id is required but only author-declared. What happens when two games share an id, or a game changes its id between releases?
- **Migration.** Candidate, not decided: leave top-level keys as implicit machine scope, so existing files need no migration. The alternative moves them into an explicit machine section.
- **Mod-state storage.** Persisted mod state already sits per mod id beside the config dir (Grounding). Do `[game."<mod_id>"]` settings live in `settings.toml` or beside that state? Are mod-declared gameplay settings settings at all, or persisted slots?
- **Prior commitments.** The `E23--accessibility` boundary inventory and U3 brief form still name a top-level `[bindings]` table, and `player_options.md` §6 says mods do not extend the action set. The U3 seed (`drafts/E23--gamepad-input`) supersedes both: authors set default bindings, and the U3 brief amends the epic index and §6. This seed reconciles only the per-game section layout.

## Grounding already gathered
- `done/hidpi-render-scale/research.md` §winit 0.30.13 platform notes covers fullscreen variants, video-mode enumeration, the macOS caveats and the transition queueing.
- **Settings path.** `options::settings_path` joins `settings.toml` to the config dir of `ProjectDirs::from("", "", "postretro")`. Every `PlayerOptions` field is a top-level key except the `[accessibility]` table.
- **Load order.** `Session::build` loads options first, post-first-pixel, before the `InputSystem`. Mod init runs later in the same install frame, after full renderer init, and commits the `ModManifest` (`boot_sequence.md` §1).
- **Default bindings.** Today `Session::build` seeds the `InputSystem` from `input::default_bindings()`, which is engine-global. The U3 seed adds an author layer applied at mod init.
- **Mod id.** `ModManifestResult.id` is required. A missing id, or one failing `validate_mod_manifest_id` (`[A-Za-z0-9_.-]{1,64}`, no `:`, not all dots), fails mod init. Nothing beyond the pattern is enforced. The id gates multiplayer admission, so peers running one game must share it; it is frozen at first commit across hot reload (`networking.md` §Mod identity). It is distinct from `--mod <dir>`, which names the content folder: the dev mod is folder `dev`, id `postretro.dev`.
- **TOML key hazard.** Ids may contain `.`, so an unquoted `[game.postretro.dev]` parses as nested tables. The key must be quoted: `[game."postretro.dev"]`.
- **Persisted mod state.** `state_persistence::state_path` writes `<data dir>/<mod_id>/state.json` under the same `ProjectDirs` name, keyed by the runtime's committed mod identity. The mod id is therefore already a per-user persistence key, and two mods sharing an id already share state. `<mod-root>/identity.json` (`store_identity`) is different: an author-owned shipped ledger mapping slot names to durable keys, not player data.
- **Precedent (model knowledge, not verified this session).** GZDoom `gzdoom.ini`: `[GlobalSettings]` vs `[Doom.Bindings]`/`[Doom.ConsoleVariables]`, scoped per base game, not per mod, so bindings leak across mods. Quake, ioquake3, Doom 3: config per mod dir, everything per mod; players redo video settings per mod. Source: per-gamedir `config.cfg` layered over `config_default.cfg`. Unreal: `GameUserSettings` vs shipped game config. Bethesda: `Skyrim.ini` vs `SkyrimPrefs.ini`.
