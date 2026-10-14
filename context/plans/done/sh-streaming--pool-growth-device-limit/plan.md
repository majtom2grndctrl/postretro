# sh-streaming--pool-growth-device-limit — plan of record

mode: compact
status: done
read at: e8f607b94

## Corrections
- Source changes since brief read-at 6078fff do not change SH pools, planner or controller. Re-read current geometry, growth, install/outcome, failure/retry and frame error boundaries; Decision premises hold.
- No extra worktrees exist. Initial disk recovery cleared Cargo incremental and downloaded archive caches; owner is also running cargo clean. Owner confirmed cargo clean finished; builds remain centralized after concurrent edits.

## Delegated answers
- Fragmentation headroom: lightweight first-fit experiment tested 1.25× (refused) and 1.5× (passed with rectangular padding); select 2.25× = 1.5× × 1.5 lifetime margin, confirmed by `fixture_churn_fits_fragmentation_headroom` in the focused renderer run. Frozen writer-closure sizes: campaign-test (21, total 57,296 tiles) and stress-warren-hallway-inspection (193, total 1,041,044 tiles). Xorshift64 seeds [1, 7, 19, 43, 101, 313, 911, 2027], 20,000 steps/seed, shuffled full demand every 257 steps and both ends. Raw peak ratios 1.5054105 / 1.0726233; largest tested refused factor 1.25×.

## AC-to-proof

Numbered in the order of the brief’s Automated rows; M1 is its Manual row.

