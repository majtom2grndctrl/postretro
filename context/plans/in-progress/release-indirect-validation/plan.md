# release-indirect-validation — plan of record

mode: compact
status: active
read at: c443c91ef

## Corrections
- Upload batching is merged in `0f6f52810`; its changes to cited renderer symbols only replace queue plumbing. Re-read the instance constructors, cull owners, install path, loader check, shader writers, and their consumers; the Decisions remain valid.
- Boot creates a dummy world index buffer with `geometry = None`; its cull constructors are unreachable. The lifetime/index-buffer scans will guard this explicit boot exception as well as the install mapping. No live indirect record precedes a level install.
- `install_level_payload` changes network parity and navigation before texture installation. Validate the world's unchanged BVH/index slices at entry, before those changes; validate capture before renderer construction or setters. UV normalization changes neither slice.

## Delegated answers
- Install error — renderer-owned typed range error containing the leaf, offset, count and index-buffer length; callers convert it to a failed load. Keep the public geometry installer signature and its debug assertion.
- Manual proof blocks landing: the brief does not permit landing first. Preserve outstanding results in `test-ready` and wait for external proof.

## AC-to-proof

Rows follow the brief's order within Automated and Manual.

| AC | Proof | Status |
|---|---|---|
| A1 release default, both feature sets | policy matrix tests in release + source gate scan | achievable as stated |
| A2 debug default, both feature sets | policy matrix tests in debug + source gate scan | achievable as stated |
| A3 release override 1 | policy matrix | achievable as stated |
| A4 debug override 0 | policy matrix | achievable as stated |
| A5 empty/false override | policy matrix | achievable as stated |
| A6 preserve other bits/env isolation | policy matrix + env source scan | achievable as stated |
| A7 shared constructors/read once | production instance/source scan | achievable as stated |
| A8 aligned load past-end | loader regression with named range message | achievable as stated |
| A9 load exact boundary | loader fixture | achievable as stated |
| A10 load overflow | loader regression | achievable as stated |
| A11 failed install load routes, no side effects | app rejection regression + failure routing/order scan | achievable as stated |
| A12 install past-end, no GPU mutation | pure check + ignored offscreen rejection/state test | achievable as stated |
| A13 install exact boundary/overflow | pure check | achievable as stated |
| A14 install zero leaves | pure check | achievable as stated |
| A15 empty leaf offset boundary, both checks | loader fixtures + pure check | achievable as stated |
| A16 writer ownership and stores | source scans + deliberately invalid scan fixtures | achievable as stated |
| A17 built-ins/composed modules/dispatch | pipeline/source scans | achievable as stated |
| A18 indirect buffer lifetime | install/constructor source scan | achievable as stated |
| A19 whole world index buffer | world draw/upload source scan | achievable as stated |
| M1 submit savings on both maps | paired same-binary release runs, 5+ windows, idle VRAM and cache state | manual-performance |
| M2 sample frames absent/present | symbol-preserving release/debug sample profiles on both maps | manual-performance |
| M3 startup logs including dev-tools | windowed startup logs for build/override matrix | manual-runtime |
| M4 visual parity | owner, spawn and short walk on both maps | manual-visual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin instance-policy slice; build release and attempt hallway paired timing before adding tests | integrating executor | — | done — symbol-preserving release build; paired 35-second hallway launch reached Metal and logged on/off once, but no in-level frames/timing windows. M1 remains outstanding. |
| 2 | Load regressions, pure install check, early failed-load routing and offscreen rejection proof | integrating executor | 1 | done — 10 loader range tests, 3 pure install tests, 2 app routing/state tests; offscreen GPU rejection test ran (1 matched, no skip) and preserved buffers/counts/uploads. |
| 3 | Policy tests, invariant comments, source drift guards and invalid scan fixtures | integrating executor | 1, 2 | done — policy test passed in all four debug/release × plain/dev-tools builds; all 10 contract scans/negative fixtures pass. |
| 4 | Focused readiness gate, review/fix loop, final preflight; record all results and external runbook | integrating executor | 2, 3 | pending |

## Runtime constraints
- Instance policy reads once at creation and adds no frame work. Range checks run O(leaves) only at install; existing cull and draw hot paths remain unchanged.
- Cargo runs are owned by the integrating executor and sequential. Check free disk space after each task/numbered step; only caches may be cleared below 15 GB.

## Exploratory run
- Release binary: `CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro` passed. Upload batching is already merged.
- Tried paired 35-second hallway runs, validation `1` then `0`; both reached AMD Radeon Pro 5300M / Metal and emitted exactly one effective-state log. Both stopped at `Window ready` without in-level frames, so no submit savings or cache-state claim is made. Foreground runtime proof remains external.
- Session artifacts: `/private/tmp/release-indirect-validation/` (idle accelerator snapshot and exploratory logs). The initial sandboxed attempt could not connect to macOS window services; the unsandboxed retry initialized the adapter successfully.

## External proof runbook (blocking)
1. Build from this branch after the final gates: `CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro -p postretro-script-compiler`. Keep `dev-tools` off for M1/M2. The same binary is both baseline and treatment; the sibling `scripts-build` helper is required by the engine launch.
2. Close the engine, record `ioreg -c IOAccelerator` idle VRAM/free VRAM and utilization. Keep machine state consistent across each back-to-back pair. Use map spawn, unchanged player options/resolution/vsync, and keep the engine window in front throughout. Record warm/cold shadow cache state and uncached spot/cube-face world passes; use a labelled Metal System Trace if the plain build has no cache counter surface. The count is needed to reconcile validation-only cost (leaf count × camera/shadow indirect draws); do not infer it from leaf count alone.
3. For each of `content/dev/maps/stress-warren-hallway-inspection.prl` and `content/dev/maps/campaign-test.prl`, run `RUST_LOG=info POSTRETRO_CPU_TIMING=1 WGPU_VALIDATION_INDIRECT_CALL=1 target/release/postretro <map>` and then `RUST_LOG=info POSTRETRO_CPU_TIMING=1 target/release/postretro <map>` with the override unset. Save logs, discard boot/warmup, and collect at least five complete `[CpuTiming]` windows in each run under the same cache state. Report the median `render_submit` window averages, their delta, the reconciled validation-only cost and half-cost threshold (provisional: 0.65 ms hallway, 0.18 ms campaign).
4. While each release run is rendering in front, take `sample <pid> 10 -file <profile>`. The unset release profile must contain neither `DrawBatcher::add` nor `inject_validation_pass`; the same binary with override `1` must contain both. Build `cargo build -p postretro -p postretro-script-compiler`, repeat both maps in debug with the override unset, and verify both frames remain. Record the sampled in-level interval; absence in a profile with no rendered frames proves nothing.
5. Record startup lines once per run for unset plain release (off), unset `cargo build --release -p postretro --features dev-tools` (off), unset debug (on), and explicit overrides (line names override). Rebuild the plain symbol-preserving release binary if the dev-tools build replaced it.
6. Compare the same release binary with override `1` versus unset on both maps at spawn and during a short walk. Record owner visual verdict for M4.
7. Return M1–M4 results. Landing remains blocked until these pass, then the owner says “land the plane.”
