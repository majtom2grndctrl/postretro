# game-user-dirs

Brief · compact · reads: `context/lib/build_pipeline.md` §Distribution packaging, `context/lib/boot_sequence.md` §1, `context/lib/player_options.md` §2 · read at 67fddd0af

## Problem
Anticipated need, raised by the developer. Every game built on the engine writes its player's settings and saved state under one `postretro` directory, because both per-user directory sites hardcode that name. A shipped standalone game therefore files player data under a name the player never installed, and two games on one machine share machine settings and device identity. When done, a `dist` payload keeps its player's settings and state under the game's own package name; dev and SDK runs keep `postretro`.

## Decisions
- **The launcher names the directory.** `dist`'s launcher passes `--app-name <package name>` beside `--mod`. A plain flag keeps the commitment that the engine learns nothing about `postretro.toml` (`build_pipeline.md` §Distribution packaging). Without the launcher the engine falls back to the dev map, so a bare launch already fails for any mod but `dev`; a bare launch of a `dev` payload works but writes under `postretro`, an accepted cost of the flag.
- **Not derived from `--mod`.** `build_pipeline.md` §Distribution packaging recommends `base` as every downstream game's mod name, so the mod name recreates the collision. The mod id arrives at mod init, too late for the pre-window settings read.
- **Resolved before the window, with the mod.** The app name resolves at boot stage 1 alongside `--mod` (`boot_sequence.md` §1), so the pre-window settings read in `drafts/window-modes` can use it. Absent → `postretro`. An invalid value is a boot error under the same rule as `--mod`: empty, holding `/`, `\` or `:`, `.`/`..`, or a leading `-`.
- **One directory chokepoint.** The config and data directories resolve in one place, from the app name; `settings.toml` and `state.json` both route through it. One resolver, not two, because `state.json`'s per-player rows are keyed by the claim id derived from `settings.toml`'s `player_id` (`player_options.md` §2): split names orphan every per-player save. The sim crate receives resolved directories and never parses arguments.
- **The package name is the player-data identity.** From a game's first release, renaming `[package].name` drops its players' settings, `player_id` and saves. The landing docs say so. `dist` refuses a package name the engine's `--app-name` rule rejects, so a payload never ships a launcher that fails at boot.
- **"App name", not "identity".** `identity.json` and `mint-identity` already name the store-slot key ledger (`store_identity`).
- **Non-goals.**
  - `postretro-tool run` and `cargo run -p xtask -- run` pass no flag: authoring runs share `postretro`, where U3's `[game."<mod_id>"]` layer keeps bindings apart.
  - Migrating data from `postretro` into a game's new directory: no game has shipped.
  - A macOS app bundle's identity: `dist` ships no bundle.
  - An `app_name` manifest override decoupling the directory from the package name: the escape hatch for a rename, added when a game needs one.

## Acceptance
### Automated
- [ ] `--app-name` with a valid name resolves both directories under it; no flag resolves both under `postretro`.
- [ ] Each invalid form above is a boot error naming the flag; a name with `.`, `-` or `_` inside it is accepted.
- [ ] The Unix and Windows launchers pass `--app-name` with the package name, quoted the way they quote `--mod` (a name holding `'` or `%` survives).
- [ ] `dist` refuses a package name with a leading `-`; a name `--app-name` accepts assembles.
- [ ] Grep gate: no `ProjectDirs::from` outside the chokepoint.
- [ ] Grep gate: the default `postretro` app-name literal appears only in the chokepoint; the settings load, the first-launch and `player_id` save, and every `state.json` restore and save take the directories resolved once at stage 1 (P2).
- [ ] One table of app-name cases — each invalid form above, and names with `.`, `-` or `_` inside — is asserted against both the engine's `--app-name` check and `dist`'s package-name check, with the same verdict per case.
- [ ] A bare `--app-name`, `--app-name=`, and `--app-name` followed by another flag are boot errors naming the flag, never the `postretro` fallback (P5).
- [ ] `--app-name` given twice resolves under the first occurrence, as `--mod` does (P3).
- [ ] `--mod <m> --app-name <name>` with no map boots the frontend with no positional map; `--app-name <name> maps/x.prl` loads `maps/x.prl` (P4).
- [ ] The SDK bundle's launcher passes no `--app-name`; a bundle launch writes under `postretro`, as `bin/postretro-tool run` does (P6).
- [ ] A first launch under a new app name, with `postretro/` holding settings and state, writes defaults and a fresh `player_id` to the new directory and leaves every file under `postretro/` byte-identical (P1).
### Manual
- [ ] A `dist` payload run through its launcher writes `settings.toml` and `state.json` under the package name; `cargo run -p xtask -- run` still writes under `postretro`.

## Path
- Arguments: `mod_arg` / `resolve_content_root` (`startup/session.rs`) for stage-1 flag parsing and its boot errors.
- Directories: `options::settings_path` (engine crate) and `state_persistence::state_path` (sim crate). Thread the resolved directories in; `Session::build` and the state store are the consumers.
- Launcher: `launcher_contents` (`dist/launcher.rs`) and its quoting helpers; the package name is already validated as one normal path component by `Manifest::parse`.
- `validate_package` (`crates/tool/src/manifest.rs`) checks the leading `-` only for the mod name today.
- Docs at landing: `build_pipeline.md` §Distribution packaging (payload launch line, rename hazard), `player_options.md` §2 (settings location), and the author-facing `docs/` page on shipping.

## Open questions
None.
