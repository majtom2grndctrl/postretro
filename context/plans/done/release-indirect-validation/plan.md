# release-indirect-validation — plan of record

mode: compact
status: done
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

| AC | Proof | Status | Result |
|---|---|---|---|
| A1 release default, both feature sets | policy matrix tests in release + source gate scan | achievable as stated | pass — plain/dev-tools release tests and gate scan |
| A2 debug default, both feature sets | policy matrix tests in debug + source gate scan | achievable as stated | pass — plain/dev-tools debug tests |
| A3 release override 1 | policy matrix | achievable as stated | pass |
| A4 debug override 0 | policy matrix | achievable as stated | pass |
| A5 empty/false override | policy matrix | achievable as stated | pass |
| A6 preserve other bits/env isolation | policy matrix + env source scan | achievable as stated | pass |
| A7 shared constructors/read once | production instance/source scan | achievable as stated | pass |
| A8 aligned load past-end | loader regression with named range message | achievable as stated | pass |
| A9 load exact boundary | loader fixture | achievable as stated | pass |
| A10 load overflow | loader regression | achievable as stated | pass |
| A11 failed install load routes, no side effects | app rejection regression + failure routing/order scan | achievable as stated | pass — 2 app tests |
| A12 install past-end, no GPU mutation | pure check + ignored offscreen rejection/state test | achievable as stated | pass — GPU test explicitly ran, 1 matched, no skip |
| A13 install exact boundary/overflow | pure check | achievable as stated | pass |
| A14 install zero leaves | pure check | achievable as stated | pass |
| A15 empty leaf offset boundary, both checks | loader fixtures + pure check | achievable as stated | pass |
| A16 writer ownership and stores | source scans + deliberately invalid scan fixtures | achievable as stated | pass — includes renamed storage/alias review regression |
| A17 built-ins/composed modules/dispatch | pipeline/source scans | achievable as stated | pass |
| A18 indirect buffer lifetime | install/constructor source scan | achievable as stated | pass |
| A19 whole world index buffer | world draw/upload source scan | achievable as stated | pass — includes nested binding review regression |
| M1 submit savings on both maps | paired same-binary release runs, 5+ windows, idle VRAM and cache state | manual-performance | pass — foreground unsampled medians: hallway 5.094 → 3.811 ms (Δ1.283 ≥ half reconciled cost 0.856); campaign 3.685 → 3.477 ms (Δ0.208 ≥ 0.158). Four separate traces record comparable warm caches and zero uncached spot/cube-face world passes; campaign also has periodic cached spot-world refreshes. |
| M2 sample frames absent/present | symbol-preserving release/debug sample profiles on both maps | manual-performance | pass — on both maps, plain release unset profiles have neither function; same-binary override 1 and debug unset profiles contain both. Ten-second in-level samples, symbols retained. |
| M3 startup logs including dev-tools | windowed startup logs for build/override matrix | manual-runtime | pass — plain release default off, dev-tools release default off, debug default on; explicit on/off overrides name their source, exactly one line per run. |
| M4 visual parity | owner, spawn and short walk on both maps | manual-visual | pass — owner confirmed both maps looked unchanged at spawn and during walks: “Yes, both looked unchanged.” Seven prior static offscreen pairs remain byte-identical. |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin instance-policy slice; build release and attempt hallway paired timing before adding tests | integrating executor | — | done — symbol-preserving release build; paired 35-second hallway launch reached Metal and logged on/off once, but no in-level frames/timing windows. M1 remains outstanding. |
| 2 | Load regressions, pure install check, early failed-load routing and offscreen rejection proof | integrating executor | 1 | done — 10 loader range tests, 3 pure install tests, 2 app routing/state tests; offscreen GPU rejection test ran (1 matched, no skip) and preserved buffers/counts/uploads. |
| 3 | Policy tests, invariant comments, source drift guards and invalid scan fixtures | integrating executor | 1, 2 | done — policy test passed in all four debug/release × plain/dev-tools builds; all 12 contract scans/negative fixtures pass after review repairs. |
| 4 | Focused readiness gate, review/fix loop, final preflight; record all results and external runbook | integrating executor | 2, 3 | done — review's 2 must-fix findings repaired and focused gate passed; final format/clippy/workspace test gate passed. All 19 automated rows pass; foreground follow-up completed M1–M4. Owner authorized landing. |

## Runtime constraints
- Instance policy reads once at creation and adds no frame work. Range checks run O(leaves) only at install; existing cull and draw hot paths remain unchanged.
- Cargo runs are owned by the integrating executor and sequential. Check free disk space after each task/numbered step; only caches may be cleared below 15 GB.

