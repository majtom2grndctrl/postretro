# Cluster-residency finding

**Status (2026-09-23): pure allocation and focused lifecycle proof is complete. GPU
residency, seam, and timing evidence is `not-yet-evaluable` on this host.** The bounded async
path remains the default for a validated id-49/id-50 pair; lack of adapter evidence does
not select `off`.

## Environment and artifact control

| item | value |
| --- | --- |
| branch / baseline | `feature/sh-probe-streaming` / `89b591fe394a975323c542ca47cb3b4e2849be83` plus this Task 13 working tree |
| Mac campaign source | `content/dev/maps/campaign-test.map`, SHA-256 `7dfde79ed7e792149008f0a0ae1229e8a84d869c7a32aecbf30cee984338dd38` |
| existing campaign PRL | `content/dev/maps/campaign-test.prl`, 202,309,938 bytes, SHA-256 `4eae4297e1b8f84ec22141dc7fa788f21d15c52679e05114d445f4cb1dc8a5c2` |
| disk gate | coordinator ran `cargo clean -p` for `postretro`, `renderer`, `level-loader`, `level-format`, `level-compiler`, `render-cpu`, and `render-data` while Cargo was idle (60,721 rebuildable files / 19.2 GiB reported); final focused-check free space: 21,797,720 KiB (about 20.79 GiB) |
| generated artifacts | none: no Task 13 PRL, report, PNG, cache, or cold-bake directory was created |

The campaign PRL above predates this check and was read only. Generated evidence would
have been confined to a unique session directory; none needed removal.

## Requested SH allocation report

The named pure fixture
`large_map_allocation_fixture_keeps_limited_visible_request_below_whole_load` models a
nondegenerate eight-cluster map and keeps the report ledgers distinct:

| report row | requested bytes | purpose |
| --- | ---: | --- |
| whole load | 536 MiB | eight 64 MiB cluster payloads (512 MiB), plus 8 MiB fixed metadata and 16 MiB whole-resident billboard scatter (ids 47/48) |
| streamed effective floor / active request | 264 MiB | 8 MiB fixed metadata + 16 MiB ids 47/48 + a 240 MiB active physical pool |
| limited-visible logical occupancy | 64 MiB | one visible cluster; recorded as a sub-ledger rather than a second physical allocation |

The fixture derives the 536 MiB whole-load value from its actual per-cluster vector, then
asserts both `536 MiB > 264 MiB` and `264 MiB < 536 MiB`. Its one-cluster target also
asserts 64 MiB logical occupancy independently. This is a CPU allocation proof, not a
claim that a GPU copy or a visual seam was observed.

The 16 MiB id-47/id-48 charge is called out separately so the streamable-payload savings
are not represented as savings from billboard scatter.

## Focused verification