| AC | Proof | Status | Result |
|---|---|---|---|
| A1 | `sh_growth_production_zero_share_load_holds_every_cluster` | achievable as stated | pass: focused proof and source/contract review |
| A2 | `whole_level_geometry_preserves_floor_share_and_covers_tiny_zero_and_full_layers`; `sh_growth_floor_geometry_and_charges_match_gpu_for_every_load_shape` | achievable as stated | pass: focused proof and source/contract review |
| A3 | `sh_growth_dense_clamps_and_limit_precedes_retirement` | achievable as stated | pass: focused proof and source/contract review |
| A4 | `sh_growth_dense_clamps_and_limit_precedes_retirement` | achievable as stated | pass: focused proof and source/contract review |
| A5 | `sh_growth_sparse_clamps_every_family_and_preflights_the_whole_transaction` | achievable as stated | pass: focused proof and source/contract review |
| A6 | `sh_growth_dense_clamps_and_limit_precedes_retirement`; `sh_growth_sparse_clamps_every_family_and_preflights_the_whole_transaction` | achievable as stated | pass: focused proof and source/contract review |
| A7 | `sh_growth_keeps_dense_commit_when_sparse_refuses_and_classifies_later_clusters` | achievable as stated | pass: focused proof and source/contract review |
| A8 | `sh_growth_refusal_rolls_back_addresses_in_both_drain_orders` | achievable as stated | pass: focused proof and source/contract review |
| A9 | `refusal_releases_ready_charge_and_permit_once_without_changing_other_work`; GPU rollback proof | achievable as stated | pass: focused proof and source/contract review |
| A10 | `refusal_counts_only_new_visible_episodes_without_rebucketing`; `device_limit_owner_dominates_read_failure_anywhere_in_closure_in_either_order` | achievable as stated | pass: focused proof and source/contract review |
| A11 | `refusal_in_reveal_drain_preserves_the_reveal_frames_existing_miss` | achievable as stated | pass: focused proof and source/contract review |
| A12 | `refusal_retries_on_repeated_expired_leave_and_reentry_without_spending_read_retry`; `early_seam_reentry_never_wakes_on_time_alone_and_later_reentry_does` | achievable as stated | pass: focused proof and source/contract review |
| A13 | `refusal_read_retry_sequences_preserve_the_lifetime_read_budget_and_warning` | achievable as stated | pass: focused proof and source/contract review |
| A14 | `rapid_horizon_churn_under_pressure_keeps_targeted_refusal_failed`; `optional_refusal_is_skipped_only_by_trim_and_does_not_inflate_protected_overshoot` | achievable as stated | pass: focused proof and source/contract review |
| A15 | `later_eviction_does_not_wake_targeted_refusal_and_refusal_does_not_request_evictions` | achievable as stated | pass: focused proof and source/contract review |
| A16 | `refused_owner_blocks_all_dependent_reads_bytes_and_permits_until_retry_is_sampleable`; `evicted_owner_waits_for_old_reads_then_refusal_releases_transitive_ready_work`; `full_ready_permits_reclaim_only_dependents_for_mandatory_and_optional_owner_reads`; `optional_owner_recovery_reclaims_across_stalled_cohorts_and_preserves_other_work`; `overlapping_refused_owners_release_their_shared_ready_dependent_once` | achievable as stated | pass: focused proof and source/contract review |
| A17 | `device_limit_warning_names_only_first_cluster_per_level`; `device_limit_refusal_then_corrupt_reread_warns_for_both_causes_once` | achievable as stated | pass: focused proof and source/contract review |
| A18 | `device_limited_mandatory_sh_reveals_into_running_only_at_timeout`; `sh_device_limit_refusal_holds_until_timeout_and_warns_once_across_mode_replacements`; `refused_optional_seam_target_does_not_hold_settling` | achievable as stated | pass: focused proof and source/contract review |
| A19 | `refusal_does_not_evict_pin_lead_owner_or_optional_residents_when_floor_covers_targets`; `ordinary_pressure_eviction_in_refusal_drain_does_not_trigger_further_eviction` | achievable as stated | pass: focused proof and source/contract review |
| A20 | `sh_growth_contract_and_metadata_errors_remain_fatal`; `sh_growth_retained_metadata_error_precedes_dense_device_limit` | achievable as stated | pass: focused proof and source/contract review |
| A21 | `sh_growth_contract_and_metadata_errors_remain_fatal`; classifier-path GpuCapacity assertions and `sh_growth_slot_address_overflow_escapes_drain_refusal_classifier` | achievable as stated | pass: focused proof and source/contract review |
| A22 | `refusal_outcome_validation_rejects_all_invalid_partitions_before_mutation`; `refusal_accounting_underflow_rejects_the_whole_outcome_before_eviction` | achievable as stated | pass: focused proof and source/contract review |
| A23 | Shared `AtlasShape` geometry source inspection and floor GPU proof | achievable as stated | pass: focused proof and source/contract review |
| A24 | `fixture_churn_fits_fragmentation_headroom` with named frozen arrays and eight deterministic seeds | achievable as stated | pass: focused proof and source/contract review |
| A25 | Captured-limit propagation source inspection and all GPU growth/geometry tests | achievable as stated | pass: focused proof and source/contract review |
| A26 | Final `POSTRETRO_REQUIRE_GPU=1` growth suite after preflight; no skips, overrides ≤ device grants | achievable as stated | pass: final 10 required-GPU growth tests, zero ignored/skipped |
| M1 | Owner in-engine A/B walks: campaign-test and stress-warren hallway at default limits; no warning; initial bytes, tiles/layer, growth, copy regions/install and install CPU | external manual proof; blocks landing | pass: both before/feature walks recorded; zero SH device-limit warnings; allocation/growth/copy/CPU comparison below; hallway route differs |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Shared geometry, captured limits, churn headroom and zero-share production-load proof (highest-risk premise) | integrating executor / renderer worker | — | complete: actual PRL zero-share load, geometry cases and seeded churn pass |
| 2 | Clamp dense and all sparse growth, distinguish refusal from contract errors, GPU ordering/rollback proof | renderer worker | 1 | complete: real GPU clamp/refusal/rollback/readback proofs pass |
| 3 | Controller refusal failure cause, accounting, retries, pressure/miss policy and focused tests | app worker | shared outcome contract | complete: 26 refusal tests passed in 224-test focused app run, including late-read and split-cohort recovery |
| 4 | Level-scoped warnings, diagnostic consumers, Settling/capture integration and proof | integrating executor | 2, 3 | complete: session/Settling/log proofs, capture serialization (1), Streaming tab (5) pass |
| 5 | Review-readiness, isolated review-panel/fix loop, final preflight plus required-GPU run | integrating executor | 1–4 | readiness passed; isolated complete: review fixes approved; fmt/Clippy/full serial workspace suite and final 10 required-GPU tests and M1 pass; complete; owner authorized landing and PR on 2026-10-13 |

## Shared contracts and performance
- `ShDrainOutcome.refused: Vec<u32>` accounts for device-limit failures exclusively and is disjoint from accepted/dropped/deferred. Renderer drops refused payloads; app releases their in-drain ready charge and permit and records the latest failure cause.
- `ShMissBuckets.device_limited: u64` counts episodes beginning behind a refused cluster/owner; existing episodes never rebucket.
- Renderer-internal `GpuDeviceLimit { reason }` is only for growth limit verdicts; `GpuCapacity` continues to identify fatal contracts.
- No refusal-driven eviction or wake. Existing ordinary floor pressure remains. Retry window is the existing 2 s hysteresis measured from last refusal. Read-failure budget survives refusal and is spent only at read-failure retry.
- Capture device limits at initialization and pass them through every SH geometry/floor/growth/replacement check. Test overrides must not exceed grants.
- Frame work remains target/change driven; no waits on growth; refusal drops payload rather than parking it. Warning latch belongs to the loaded level, across SH controller replacements. Existing counters provide install/growth CPU measurements.

