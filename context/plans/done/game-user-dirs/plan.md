# game-user-dirs — plan of record

mode: compact
status: landed
read at: 423aae2a5

## Corrections
- Path names `options::settings_path` and `state_persistence::state_path` as the two resolvers → confirmed; `state_path(mod_id)` has three engine consumers (`main.rs` per-owner save and clean-exit save, `startup/splash_lifecycle.rs` restore). Planning around it by making sim's `state_path` take the data directory (the existing private `state_path_from_data_dir` becomes the public shape) and dropping sim's `directories` dependency, so `ProjectDirs::from` lives only in the engine chokepoint.
- `--app-name` value parsing → reuses `path_flag_value` / `names_flag` by joining `PATH_FLAGS` (the flag names a per-user directory by name, as `--mod` names a mod directory); this also gives the map-path scan its step-over (P4) and first-occurrence-wins (P3) for free.

- Decision "invalid … `.`/`..`" → `directories` on Linux strips all whitespace before naming the directory, so `" .."` or `". ."` became `..` (writing settings to `$HOME`). Clarified, same meaning: the blank / `.` / `..` checks apply to the whitespace-stripped name, in both the engine and the tool; the shared table carries the padded cases. (review panel, 3 lenses)
- `path_flag_value` let an empty `--flag=` fall through to a later occurrence (`--mod= --mod dev` booted `dev`), contradicting first-occurrence-wins and the "`--flag=` is refused" docs on `--mod` and `--app-name`. It now settles on the first occurrence; `--mod` inherits the fix.

## Delegated answers
- Where the shared app-name case table lives — beside the engine rule, `crates/postretro/src/startup/app_name_cases.toml`, read by the tool test through `include_str!`. The engine owns the rule the tool mirrors; a test-only relative `include_str!` is not the compile-time path macro the tool's shippability test forbids.
- How AC "Unix and Windows launchers" is proven on one host — both launcher renderers compile on every host (the `cfg` picks which one `emit_launcher` writes), so both quoting paths are tested on macOS.
- How P1 is proven without touching the real home directory — the resolver test proves distinct app names resolve to disjoint directories; the first-launch test drives `load_player_options` (the function `Session::build` calls) and the state-store path against temp directories standing in for both resolutions.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 `--app-name` resolves to `ProjectDirs::from`'s dirs; absent → `postretro` | `app_name_resolves_both_directories_through_project_dirs`, `absent_app_name_resolves_both_directories_under_postretro`, `app_name_flag_resolves_the_named_directories_in_both_forms` | automated | pass |
| 2 invalid forms are boot errors naming the flag; `.`/`-`/`_` inside accepted | `an_invalid_app_name_is_refused_naming_the_flag`, `app_name_check_matches_the_shared_case_table` | automated | pass |
| 3 Unix + Windows launchers pass `--app-name`, quoted like `--mod` (`'`, `%` survive) | `launcher_pins_its_own_directory_and_mounts_the_published_mod_under_the_package`, `apostrophes_survive_the_posix_shell_in_both_values`, `percent_signs_survive_batch_expansion_in_both_values` (both shells render on every host) | automated | pass |
| 4 `dist` refuses leading `-` package; an accepted name assembles | `dist_refuses_a_flag_shaped_package_name_when_opening_the_project`, `manifest_refuses_a_flag_shaped_package_name_and_accepts_punctuation_inside` | automated | pass |
| 5 grep gate: no `ProjectDirs::from` outside the chokepoint | `no_crate_resolves_project_dirs_outside_the_chokepoint` | automated | pass |
| 6 grep gate: default literal only in chokepoint; settings/state consumers take stage-1 dirs | `the_default_app_name_literal_appears_only_in_the_chokepoint`; zero-arg `settings_path()` / `state_path(mod_id)` removed (compile-enforced) | automated | pass |
| 7 one case table, same verdict from engine and tool checks | `app_name_check_matches_the_shared_case_table` (engine), `package_name_check_matches_the_engines_app_name_verdicts` (tool) over `app_name_cases.toml` | automated | pass |
| 8 bare / `=` / followed-by-flag → boot error naming the flag (P5) | `an_app_name_flag_without_a_name_is_refused` | automated | pass |
| 9 given twice → first occurrence (P3) | `app_name_given_twice_resolves_under_the_first` | automated | pass |
| 10 `--mod m --app-name n` → no map; `--app-name n maps/x.prl` → map (P4) | `app_name_value_is_not_mistaken_for_the_map_path`, `resolve_map_path_skips_every_directory_naming_flag` | automated | pass |
| 11 `postretro-tool run` passes `--app-name` unless caller named it; `xtask run` passes none | `launch_supplies_the_mod_the_app_name_the_baked_root_and_the_core_root`, `an_explicit_flag_wins_rather_than_being_shadowed`; xtask `split_run_args_*` (engine args forwarded verbatim) | automated | pass |
| 12 SDK marker names `<package>-sdk`; bundle launcher passes it (P6) | `bundle_launcher_and_marker_both_name_the_sdk_package`, `bundle_manifest_publishes_the_projects_own_mod` | automated | pass |
| 13 first launch under new name writes defaults + fresh `player_id` there, `postretro/` byte-identical (P1) | `first_launch_under_a_new_app_name_leaves_the_postretro_directory_untouched`, `a_game_app_name_shares_no_directory_with_postretro` | automated | pass |
| M1 payload launcher, `postretro-tool run`, SDK launcher, `xtask run` write under the right directory | owner, on-machine (the `[Engine] Player data:` log line names the directories) | manual | outstanding |

Gate: `cargo test -p postretro -p postretro-sim -p postretro-tool -p xtask` — 1153 + 1244 + 157 + 19 + 7 + 3 passed, 0 failed. `cargo clippy -p postretro -p postretro-sim --all-targets` clean in touched code (4 pre-existing `chunks_exact_to_as_chunks` warnings in `light_bridge.rs` / `particle_render.rs`, untouched). `/preflight` (owner-invoked): fmt ✓, clippy `-D warnings` ✓, `cargo test` ✓ (9283 passed, 0 failed), `cargo check --release` ✓, crate-graph ✓.

## Review loop
- Panel 1 (tracer, contract verifier, adversarial — opus; hygiene/drift — sonnet): 1 🟡 found by 3 lenses (padded `.`/`..` names), 3 🟡 docs, ~6 🟢. All mechanical findings fixed in `3d40871fd`.
- Owner ruled out of scope (2026-10-04: PostRetro is the engine, not a game name, so a package case-folding to `postretro` is not a realistic scenario); left documented, not refused: names that case-fold to `postretro` (e.g. `PostRetro`) share the bare-launch directory; Windows-reserved names and characters (`CON`, `<>|?*`, trailing `.`) are accepted. Both are now documented in `docs/distribution.md`.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Engine chokepoint: `startup/app_dirs.rs`, `--app-name` stage-1 parse, thread dirs through `PendingSessionInit` → `Session`, sim `state_path(data_dir, mod)`, gates, P1 test | integrating executor | — | done — `app_dirs` 6, session/arg/P1 22, sim 1 passing |
| 2 | Shared case table | integrating executor | — | done |
| 3 | Tool: package-name rule, launcher `--app-name` (both renderers), `run` prepend, SDK marker `<package>-sdk` + README | worker | 2 | done — `postretro-tool` 157/157, clippy clean |
| 4 | Docs: `docs/distribution.md`, `docs/modding.md` now; `build_pipeline.md`, `boot_sequence.md`, `player_options.md` at land-the-plane | integrating executor | 1, 3 | `docs/` done; context/lib pending landing |
