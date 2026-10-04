# game-user-dirs — plan of record

mode: compact
status: active
read at: 423aae2a5

## Corrections
- Path names `options::settings_path` and `state_persistence::state_path` as the two resolvers → confirmed; `state_path(mod_id)` has three engine consumers (`main.rs` per-owner save and clean-exit save, `startup/splash_lifecycle.rs` restore). Planning around it by making sim's `state_path` take the data directory (the existing private `state_path_from_data_dir` becomes the public shape) and dropping sim's `directories` dependency, so `ProjectDirs::from` lives only in the engine chokepoint.
- `--app-name` value parsing → reuses `path_flag_value` / `names_flag` by joining `PATH_FLAGS` (the flag names a per-user directory by name, as `--mod` names a mod directory); this also gives the map-path scan its step-over (P4) and first-occurrence-wins (P3) for free.

## Delegated answers
- Where the shared app-name case table lives — beside the engine rule, `crates/postretro/src/startup/app_name_cases.toml`, read by the tool test through `include_str!`. The engine owns the rule the tool mirrors; a test-only relative `include_str!` is not the compile-time path macro the tool's shippability test forbids.
- How AC "Unix and Windows launchers" is proven on one host — both launcher renderers compile on every host (the `cfg` picks which one `emit_launcher` writes), so both quoting paths are tested on macOS.
- How P1 is proven without touching the real home directory — the resolver test proves distinct app names resolve to disjoint directories; the first-launch test drives `load_player_options` (the function `Session::build` calls) and the state-store path against temp directories standing in for both resolutions.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 `--app-name` resolves to `ProjectDirs::from`'s dirs; absent → `postretro` | `app_dirs` resolver tests | achievable as stated |
| 2 invalid forms are boot errors naming the flag; `.`/`-`/`_` inside accepted | `app_dirs` / `session` arg tests over the shared table | achievable as stated |
| 3 Unix + Windows launchers pass `--app-name`, quoted like `--mod` (`'`, `%` survive) | `dist::launcher` tests (both renderers) | achievable as stated |
| 4 `dist` refuses leading `-` package; an accepted name assembles | `manifest` tests (dist reads the manifest first) | achievable as stated |
| 5 grep gate: no `ProjectDirs::from` outside the chokepoint | source-scan test | achievable as stated |
| 6 grep gate: default literal only in chokepoint; settings/state consumers take stage-1 dirs | source-scan test + signatures (no zero-arg resolver remains) | achievable as stated |
| 7 one case table, same verdict from engine and tool checks | table-driven tests in both crates | achievable as stated |
| 8 bare / `=` / followed-by-flag → boot error naming the flag (P5) | `session` arg tests | achievable as stated |
| 9 given twice → first occurrence (P3) | `session` arg test | achievable as stated |
| 10 `--mod m --app-name n` → no map; `--app-name n maps/x.prl` → map (P4) | `resolve_map_path` tests | achievable as stated |
| 11 `postretro-tool run` passes `--app-name` unless caller named it; `xtask run` passes none | `run::engine_arguments` tests; xtask `split_run_args` tests | achievable as stated |
| 12 SDK marker names `<package>-sdk`; bundle launcher passes it (P6) | `sdk_dist::readme` marker test + launcher test on the bundle name | achievable as stated |
| 13 first launch under new name writes defaults + fresh `player_id` there, `postretro/` byte-identical (P1) | session first-launch test over temp dirs | achievable as stated |
| M1 payload launcher, `postretro-tool run`, SDK launcher, `xtask run` write under the right directory | owner, on-machine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Engine chokepoint: `startup/app_dirs.rs`, `--app-name` stage-1 parse, thread dirs through `PendingSessionInit` → `Session`, sim `state_path(data_dir, mod)`, gates, P1 test | integrating executor | — | done — `app_dirs` 6, session/arg/P1 22, sim 1 passing |
| 2 | Shared case table | integrating executor | — | done |
| 3 | Tool: package-name rule, launcher `--app-name` (both renderers), `run` prepend, SDK marker `<package>-sdk` + README | worker | 2 | done — `postretro-tool` 157/157, clippy clean |
| 4 | Docs: `docs/distribution.md`, `docs/modding.md` now; `build_pipeline.md`, `boot_sequence.md`, `player_options.md` at land-the-plane | integrating executor | 1, 3 | `docs/` done; context/lib pending landing |