## Review and focused verification
- Readiness: `cargo fmt --check`; touched-crate check with capture/observability; loader range filter (10), pure install filter (3), app install filter (2), and four build/feature policy variants (1 each) passed. The on-demand offscreen GPU rejection test ran and passed (1 matched, no skip).
- Fresh review panel: two slices plus seam pass; two correctness tracers, one contract verifier, one adversarial tester (Sol xhigh), two hygiene/drift reviewers (Sol medium). Runtime flows and comment drift were clean; two must-fix guard gaps found.
- Fixed A16's renamed third-shader binding bypass with structural storage/alias discovery and negative fixtures. Fixed A19's nested-block state restoration with conservative outer-binding invalidation and lexical-shadow fixtures. `cargo check -p postretro-renderer` and `cargo test -p postretro-renderer --lib indirect_contract_` (12 tests) passed after both repairs. No Decision or Acceptance changes.
- Final preflight ran once after integration/review/repairs: `cargo fmt --check` passed; `cargo clippy --target-dir target/preflight-clippy -- -D warnings` passed; `cargo test` passed (9,064 passed, 0 failed, 40 ignored across 60 targets). The on-demand GPU rejection proof above ran separately; its ignored default-suite status is not used as proof. Full-suite compilation emitted one existing level-compiler lib-test dead-code warning (`assemblies`, `brush_assembly`); release matrix builds also exposed existing debug-only upload-test helper warnings. No production warnings in the prescribed clippy gate.
- Check logs: preserved locally at `measurements/release-indirect-validation/runtime/raw/preflight-{fmt,clippy,tests}.log`. Free disk at completion: 19 GiB; cleared inactive incremental caches when space reached the threshold, preserving active compiler caches and all target directories.
- All automated AC results are recorded above. Foreground follow-up completed M1–M4; owner authorized landing. Durable context contracts are updated and the brief is moved to `done/`.

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

## Offscreen screenshot supplement
- Owner requested screenshots while away from the desk. Built `CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro --features capture` (passed); no `dev-tools`. This replaces `target/release/postretro` with a capture-enabled binary; rebuild the plain symbol-preserving release binary before M1/M2.
- Saved fourteen 1280×720 images across seven views: campaign spawn/nearby; hallway spawn/nearby/three turns. Same binary and adapter for every pair. Validation on uses override `1`; off leaves it unset. All seven PNG pairs are byte-identical; each run logged its effective state once.
- Inputs, map/binary/image hashes and gallery: `measurements/release-indirect-validation/screenshots/`. PNGs/logs remain local generated products; scene inputs and comparison report are versioned. Baked section-29 spawn origins/angles agree with the source spawns; eye adds the player's 0.5 m capsule eye height.
- Static capture has no VM/gameplay/HUD or simulated walk. Physical display power state was not inspected. This supplements M4, without claiming M1–M4 passed.

## Foreground follow-up — complete (2026-10-01)

- Owner turned the screen on and authorized agent-launched engine runs. All manual results are now **pass**. No acceptance proof remains outstanding. Owner subsequently authorized landing.
- Evidence: [runtime report](../../../../measurements/release-indirect-validation/runtime/README.md), structured `results.json`, binary/map digests, foreground run records, four cache trace summaries, and two cost profile summaries. The measured plain binary is pinned to `dcde8f292`; source code is unchanged from final preflight. No full suite rerun is needed for these proof/documentation additions.
- M1 used eight complete 120-frame windows per mode/map, discarding the first three and taking the final-five median. Profiling, tracing, and Cargo work were separate from the accepted timing pairs. Same-binary treatment leaves the override unset. Recorded warm state has two camera world passes and zero uncached spot/cube-face world passes. Hallway records no cache refreshes (16,874 indirect world draws/frame). Campaign has 5 / 6 cached spot-world refreshes across 104 / 137 camera-frame equivalents (enabled / unset): ordinary warm frames draw 1,548, refresh frames add 774; sampled means are approximately 1,585 / 1,582. Profile-derived validation-only CPU point estimates are 1.712 / 0.316 ms; observed savings exceed half of each, and the provisional thresholds.
- Machine state: AMD Radeon Pro 5300M / Metal; vsync on; unchanged window/options (resolution `auto`, shadows/fog `low`, Surface Depth `off`). Engine-closed VRAM/utilization snapshots are retained in the structured report, including nonzero shared-GPU utilization. Unsupported timestamp queries are reported explicitly; no GPU-time claim is made.
- M2: both map profiles cover ten in-level seconds, after two timing windows. Both named functions are absent with release default off and present with override 1 and debug default on. M3 includes dev-tools release default and override checks, plus a debug off override, all logging once.
- M4 owner verdict: “Yes, both looked unchanged.” Existing PRLs were used throughout; no re-bake. Live macOS window capture failed (`could not create image from window`); the earlier static offscreen gallery supplies the screenshot evidence.
- Plain release is rebuilt after dev-tools startup checks so Cargo fingerprints remain consistent. Parsed Instruments bundles and session-owned ktrace temporaries are removed; raw XML exports were compressed into `measurements/release-indirect-validation/runtime/raw/` during landing, with uncompressed digests in the summaries. Inactive incremental caches were cleared after disk crossed the skill threshold; full target directories were preserved.
- Trace-summary correction: initial temporal grouping excluded campaign cache-refresh frames. Final summaries count every labelled pass, deduplicating encoder IDs; all five/six refreshes are retained. Trace-boundary frame normalization is approximate. This correction does not change the profile-derived cost thresholds or timing pass results.

## Landing — owner authorized

- Owner said “Land the plane.” All 19 automated and four manual acceptance rows passed before landing.
- Updated durable renderer contracts in §5, §7.1, and §12. Moved this brief to `done/`; no matching roadmap entry exists.
- Proof report remains in `measurements/release-indirect-validation/runtime/`. Temporary raw GPU XML exports are compressed into its ignored `raw/` directory, alongside preserved verification/build logs. Screenshots and runtime logs/profiles remain available locally. Session temporary tools and saved binaries are removed.
- No worktree was created for this build. Cleaned the heavy-churn engine, renderer, and loader packages in the normal and preflight Cargo targets. Source, content, bake caches, and unrelated worktrees are preserved.
- Feature branch: `codex/release-indirect-validation`. Landing commit and proof checkpoints are pushed together. Main-branch merge and post-merge cleanup follow the owner’s merge report.