## External runbook

M1 required these walks before landing; its completed result is recorded below. Automated results cannot substitute for manual evidence.

1. Baseline: `e8f607b94` (claim-only commit, before implementation). Feature: this branch after final checks. Use the same compiled `campaign-test` and `stress-warren-hallway-inspection` PRLs for both; do not rebake between runs. Keep their content hashes with the measurements.
2. Pin machine/GPU/backend, window resolution, lighting options and the route through each map. Record the route/poses and run duration. Use the same feature set (`dev-tools`) and default device limits/streaming settings; no test-limit overrides. Record other GPU workloads so contention is visible.
3. Build the authoring engine and helpers before launching through `postretro-tool run --install-root .`. Existing compiled PRLs live under `content/dev/maps/`. Check disk budget before any comparison checkout/build; never share target dirs across checkouts. Copy comparison binaries out and remove their temporary target after building.
4. Enable `RUST_LOG=info,postretro_renderer::render::sh_streaming=trace`. The global `info` filter is required to retain the app's device-limit warnings and periodic counters. The feature's load log reports `[Renderer] streamed SH initial dense pool: bytes=… tiles_per_layer=… layers=… slots=…`. Each successful nonempty install trace reports cluster ID, `upload_copy_regions` and `install_cpu_micros`; periodic logs report pool growth and steady/growth install cost.
5. The baseline predates the measurement logs. From its isolated checkout at `e8f607b94`, apply `git apply /Users/dhiester/Projects/Personal/postretro/context/plans/done/sh-streaming--pool-growth-device-limit/baseline-measurement.patch`. This supplied patch adds only initial dense-pool logging and the same conditional install timing/copy counts; it preserves baseline geometry and policy. The patch was checked against the exact baseline files. Discard its temporary instrumentation after the comparison.
6. For each map/build, walk the pinned route from a fresh level entry. Report device-limit warning count (must be zero), initial dense atlas bytes and tiles/layer, pool growth count, copy regions per install, and install CPU (total, per-install distribution and slowest steady drain). Keep settling and running costs distinct and report sample counts.
7. Record pass/fail and actual measurements in M1. Clean comparison binaries, logs and temporary instrumentation/checkouts after recording the result. A failure remains a blocking result to investigate; no inferred pass.

### Feature launch commands

From the workspace root, with the two existing compiled PRLs retained:

```bash
CARGO_INCREMENTAL=0 cargo build -p postretro -p postretro-script-compiler -p postretro-tool --features postretro/dev-tools
RUST_LOG=info,postretro_renderer::render::sh_streaming=trace target/debug/postretro-tool run --install-root . maps/campaign-test.prl
RUST_LOG=info,postretro_renderer::render::sh_streaming=trace target/debug/postretro-tool run --install-root . maps/stress-warren-hallway-inspection.prl
```

Run the two walks separately. These commands do not rebuild the PRLs. Build and launch the instrumentation-only baseline from its isolated checkout in the same way, using identical PRL bytes. Keep the logs and measurements until M1 is recorded.

### Prepared before-walk launchers

Historical preparation record; this bundle is removed during landing cleanup. Pre-change executables were prepared in the ignored `target/sh-before-walks/` bundle. Built from `e8f607b94744e338039fcc4389ead98ef91ef6d6` with only `baseline-measurement.patch`, default dev profile and `postretro/dev-tools`. Separate worktree/target; no shared Cargo artifacts. Build passed in 24m 13s. Copied and hash-verified engine, script compiler and tool total 93.4 MiB. The primary feature executable hash did not change during preparation.

Baseline engine SHA-256: `8653e9a0a83ecbe03b1ccca3658a85d2092aa13a01037d73d2f3d3919a7d2d32`. Bundle `build-info.json` records executable hashes and build provenance. Helper CLI and shell syntax passed. The initial launch check bypassed engine argument validation and missed incorrect map paths; see the correction below. Temporary target was removed; the clean checkout was archived after verifying its commit is preserved on remote main. About 23 GiB remained free after cleanup.

