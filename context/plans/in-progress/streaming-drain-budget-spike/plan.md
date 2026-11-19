# streaming-drain-budget-spike — plan of record

mode: compact
status: active
read at: 32ac4217e

## Corrections
- **Read at.** The brief was read at e0d708672; the build starts from 32ac4217e. No cited source changed between them: the only edits in the cited trees are co-op session files (`session/mod.rs`, `net_entry.rs`, `progress_save.rs`, `role_state.rs`) and one decode-pool test's ordering handshake. The Decision reads made while drafting stand. AC 1's "values recorded at the read commit" are recorded at 32ac4217e, with the same command, before any spike edit.
- **Showcase copy.** The frozen copy's PRL from `lightmap-compact-storage` no longer loads on `main`: later SH section versions refuse it. Its `.map` survives (sha256 `6f937774718de8bb…`, the 0.1 m source with `_lightmap_density` 0.0125), so it is recovered by rebaking it at 32ac4217e with `--no-cache`, as `drain/sc-frozen/` in the session scratchpad. Prior numbers are re-measured on that bake as the baseline, not compared across bakes. The fresh textured copy baked earlier the same day (`.map` sha256 `488884011ea17e61…`, 31,722 cells) stays available as a second stress fixture; it is not this spike's showcase.
- **Campaign-test.** The repository's `campaign-test.prl` predates `lightmap-compact-storage`. A fresh bake at 32ac4217e (`drain/ct/campaign-test.prl`, 115 MB) is the fixture.
- **Baseline binary.** The walk test binary built at 32ac4217e is kept outside `target/` (`drain/bin/walk-base`), so baseline walks never rebuild against spike edits.

## Delegated answers
- Gate thresholds, the same-class cut count and the sweep's cap values — stated after the first slice (Task 5), before the full matrix reads them.
- Windows GPU-timestamp run — not added; the Mac trace is the default.
- Forced-miss hook — chosen only if the severity read runs (Task 8).

## Lightmap cause buckets (as built)
Classified in `count_visible_misses` (`lightmap_streaming/miss_causes.rs`) after the frame's outcome, first match wins, over drawn blocks only. Per-block marks carry a frame epoch, so they reset without a pass over all blocks.
1. **failed**: marked failed this frame (`fail()`: read, decode, install or preload failure; `abort_drain`: rollback) or phase `Failed`. First, so a rolled-back block re-requested in a later drain of the same frame is still failed (P8).
2. **refused**: phase `Refused`, or (item 9) a band read the request pass held back for pool headroom.
3. **read_in_flight**: phase `InFlight`, or (item 9) targeted and waiting for the issuer, as SH files it.
4. **awaiting_renderer_drain**: phase `InDrain`, or `Ready` and not offered because the previous batch was outstanding (P4).
5. **deferred_by_pool**: `Ready` and deferred in this frame's outcome; wins over a later cut in the same frame, never carried forward (P6). Retiring-generation and device-limit deferrals share it: the outcome does not tell them apart.
6. **held_by_drain_budget**: `Ready`, offered this frame, not deferred: past the shared drain's cut.
7. **outside_reach**: `Absent` and untargeted, or a frame that may not read it.
8. **waiting_on_permit**, **waiting_on_in_hand_cap**: `Absent`, targeted, readable, at or past the block where the latest request pass stopped for permits or in-hand bytes (full or band half). The verdict is the pass's and stands until the next pass (P7, P15).
9. Other `Absent`, targeted, readable blocks: refused or read in flight, as above.

