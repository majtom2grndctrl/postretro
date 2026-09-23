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