Launcher correction: `postretro-tool run` supplies `--mod dev`, so both engine arguments must be mod-relative `maps/…` paths. Filesystem hash checks retain absolute `content/dev/maps/…` paths. Both routes were checked through the baseline tool with an argument-recording engine substitute: correct map argument, `--mod dev`, unchanged PRL hashes and log headers. This verifies launch arguments; it does not substitute for the manual walks. The feature launch examples above use the same corrected path form.

From the primary workspace root, run these separately. Repeat the new-build routes and settings, then quit each game completely:

```bash
./target/sh-before-walks/run-before.sh campaign
./target/sh-before-walks/run-before.sh hallway
```

The scripts launch the old engine using the primary project's existing assets/settings and save full startup/walk output automatically:

- `campaign-test-before-sh-log.txt`
- `stress-warren-hallway-inspection-before-sh-log.txt`

If a log already exists, its replacement run gets a timestamp/PID suffix instead of overwriting it. Headers record the baseline commit, engine hash and selected map hash. Scripts refuse changed PRL bytes against `prl-sha256.txt`. Preparation hashes:

| PRL | SHA-256 |
|---|---|
| campaign-test | `868cf5db03ec7c9a97f75d54cad1f9d8d9c6c4b7337af6c64c3de22793a70ee2` |
| stress-warren-hallway-inspection | `9a4a5d089c5cfaf8f068c13c8d39e6b472ecf72d39f8e15eb37c2db91a1eb53a` |

Do not rebake between comparison runs. Preserve the bundle until both baseline walks are recorded and analyzed, then remove it during landing cleanup. At preparation M1 was outstanding; no baseline walk was performed during preparation. Completed walks are recorded below.

## Verification log

All build/test commands use `CARGO_INCREMENTAL=0`. Host permissions are needed for normal network access, Metal and macOS test-binary initialization. A sandboxed test process stalled in dyld/AppKit logging initialization before the Rust harness; host retry resolved it without code changes.

| Gate | Result |
|---|---|
| Formatting | pass |
| Touched-crate checks, test targets, dev-tools/capture | pass |
| Focused app streaming tests | 224 passed, 0 failed, 2 existing heavy tests ignored; all 26 refusal tests included |
| Focused renderer SH tests, required GPU | 174 passed, 0 failed, 0 ignored/skipped, AMD Radeon Pro 5300M / Metal |
| Capture serialization / Streaming tab focused consumers | 1 / 5 passed |
| Workspace Clippy, all targets, dev-tools/capture, warnings denied, dedicated target | pass |
| Full workspace tests, dev-tools/capture, serial test threads | pass: 10,980 passed (10,978 unit/integration + 2 doc), 0 failed, 57 existing ignored |
| Final required-GPU growth suite after preflight (A26) | pass: 10 matched/passed, zero failed/ignored/skipped, AMD Radeon Pro 5300M / Metal |

Final commands:

```bash
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features postretro/dev-tools,postretro/capture --target-dir target/preflight-clippy -- -D warnings
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test --workspace --features postretro/dev-tools,postretro/capture -- --test-threads=1
POSTRETRO_REQUIRE_GPU=1 CARGO_INCREMENTAL=0 cargo test -p postretro-renderer --lib --features dev-tools sh_growth_ -- --nocapture
```

The full workspace suite starts only after the independent review/fix/focused-retest loop. Two mechanical Clippy suggestions in tests were fixed and lint was rerun successfully before the workspace test run. No full workspace suite ran earlier.

## Review loop

The isolated panel reviewed `59b9da7ba..4b0d7fc8c`: 12 read-only passes across all 40 changed files and the cross-subsystem seam. Four findings were acted on:

1. Late dependent reads/payloads could remain behind a refused reread owner (A16/P13). Cache transitive owner closure per active permit (at most eight), wait for physical Queued completions before any owner reread, and preflight/drop retained or deferred Ready dependents on refusal. Preserve independent failure history. No physical read permit is released early.
2. Dense growth refusal could mask retained-metadata disagreement (A20). Reserve and validate CPU metadata/address/row/compose contracts before recoverable growth. Shared GPU validators accept explicit planned capacities before growth and verify actual backing afterward. Successful dense growth remains committed before a later sparse refusal.
3. Fatal-contract proofs bypassed the new refusal classifier (A21). Missing handles, broken post-growth backing and canonical slot overflow now traverse `install_ready` or drain and assert exact escaping errors.
4. Sparse proofs lowered only the storage-binding bound (A5/A6). The matrix now independently tightens each device bound across all three sparse families and sibling atomicity. The total-buffer case uses 65,536 bytes to accommodate the existing fixed compose record; sparse demands and exact clamps scale to that bound. The other device bound remains at its grant.

