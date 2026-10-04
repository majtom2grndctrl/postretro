# game-user-dirs

Brief · compact · reads: `context/lib/build_pipeline.md` §Distribution packaging, `context/lib/boot_sequence.md` §1, `context/lib/player_options.md` §2 · read at 2da2aa59d

## Problem
Anticipated need, raised by the developer. Every game built on the engine writes its player's settings and saved state under one `postretro` directory, because both per-user directory sites hardcode that name. A shipped standalone game therefore files player data under a name the player never installed, and two games on one machine share machine settings and device identity. When done, every launch that knows its project — a `dist` payload's launcher, `postretro-tool run`, an SDK bundle's launcher — keeps its player's settings and state under that project's package name, and an SDK bundle is its own project, `<package>-sdk`. A bare engine launch keeps `postretro`.

## Decisions
- **Whoever knows the project names the directory.** The `dist` launcher, `postretro-tool run` and the SDK bundle's launcher pass `--app-name <package name>`, as they pass `--mod`. A plain flag keeps the commitment that the engine learns nothing about `postretro.toml` (`build_pipeline.md` §Distribution packaging). Authoring runs write where the shipped game writes, so an author sees their players' first launch and saves — the Godot and Unity editors behave the same way. `cargo run -p xtask -- run` passes no flag, as it passes no `--mod`: it is a bare engine launch. In this workspace, whose package is `postretro-dev`, `xtask run` and `postretro-tool run` therefore keep separate settings and `player_id`s; accepted.
- **An SDK bundle is its own project.** `sdk-dist` writes the bundle's `postretro.toml` with the package name `<package>-sdk`, the bundle's folder name, so its launcher, its `postretro-tool run` and any `dist` built from it write under that name. A modder's test runs never touch the installed game's saves, and a derivative shipped from the bundle cannot collide with the original by default. Settings do not carry over from the installed game; accepted.
- **A bare launch writes under `postretro`.** Run directly, the engine falls back to the dev mod root; for any payload but a `dev` one it then fails, but only after `Session::build` has written `postretro/settings.toml` and a fresh `player_id`. Accepted: the launcher exists to prevent that path, and a game-named executable (non-goals) removes it.
- **Not derived from `--mod`.** `build_pipeline.md` §Distribution packaging recommends `base` as every downstream game's mod name, so the mod name recreates the collision. The mod id arrives at mod init, too late for the pre-window settings read.
- **Resolved before the window.** The app name resolves at boot stage 1 (`boot_sequence.md` §1), so the pre-window settings read in `ready/window-modes` can use it. Absent → `postretro`. An invalid value is a boot error: empty, whitespace only, holding `/`, `\` or `:`, `.`/`..`, or a leading `-`.
- **Platform-native directory names.** The app name maps through `ProjectDirs::from`, as `postretro` does today, so existing dev data stays put. Each platform normalizes the name its own way, and the docs state the result per platform. The mapping is one-way per shipped game; no game has shipped.
- **One directory chokepoint.** The config and data directories resolve in one place, from the app name; `settings.toml` and `state.json` both route through it. One resolver, not two, because `state.json`'s per-player rows are keyed by the claim id derived from `settings.toml`'s `player_id` (`player_options.md` §2): split names orphan every per-player save.
- **The package name is the player-data identity.** From a game's first release, renaming `[package].name` drops its players' settings, `player_id` and saves. `dist` and `sdk-dist` refuse a package name the engine's `--app-name` rule rejects, so no launcher ships that fails at boot.
- **Non-goals.**
  - Migrating data from `postretro` into a game's new directory: no game has shipped.
  - A macOS app bundle's identity, and the bundle-id directory convention it would bring: `dist` ships no bundle.
  - An `app_name` manifest override decoupling the directory from the package name: the escape hatch for a rename, added when a game needs one.
  - Carrying launch identity in the payload itself — one descriptor read before the window holding the app name and the mod, or a game-named executable — so a store or a shortcut can launch without the launcher. Deferred, not rejected: a marker holding only the app name still needs the launcher for `--mod`. When the payload carries both, the value's source moves and the resolver does not change.

## Acceptance
### Automated
- [ ] `--app-name` with a valid name resolves both directories to `ProjectDirs::from`'s directories for it; no flag resolves both under `postretro`.
- [ ] Each invalid form above is a boot error naming the flag; a name with `.`, `-` or `_` inside it is accepted.
- [ ] The Unix and Windows launchers pass `--app-name` with the package name, quoted the way they quote `--mod` (a name holding `'` or `%` survives).
- [ ] `dist` refuses a package name with a leading `-`; a name `--app-name` accepts assembles.
- [ ] Grep gate: no `ProjectDirs::from` outside the chokepoint.
- [ ] Grep gate: the default `postretro` app-name literal appears only in the chokepoint; the settings load, the first-launch and `player_id` save, and every `state.json` restore and save take the directories resolved once at stage 1 (P2).
- [ ] One table of app-name cases — each invalid form above, and names with `.`, `-` or `_` inside — is asserted against both the engine's `--app-name` check and the tool's package-name check, with the same verdict per case.
- [ ] A bare `--app-name`, `--app-name=`, and `--app-name` followed by another flag are boot errors naming the flag, never the `postretro` fallback (P5).
- [ ] `--app-name` given twice resolves under the first occurrence, as `--mod` does (P3).
- [ ] `--mod <m> --app-name <name>` with no map boots the frontend with no positional map; `--app-name <name> maps/x.prl` loads `maps/x.prl` (P4).
- [ ] `postretro-tool run` passes `--app-name` with the package name unless the caller already named the flag, as it does `--mod`; `cargo run -p xtask -- run` passes none.
- [ ] The SDK bundle's generated `postretro.toml` names the package `<package>-sdk`, and the bundle's launcher passes `--app-name <package>-sdk` (P6).
- [ ] A first launch under a new app name, with `postretro/` holding settings and state, writes defaults and a fresh `player_id` to the new directory and leaves every file under `postretro/` byte-identical (P1).
### Manual
- [ ] A `dist` payload run through its launcher and `postretro-tool run --install-root .` both write `settings.toml` and `state.json` under the package name's directory; an SDK bundle's launcher writes under `<package>-sdk`; `cargo run -p xtask -- run` still writes under `postretro`.