Amendments to the briefed list: **waiting_on_permit** added, because the permit cap is a separate limit from the in-hand bytes and ready pairs left behind hold permits too. **awaiting_renderer_drain** added for P4. **refused** widened to band reads held back for pool headroom (held frames only; portal-walk drawn blocks are never band). **read_in_flight** widened to targeted blocks still waiting for the issuer, as SH does.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 Levers unset: completed walks, SH on and off, equal to read-commit values on both maps | `drain/walks/baseline.jsonl` (walk-base) vs the spike build, same command, both maps; compare late blocks, installs, payload hash, `sh_late`, `sh_installs` | achievable as stated |
| 2 Controller fixture: each cause lands in its own bucket only (P6–P8, P15) | controller unit tests, one per cause plus the pins | achievable as stated |
| 3 Buckets on every SH surface in a non-test build, units named | log-line test via log capture; Streaming tab and capture report JSON field tests; `cargo build` without test | achievable as stated |
| 4 Walk: buckets sum to drawn-not-resident; spawn apart; held-frame gap line (P9, P13) | walk assertion each run plus a frame-driver unit test | achievable as stated |
| 5 Same-class cut counter (P1–P4) | unit tests on the counter over hand-built drains | achievable as stated |
| 6 SH late cluster-frames by cause per frame, spawn apart (P10, P13, P14) | unit test of the per-frame tally on the P10 sequence; walk output | achievable as stated |
| 7 Cap override at production equals unset | completed walks, both maps, override = 8 MiB vs unset | achievable as stated |
| 8 Lever-set cap admits against it (half / twice) | `level_streaming` tests through each lever | achievable as stated |
| 9 In-hand totals unchanged at half and twice | controller test reading the request pass's wait at both lever values | achievable as stated |
| 10 First item over the lever cap admits alone | `level_streaming` test | achievable as stated |
| 11 Settling ignores the levers (P5) | `level_streaming` test, settling frame with either lever set | achievable as stated |
| 12 Install-cost cold drains counted apart (P12) — if the gate opens | renderer test classification | conditional on gate |
| 13 Install-cost sizes and counts separate fixed and per-MiB cost — if the gate opens | renderer test design | conditional on gate |
| 14 Grep gate: slider dev-tools only; override, tallies, test levers test-only; no player setting | grep test in the crate's test suite | achievable as stated |
| 15 Realtime medians of 3 per walk, by cause, both resources, both core settings | walk matrix, Task 6 | manual / measured |
| 16 Gate decision with thresholds and shares | findings note | manual / measured |
| 17 Cap sweep — if the gate opens | walk matrix with the override | conditional on gate |
| 18 Install cost CPU and GPU — if the gate opens | renderer tests plus Metal System Trace | conditional on gate |
| 19 Owner visual read of forced misses — if same-class cuts exceed the count | owner, in-engine | conditional on gate |
| 20 Owner read with the slider at a showcase burst | owner, in-engine (dev-tools build) | manual |
| 21 Findings note answering the cap question, per assumption | `findings.md` | manual |
| 22 Note records copy hash, confounders, SH bucket caveat, amendments, added levers | `findings.md` | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 0 | Baseline at 32ac4217e: bakes (campaign-test; frozen showcase `--no-cache`), walk-base binary, completed walks SH off/on ×2 on both maps | integrating executor | — | campaign-test done: both walks 0 late blocks, installs 5,986 / 5,758, SH 19 installs, 0 `sh_late`, reruns identical. Showcase waits on its bake |
| 1 | Production lightmap cause buckets in the controller (request-pass, drain-outcome and rollback verdicts per block), fixture tests (AC 2), and the three surfaces (AC 3) | delegated slice | 0 | done: buckets above; 13 fixture tests in `miss_cause_tests.rs` (one per cause, P4, P6, P7/P15, P8, settling); log-line tests for both resources via log capture, capture-report JSON fields with `miss_unit`, Streaming tab section tests. postretro bin 2,739 passed, renderer `streaming_tab` 5, capture report 14; clippy with and without dev-tools, fmt clean. `take_requests` reordered to read the target before the permit check; submissions unchanged (AC 1 run confirms). `rendering_pipeline.md` §4 lists the buckets |
| 2 | Cap levers: one play-cap override on the level's drain step, set by the dev-tools slider and by the walk's env override; tests AC 7–11; grep gate AC 14 | delegated slice | 1 | |
| 3 | Walk tallies: bucket sums and held-frame gap (AC 4), same-class cut counter (AC 5), SH per-frame tally (AC 6), largest drains with per-resource shares, spawn step apart; results JSON | delegated slice | 1, 2 | |
| 4 | AC 1 and AC 7 runs: spike build vs baseline, completed, both maps | integrating executor | 3 | |
| 5 | First slice: realtime showcase far-point at the production cap; state thresholds; open or close the gate | integrating executor | 4 | |
| 6 | Full matrix: realtime ×3, showcase far-point and random, campaign-test, default and 1 core, SH on (AC 15, 16) | integrating executor | 5 | |
| 7 | If the gate opens: cap sweep (AC 17); install-cost tests and trace (AC 12, 13, 18) | integrating executor | 6 | |
| 8 | If same-class cuts exceed the count: forced-miss hook, owner visual read (AC 19). Owner slider read (AC 20) | owner | 6 | |
| 9 | Findings note (AC 21, 22) | integrating executor | 6–8 | |