Targeted lifecycle repair review found one additional liveness edge: eight Ready reads split across two now-optional owner groups could not individually reclaim enough permits. Recovery now takes its own blocked cohort first, then other Ready work blocked by Absent owners, while preserving installable, in-flight and compose-waiting work. Five realistic new lifecycle tests cover success/failure/cancellation, retry, stale completion, diamond deduplication, arithmetic atomicity, full mandatory/optional slots, split groups and unrelated work.

Targeted renderer repair review and lifecycle re-review both approve, with no actionable regressions. Cross-family retirement ordering preserves P3's documented dense-commit behavior; no Decision or Acceptance change was needed. No unsafe code was introduced.

The first full parallel run reported 8,035 passed, 1 failed, 56 ignored before reaching doc-tests. The unchanged `two_frames_in_flight_reach_a_bounded_staging_steady_state` expected the warmup allocation count to remain 2 but observed 3 (within its separate live-buffer bound of 4). Source is unchanged from the baseline. No test or production code was altered for this failure. Its isolated required-GPU rerun passed on the same AMD/Metal adapter with three created/live buffers. The full serial workspace rerun passed, including the renderer target (950 passed, 8 ignored). This existing test is sensitive to concurrent driver timing; the final gate records serial test execution explicitly.

## Manual evidence — campaign-test feature walk

Partial M1 evidence supplied by the owner on 2026-10-13. Source: temporary workspace-root `campaign-test-sh-log.txt` (60 lines), SHA-256 `bebfa87654d5d332da6d644d3ca217679133dcf56d088217ee5a9a35eacf511c`. Captured timestamps: 21:19:20–21:20:18 UTC (58 s). New feature-format logs; executable revision, map hash, route, resolution/options and device-limit overrides are not recorded in this excerpt. User identifies the map as campaign-test.

| Captured measurement | Result |
|---|---|
| SH device-limit warnings / WARN / ERROR records | 0 / 0 / 0 |
| Visible SH misses, including device-limit bucket | 0 across all 11 periodic windows |
| SH pool growth events / growth CPU | 0 / 0.00 ms |
| Target/sampleable consistency | all targets sampleable at each report; 9–17 targets |
| Queued / Ready / held permits at report times | 0 / 0 / 0 in every report |
| Re-reads / evictions / retries | 11 / 19 / 0 across reported windows |
| Initial dense atlas allocation | 62,949,120 B (60.033 MiB), supplied startup line at 21:19:19 UTC |
| Initial tiles per layer / layers / slots | 37,830 / 1 / 37,830 |
| Reported active SH pool capacity | 71.0 MiB; includes other SH allocations, so cannot substitute for initial dense atlas bytes |

Per-install trace measurements split at the `[Startup]` report. CPU percentiles use nearest rank; copy means include zero-copy installs.

| Phase | Installs | Copy regions total / mean / range | Install CPU total / mean / median / p95 / max |
|---|---:|---|---|
| Before startup report | 13 | 938 / 72.15 / 0–347 | 39.825 / 3.063 / 0.567 / 24.238 / 24.238 ms |
| After startup report | 19 | 890 / 46.84 / 0–140 | 25.732 / 1.354 / 0.369 / 5.877 / 5.877 ms |
| Whole capture | 32 | 1,828 / 57.13 / 0–347 | 65.557 / 2.049 / 0.468 / 5.877 / 24.238 ms |

Periodic drain CPU sums to 72.71 ms: 42.03 ms in the first startup-containing window and 30.68 ms in later windows. This includes drain overhead and differs from the per-cluster trace sum. The cumulative no-growth drain maximum is 25.13 ms, already present at the first report; it is not evidence of a new 25 ms walking hitch. “Steady” in that counter means no pool growth, not post-settling.

One lightmap refusal and one repack appear in its separate diagnostics, with zero reported lightmap visible misses or failed installs/reads. These do not count as SH device-limit warnings.

The owner supplied the missing startup measurement separately in chat: `[Renderer] streamed SH initial dense pool: bytes=62949120 tiles_per_layer=37830 layers=1 slots=37830` at 21:19:19 UTC. The original temporary log and its recorded hash remain unchanged. The 8 B monolithic base-atlas placeholder in the adjacent line is separate from this streamed dense pool.

The captured SH warning check passes for this excerpt. M1 remains outstanding: pre-change measurements for both maps and matched run conditions. Visual smoothness/lighting cannot be inferred from logs. Keep the original temporary log untracked and unchanged.