## Path
- Arguments: `mod_arg` / `resolve_content_root` (`startup/session.rs`) for stage-1 flag parsing and its boot errors; `--app-name` must join the value-taking flags the map-path scan steps over.
- Directories: `options::settings_path` (engine crate) and `state_persistence::state_path` (sim crate). Thread the resolved directories in; `Session::build` and the state store are the consumers. The sim crate receives directories and parses no arguments; a private `state_path_from_data_dir` already exists.
- Launchers: `launcher_contents` (`dist/launcher.rs`) and its quoting helpers. `sdk_dist/assemble.rs` calls the shared `emit_launcher` with the bundle name from `sdk_bundle_root_name`; the bundle's marker (`sdk_dist/readme.rs` `render`) copies the source package name today.
- Authoring: `engine_arguments` (`crates/tool/src/run.rs`) prepends `--mod` unless the caller named it; `--app-name` takes the same rule.
- Validation: the package name is already one normal path component (`Manifest::parse`); `validate_package` (`crates/tool/src/manifest.rs`) checks the leading `-` only for the mod name today.
- Naming: call it the app name, not identity — `identity.json` and `mint-identity` already name the store-slot key ledger (`store_identity`).
- Docs at landing: `build_pipeline.md` §Distribution packaging (payload launch line, `postretro-tool run` flags, rename hazard); `player_options.md` §2 (settings location, and `player_id` becomes per game rather than per device); `docs/distribution.md` (the `%APPDATA%\postretro` settings line), `docs/modding.md` (where an SDK bundle keeps its data, and renaming before shipping a derivative), and the author-facing `docs/` page on shipping.

## Open questions
None.