| check | command | result |
| --- | --- | --- |
| allocation fixture | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro --features capture --bin postretro large_map_allocation_fixture_keeps_limited_visible_request_below_whole_load` | pass: 1 passed, 848 filtered |
| residency controller | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro --features capture --bin postretro sh_streaming --quiet` | pass: 26 passed, 823 filtered |
| async I/O / teardown | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro --features capture --bin postretro sh_async_workers --quiet` | pass: 2 passed, 847 filtered; covers the nonblocking positional reader and four-worker cancellation/join |
| verified id-50 loader | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-loader --lib sh_stream --quiet` | pass: 13 passed, 194 filtered |
| cold worker-count determinism | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build cluster_payload_spool_is_deterministic_across_dedicated_rayon_pools --quiet` | pass: 1 passed, 1,258 filtered; compares dedicated one- and four-worker Rayon pools |
| legacy output preservation | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build cluster_payload_keeps_legacy_bodies_and_excludes_billboard_scatter --quiet` | pass: 1 passed, 1,258 filtered |
| shader binding budget | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-renderer --lib pipeline_budget --quiet` | pass: 7 passed, 575 filtered |
| package check | `cargo check -p postretro` | pass; existing dead-code warnings only |
| formatting | `cargo fmt --all --check` after formatting the fixture | pass |

The controller coverage includes target reset, generation/tag/hash rejection, delayed
admission, bounded-permit accounting, and logical-versus-GPU ledger behavior. The worker
suite proves positional read/decode stays off the frame-side drain and that queued work is
cancelled and all four workers join. The loader coverage exercises verified id-49/id-50
input. The compiler's 1-versus-4-worker test is cold worker-count determinism without a
giant map bake; its legacy test protects the prior output bodies and scatter exclusion.
These are focused checks rather than the coordinator's workspace-equivalent preflight.

No live reload/teleport route or complete cold map bake was run: the former needs a sampled
adapter-backed level frame and the latter would create avoidable large artifacts. Existing
Slice 1/2 and Task 6–12 records remain the applicable prior deterministic/validation
evidence; this record does not relabel them as fresh Task 13 GPU proof.

## GPU, seam, and resource observations

- GTX 1660 Super large-map release/cold evidence was unavailable: no GTX adapter is
  exposed to this Mac host. It is therefore `not-yet-evaluable`, not a negative result.
- The permitted Mac campaign command was started with
  `POSTRETRO_SH_STREAMING=async cargo run -p postretro -- content/dev/maps/campaign-test.prl`.
  It was interrupted after 30 seconds while rebuilding, before engine launch, adapter
  selection, PRL loading, or a sampled level frame. Exit was 130; a subsequent process
  check found no `postretro` engine or `cargo run -p postretro` process. This attempt is
  `not-yet-evaluable`.
- Consequently no composed-atlas GPU readback, whole-versus-streamed live report,
  timestamp result, leak sample, or seam inspection image exists. No visual continuity,
  adapter-backed shader execution/budget observation, or release/cold performance claim is
  made here; the source-level pipeline binding-budget test above did pass.

The bounded command left no engine process running. Before the focused rebuild, free space
was 10,517,768 KiB (about 10.03 GiB), then 10,495,228 KiB, only about 9 MiB above the stop
threshold. The coordinator performed the scoped cleanup recorded above before focused
checks resumed; every resumed phase remained above the threshold, ending at 21,797,720 KiB.

## Manual follow-up when an adapter is available

1. Run the same map in valid `async`, `sync-proof`, and validated `off` modes; retain one
   PRL and keep id-47/id-48 whole-resident in each report.
2. Capture the separate GPU request/floor, logical occupancy, and encoded/decoding/ready
   CPU phase ledgers during whole-load and limited-visible views.
3. Teleport across cluster boundaries, inspect composed-atlas seams at each transition,
   and compare a cold rerun for deterministic bytes and report rows.
4. Capture shader binding/tap budgets and renderer retirement/leak evidence only after a
   sampled level frame is reached.

## Performance levers for further testing

The current values are starting points, not measured frame-time optima. Change one lever
at a time against a fixed map, route, resolution, adapter, and build profile. Record the
PRL hash and whether the run used `async`, validated whole-load `off`, or `sync-proof`.
Only `async` exercises eviction; `sync-proof` is a deterministic no-eviction proof path.

| Lever (current value) | Expected trade-off | Change scope |
| --- | --- | --- |
| Cluster partition (64 nonempty BVH primitives / 32 runtime cells) | Smaller clusters can reduce overfetch and improve eviction granularity, but increase chunk count, directory size, and duplicated halo coverage. Larger clusters reverse that trade-off. Geometry count is a proxy, not a bound on SH bytes or meters. | Compiler constants; rebuild PRL and remeasure the cluster/payload distribution. The original three-way dry run is in `cluster-directory-thresholds.md`. |
| Probe spacing (1.0 m default) and base-density fidelity | Coarser SH can lower bake, disk, and resident-payload costs, but changes lighting quality and the workload itself. Do not use a spacing change as an `off`-versus-`async` streaming A/B. | Existing compiler options `--sh-probe-spacing` and `--sh-density-fidelity`; rebuild PRL and perform visual checks. |
| Prefetch horizon (2 portal-cluster hops) and departure hysteresis (2 seconds) | More look-ahead/retention can reduce cold misses and doorway churn but keeps more clusters targeted or resident. Less can lower occupancy but expose late loads and popping. | Runtime policy constants; requires a code change, not a map setting. |
| Requested SH GPU floor (256 MiB including fixed metadata, whole-resident billboard scatter, and active pools) | A lower floor can save physical allocation but increase prefetch suppression, pool growth, or unavoidable overage. A higher floor can reduce pressure at a memory cost. The renderer may choose a smaller effective floor when the complete level cannot fill 256 MiB. | App/renderer policy constants; compare actual effective floor and pool capacity, not only the requested number. |
| Parallel stream permits/workers (4) and installs per drain (2) | More concurrency may shorten visible misses but raises concurrent I/O/decode buffers and install bursts; less reduces transient load at the cost of latency. | Runtime policy constants; keep the host-phase and per-frame bounds in view. |

First use a current id-49/id-50 campaign build to test movement, misses, ordinary
departures, and visual seams. It does not by itself establish a memory-pressure win.
For pressure testing, use a current-compiler build of a map whose visible/halo working
set challenges the *effective* floor; PRL file size alone is not that measure. Check
for ids 49/50 before treating any existing stress-map PRL as a streaming fixture.

For each A/B, capture frame-time distribution and transition hitches, visible misses,
installs/evictions, targeted and sampleable cluster counts, logical occupancy versus
physical GPU request/effective floor, pool growth/retirement, and encoded/decoding/ready
host high-water bounds. Inspect lighting at doorways and after fast travel. Prefer a
repeatable camera route; static capture alone cannot exercise async eviction. A knob is
worth changing only when those measurements identify its bottleneck and the visual
result remains acceptable.