## Manual evidence — stress hallway feature walk

Owner supplied `sh-test-hallway-stress-log.txt` on 2026-10-13 (429 lines), SHA-256 `783815e8375c78271fd679130b4c199268831835f0c1cba1c82337ca0af19f49`. Full launch/exit timestamps: 21:30:47–21:32:09 UTC (82 s); level startup report at 21:30:53 UTC. Log confirms `content/dev/maps/stress-warren-hallway-inspection.prl`, AMD Radeon Pro 5300M / Metal, and the feature branch. Exact executable revision, map hash, route and resolution/options are not recorded.

| Captured measurement | Result |
|---|---|
| SH device-limit warnings / visible SH misses | 0 / 0 across all 15 reporting windows |
| Initial dense atlas allocation | 169,861,120 B (161.992 MiB) |
| Initial tiles per layer / layers / slots | 102,080 / 1 / 102,080 |
| Pool growth events / added capacity | 12 / 1,260.0 MiB, all in the first reporting window; none later |
| Active SH pool capacity after growth | 1,440.0 MiB; not the initial dense-only allocation |
| Growth CPU | 70.89 ms, all in the first reporting window |
| Re-reads / evictions / retries / budget-limited drains | 13 / 94 / 0 / 2 across reporting windows |
| Work pending at report times | one report: 112 targets, 108 sampleable, 1 Queued/held permit; next report 108/108 sampleable and 0 held permits; all other reports fully sampleable |
| Error / panic records | 0 / 0 |

Per-install trace measurements split at the level `[Startup]` report. CPU p95 uses nearest rank; copy means include zero-copy installs.

| Phase | Installs | Copy regions total / mean / range | Install CPU total / mean / median / p95 / max |
|---|---:|---|---|
| Before startup report | 77 | 8,985 / 116.69 / 0–313 | 390.845 / 5.076 / 3.740 / 20.047 / 28.601 ms |
| After startup report | 59 | 5,095 / 86.36 / 0–210 | 201.194 / 3.410 / 2.604 / 14.676 / 20.584 ms |
| Whole capture | 136 | 14,080 / 103.53 / 0–313 | 592.039 / 4.353 / 3.262 / 16.945 / 28.601 ms |

Periodic drain CPU totals 633.42 ms: 506.70 ms in the first startup-containing window, 126.72 ms in later windows. That first window includes eight installs after the startup report, so its twelve growths cannot all be assigned to Settling. The cumulative largest drain is 36.14 ms; the largest no-growth drain is 34.15 ms. Both are already present in the first report and remain unchanged. Drain timing includes overhead outside individual install traces.

The full log also contains 135 non-SH warning records: 100 typedef-generation warnings and 35 behavior/content/UI/AI warnings. No SH device-limit warning appears. Their pre-change status is unknown until comparison; they are not grounds to mark the whole log warning-free.

Both feature-map logs satisfy the captured zero-SH-device-warning check. The hallway's early capacity growth and install costs require the pre-change comparison; no performance pass or regression is inferred. M1 remains outstanding for the two baseline walks and matched run conditions. Preserve both original temporary logs untracked and unchanged.

## Manual evidence — completed before/feature comparison

M1 passes its stated manual acceptance: both default-limit maps logged zero SH device-limit warnings, and initial allocation, growth, copy regions/install and CPU measurements are reported against the pre-change build. Owner completed both before walks and confirmed the same GPU, resolution and lighting settings, with no test-limit overrides. Owner reports similar fast turns/hallway traversal, not an identical hallway route. This is an observational comparison, not proof of a performance improvement or absence of hitches.

Baseline provenance: launcher headers identify `e8f607b94744e338039fcc4389ead98ef91ef6d6`, logging-only instrumentation, dev profile/dev-tools, baseline engine hash and unchanged PRL hashes from the prepared bundle. Baseline logs and the full feature hallway log confirm AMD Radeon Pro 5300M / Metal; surface 2560×1440, scene 1280×720, divisor 2. Feature campaign startup was supplied separately in chat; its excerpt lacks executable/map hashes. Other GPU workloads and exact feature executable revision remain unrecorded.

Selected baseline sources (retained untracked):

| Log | Lines | SHA-256 |
|---|---:|---|
| `campaign-test-before-sh-log-20261013-165822-34484.txt` | 382 | `67bcb6c5a320305780e54f85acdd5cf0efdd2f88ea26ff0238cb4e336efc4a0f` |
| `stress-warren-hallway-inspection-before-sh-log.txt` | 634 | `23ab20cafc27121360682ecfc5d1a0052a657457800c23c110c7f48c6ff1e558` |

