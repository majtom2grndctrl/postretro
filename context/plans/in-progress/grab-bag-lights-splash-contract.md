# Grab bag: stale level scripts, fog glow rename, splash logo size

## Goal

campaign-test's animated lights stayed fully on after the addressing-model SDK change. The script source was already migrated; the `.prl` embeds a compiled copy of its level script from bake time, and that copy still called the removed `world.query`. `setupLevel` threw, so no light reaction installed. Fix the local maps, make the failure mode loud, clear the one unrelated dead reaction in the same script, and resize the splash logo.

## Decisions

1. **Stale maps are rebaked, not patched.** The PRL's embedded script is the runtime contract; the engine never reads the source at load. Small dev maps rebake in this session (local artifacts, gitignored). The stress-warren variants are left to the owner's terminal — long bakes.
2. **Load-time staleness error, mtime-based.** At level load, when the embedded script's `source_path` exists on disk and is newer than the `.prl`, log an error naming the map and the source and saying to rebuild the map. Consequence: catches edits to the entry script (this case), not SDK-only or imported-module changes; a git checkout that touches the source also trips it, which is acceptable for a rebuild hint. Missing source (shipped builds, other machines) is silent. No PRL format change.
3. **`setFogScatter` → `setFogGlow`, arg `scatter` → `glow`.** The engine registers only `setFogGlow` and its args have no alias; the generated TS typedefs and `context/lib` already say glow. Every remaining spelling (dev scripts, Luau SDK, TS SDK re-exports, author docs) moves. No compatibility alias.
4. **Splash logo: 50% of a fitted 16:9 frame.** Logo width = 0.5 × min(window width, window height × 16/9), height cap unchanged. 16:9 → half the window width; ultrawide → same size as a 16:9 window of that height; 4:3 → half the window width.

## Invariants

- No PRL section or format change.
- The staleness check lives in `postretro` startup (it has the map path); not in `prl_loader.rs` (already ~3000 lines).
- Logo keeps source aspect and stays centered; degenerate viewport still yields a zero rect.

## Acceptance

- `cargo test -p postretro-renderer --lib splash_pass` — all pass, count > 0.
- `cargo test -p postretro --lib data_script_staleness` — all pass, count > 0.
- `rg -n "setFogScatter|SetFogScatter|\bscatter: 0" content sdk docs crates --glob '!**/target/**'` — empty.
- Typedef freshness tests in `postretro-sim` pass.
- campaign-test run log: no `setupLevel threw`, no `not registered` line. Visual: arena lights pulse (owner).

## Open

- An SDK API epoch stamped into the DataScript section would also catch SDK-only breaks. Not in this change.
