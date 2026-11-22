# CLI help and unknown-flag rejection — contract

## Goal

`postretro --help` boots the default map in a window: the engine has no parser, each subsystem scans argv for its own flag, and an unclaimed token is silently ignored. The engine gets a flag registry that owns every flag's name, value shape and help line; `--help` is generated from it, and an unregistered flag is rejected before any window. Two small binaries gain `--help`; the binaries that already print help gain a test that keeps their help and parser in step.

## Decisions

- **Unknown engine flags reject and exit.** Owner decision. A `-`-prefixed token the registry does not know prints `postretro: unknown flag `<tok>`; run `postretro --help` for usage` to stderr and exits 2. Consequence: the registry is the single gate. A flag parsed somewhere but not registered fails the first time anyone passes it, so registration cannot be skipped silently.
- **Help is generated from the registry.** Each entry carries its own help text. No hand-written flag list exists anywhere else in the engine.
- **The engine checks for help and unknown flags first.** In `main`, before `telemetry::start_engine_run`, the logger, the `--capture` / `--headless` branches and the event loop. Consequence: neither a help request nor a rejected flag writes telemetry, which matches `prl-build`'s existing rule (`level-compiler/src/main.rs`, comment above `telemetry::start`).
- **Feature-gated flags are registered in every build** and their help names the feature (`requires a build with the `dev-tools` feature`). The stock binary already detects them to explain how to launch those modes. Stock-build behaviour for a gated flag stays the same: warn, or exit with the existing message.
- **Single-parser binaries get a sync test, not a registry.** `prl-build`, `scripts-build`, `texture-tool` and `texture-tool`'s `contact` keep their parsers. A test asserts that every flag the parser matches appears in the help text, and every flag the help lists is one the parser matches.
- **Not in scope:** the engine's `POSTRETRO_*` environment variables, `--help` on `xtask` / `postretro-tool` subcommands, and `--version`. With rejection in place, `--version` exits 2 and opens no window.

## Invariants

- **Engine streams and exit codes.** Requested help goes to **stdout** and exits **0**. A usage error goes to **stderr** and exits **2**. Existing binaries keep their current streams: `prl-build`'s data-script test reads `scripts-build --help` from stderr.
- **What counts as a flag token.** Any argv token after `argv[0]` that starts with `-` and is not consumed as a preceding flag's value. `-h` and `--help` print help wherever they appear as flag tokens.
- **Value consumption matches today's parsers exactly.** A value-taking flag consumes the next token unless that token starts with `--`. `--host` and `--connect` also leave an empty next token unconsumed. A value starting with a single `-` (for example `--start-pose -1,2,3,0,0`) is a value, not a flag. The registry records each flag's value shape (none, required, optional), and both the unknown-flag check and `resolve_map_path` derive their skipping from it. If `resolve_map_path` keeps its own list, a test asserts that the list and the registry agree.
- **The `=` form is accepted only where the owning parser accepts it.** The registry records which flags accept `--flag=value`. The check rejects `--flag=value` for a flag whose parser ignores that form. It must never accept a token the subsystem then silently drops.
- **Positional tokens are unchanged.** The first non-flag token is the map; extra positionals keep today's behaviour.
- **Layering.** The registry lives in `crates/postretro` (the binary owns its command line). `netcode::parse_net_config` keeps parsing `--host` / `--connect` / `--password`; the engine registers them. No new crate edges (`layering_invariants_hold`).
- **`docs/` rule.** Any command added to `docs/` must run from an SDK bundle alone: `postretro --help`, never `cargo run -p xtask`.

## File ownership

- **Track A (agent, engine):** `crates/postretro/**` and the minimal matching change in `crates/netcode/src/lib.rs` if needed. The registry is a new module under `crates/postretro/src/startup/`. Plus a pointer in `docs/` wherever engine flags are documented for authors.
- **Track B (orchestrator):** `crates/sim/src/bin/gen_script_types.rs`, `crates/sim/src/bin/mint_identity.rs`, `crates/level-compiler/src/main.rs` (test only), `crates/script-compiler/src/main.rs`, `tools/texture-tool/src/{main.rs,bin/contact.rs}`.
- **Orchestrator:** this contract and `context/lib/`.

## Acceptance

Track A:
- `cargo run -p postretro -- --help` prints usage and the grouped flag list to stdout, exits 0, and opens no window; the same for `-h` and for `--mod dev --help`.
- `cargo run -p postretro -- --hepl` and `-- --version` print the unknown-flag message to stderr and exit 2, with no window and no telemetry run file written.
- A unit test fails when a `"--flag"` literal in the engine's argv-reading source (non-test code, including `parse_net_config`) has no registry entry. The file list is explicit in the test. Literals that are arguments passed to other programs (for example `prl-build` arguments in `candidate_cull_probes.rs`, `cargo` arguments) are excluded by name, with the reason.
- A unit test proves that every registered value-taking flag's value is skipped by `resolve_map_path` and by the unknown-flag check.
- `cargo test -p postretro --lib <filters>`: the existing `resolve_map_path*`, net-flag and camera-route-flag tests still pass, with a nonzero test count reported.

Track B:
- `gen-script-types --help` prints usage, exits 0 and writes nothing. An unknown argument exits 2.
- `mint-identity --help` prints usage and exits 0. A `-`-prefixed argument other than `-h` / `--help` exits with a usage error and is never treated as a mod root.
- The sync tests pass in `postretro-level-compiler` (`--bin prl-build`), `postretro-script-compiler` and `texture-tool`, and each fails when a parser flag is removed from the help.

## Open

- None.