The earlier campaign run `campaign-test-before-sh-log-20261013-165744-34387.txt` contains only 16 installs and little traversal; use the later 32-install run above. The unsuffixed campaign file is the failed path launch and supplies no walk evidence. Do not combine these runs.

| Measurement | Campaign before → feature | Hallway before → feature |
|---|---|---|
| Initial dense bytes | 62,949,120 → 62,949,120 | 169,861,120 → 169,861,120 |
| Initial tiles/layer; layers | 37,830; 1 → 37,830; 1 | 102,080; 1 → 102,080; 1 |
| Pool growth count / added capacity | 0 / 0 → 0 / 0 | 12 / 1,260 MiB → 12 / 1,260 MiB |
| Growth CPU | 0 → 0 ms | 76.99 → 70.89 ms; first window only |
| SH device-limit warnings / visible misses | 0 / 0 → 0 / 0 | 0 / 0 → 0 / 0 |
| Whole-capture installs / copy regions | 32 / 1,828 → 32 / 1,828 | 309 / 34,568 → 136 / 14,080 |
| Whole-capture trace CPU | 60.906 → 65.557 ms | 1,362.432 → 592.039 ms |
| Whole-capture periodic drain CPU | 71.28 → 72.71 ms | 1,448.94 → 633.42 ms |
| Re-read requests / evictions | 11 / 19 → 11 / 19 | 182 / 210 → 13 / 94 |
| Budget-limited drains | 0 → 0 | 21 → 2 |

Baseline trace phases split by line order at the level `[Startup] worker_dispatch=…` report. This avoids assigning all same-second work to Settling. CPU p95 uses nearest rank. Copy means include zero-copy installs.

| Map / phase | Installs | Copy regions total / mean / range | CPU total / mean / median / p95 / max (ms) |
|---|---:|---|---|
| Campaign before startup report | 13 | 938 / 72.15 / 0–347 | 38.537 / 2.964 / 1.192 / 22.232 / 22.232 |
| Campaign after startup report | 19 | 890 / 46.84 / 0–140 | 22.369 / 1.177 / 0.394 / 4.779 / 4.779 |
| Campaign whole capture | 32 | 1,828 / 57.13 / 0–347 | 60.906 / 1.903 / 0.492 / 4.779 / 22.232 |
| Hallway before startup report | 80 | 9,503 / 118.79 / 0–313 | 459.306 / 5.741 / 3.698 / 24.412 / 42.608 |
| Hallway after startup report | 229 | 25,065 / 109.45 / 0–219 | 903.126 / 3.944 / 3.305 / 9.278 / 39.298 |
| Hallway whole capture | 309 | 34,568 / 111.87 / 0–313 | 1,362.432 / 4.409 / 3.440 / 12.680 / 42.608 |

### Campaign workload match

Owner reports a closer, though not mechanically identical, campaign route. Both captures have the same 13 before-startup and 19 after-startup installs, 1,828 total copy regions, 11 re-reads and 19 evictions. This is the stronger workload comparison. Trace CPU rises 60.906 → 65.557 ms overall (+4.651 ms, ~7.6%); periodic drain CPU rises only 71.28 → 72.71 ms. After-startup mean CPU rises 1.177 → 1.354 ms (~0.177 ms/install), while median falls 0.394 → 0.369 ms. Maximum rises 4.779 → 5.877 ms; with only 19 samples nearest-rank p95 is also the maximum. Mixed timing and one run per build cannot establish either a speed improvement or a regression. It gives no support for attributing the hallway’s large upload-count reduction to this feature.

### Hallway duration, frequency and payload normalization

Full recorded spans: before 00:00:31–00:01:58 UTC on October 14 (87 s), feature 21:30:47–21:32:09 UTC on October 13 (82 s). Local dates are October 13 for both. Startup-report-to-exit spans are about 80 / 76 s; 229 / 59 installs yield 2.86 / 0.78 installs/s and 11.29 / 2.65 trace CPU ms/s. Timestamps have one-second precision and cannot identify frame boundaries or exact movement duration.

Exclude the first reporting window from both to separate early growth/reveal work. Remaining complete periodic windows span 75 / 70 s. They account for 224 / 51 installs; the first window in each accounts for 85 installs and 206.7 MiB. Payload totals below sum rounded logged decoded-install sizes, not measured GPU transfer bytes.

| Later-window measurement | Before | Feature |
|---|---:|---:|
| Installs per second | 2.99 | 0.73 |
| Installed payload total / per second | ~398.8 MiB / 5.32 MiB/s | ~59.0 MiB / 0.84 MiB/s |
| Mean installed payload / install | ~1.78 MiB | ~1.16 MiB |
| Copy regions / install | 108.95 | 76.76 |
| Trace CPU total / mean / p95 / max | 832.447 / 3.716 / 8.329 / 20.636 ms | 118.646 / 2.326 / 4.418 / 5.747 ms |
| Trace CPU per MiB of installed payload | ~2.09 ms | ~2.01 ms |
| Periodic drain CPU per MiB of installed payload | ~2.23 ms | ~2.15 ms |
| Re-read requests / evictions | 181 / 209 | 12 / 93 |
| Windows with installs / later windows | 12 / 15 | 11 / 14 |
| Largest five-second install count | 36 | 26 |

The similar duration does not explain the upload difference. Before had more repeat loading and larger average payloads. Its lower-level CPU cost per payload MiB is close to the feature's; lower feature CPU per install is consistent with smaller payloads. Route/demand differences and unrecorded machine load prevent attributing this to the change. Both runs spread uploads across most five-second windows, with bursts rather than uniform intervals. At one-second log resolution the largest later bursts contain 33 / 23 installs and 129.484 / 78.579 ms summed trace CPU; neither sum is a frame time. Feature's largest burst occurs about 45 s after its startup report; before's about 60 s after its report.

Expected behavior at these default limits: protect against device-limit crashes, hold the whole-level geometry minimum and preserve ordinary growth. Both maps retain the same initial geometry and growth counts. No device-limit refusal was triggered. This evidence does not show that the change reduces routine upload frequency; automated lowered-limit and zero-share tests exercise its intended new paths.

### Hitch evidence

Before hallway at 00:01:13 UTC / 17:01:13 local: clusters 2452, 2613 and 3067 cost 20.636, 19.097 and 16.726 ms individually (raw log lines 452, 455, 462). Later work includes 16.376 ms at 00:01:48 and 15.898 ms at 00:01:49 (lines 585, 592). These occur after the startup-containing window. Individual work around or above a 16.67 ms frame budget can contribute to the observed hitches. Source `crates/renderer/src/render/sh_streaming/install.rs` times CPU preparation/staging/submission through install completion, not GPU execution or presentation.

The before hallway lifetime maximum drain is 49.57 ms (also no-growth maximum), already present at its first report and unchanged afterward. It cannot be interpreted as a repeated 49.57 ms walking hitch. The feature's maxima are 36.14 ms overall / 34.15 ms no-growth, also fixed from its first report. Both runs report no later growth. Baseline 39.298 ms and feature 20.584 ms individual after-startup maxima occur immediately after their startup reports; later-window maxima are the separate 20.636 / 5.747 ms values above.

Campaign's 19 after-startup installs have max CPU 4.779 → 5.877 ms. Its lifetime drain max 23.53 → 25.13 ms is already present at each first report. No large late SH-install spike appears there. Lightmap maxima (campaign 112.36 → 106.33 ms; hallway 88.59 → 86.47 ms) are also present at first reports and never increase. Asynchronous read latency, pending optional targets and budget-limited drains are not direct frame-stall measurements. No CPU frame-stage or GPU timing capture is present, so the logs cannot attribute every perceived hitch or prove smooth frame delivery.

Manual acceptance is complete. Full frame-timing attribution or a repeatable matched-route performance study would be separate work. Keep comparison artifacts until landing cleanup; preserve the owner's raw logs unless cleanup is authorized.

## Landing

Owner authorized “land the plane” and requested a PR on 2026-10-13. Automated A1–A26 and manual M1 pass. Durable renderer and Settling contracts now describe shipped behavior; all acceptance boxes are complete. Brief moved to `done/`. Roadmap contains no entry for this brief.

Current remote main merges cleanly with the tested feature branch; no source integration changes were needed. Final edits only close documentation and acceptance state. Cargo format and diff checks cover these edits; completed full preflight and required-GPU results above remain the implementation proof.

Landing cleanup removes the session comparison bundle and dedicated Clippy target, cleans the heavily changed app/renderer packages from the primary target, and removes session-owned temporary tools and diagnostics. The comparison worktree was already archived cleanly with its baseline commit preserved on remote main. Preserve all owner walk logs and the separate local edit in this plan. Publish the feature branch and open a PR; do not merge it automatically. After owner reports merge, follow the build-brief merged cleanup procedure.
