# SH streaming authored-hints integration evidence

**Status (2026-09-23): correctness and deterministic-artifact evidence are
complete. Manual GPU/visual evidence is `not-yet-evaluable` on this desktop
sandbox.** That limits later tuning observations only; it does not weaken the
format, compiler, loader, controller, or renderer correctness gates below.

## Controls and artifact lifecycle

| Control | Value |
| --- | --- |
| Revision | `8c8604a30824ea86d62354119c1b8ca64f36265f` plus Task 6 evidence/docs worktree changes |
| Source fixture | `content/dev/maps/sh-streaming-hinted-door.map` |
| Source SHA-256 | `6dde26e29bf70f01a047078b2cafa9ad86bfeef6fe501709c8bad0422091fe88` |
| Probe spacing | `4` meters, explicitly supplied on every bake |
| Cache mode | `--no-cache` |
| Committed loader fixture | `content/dev/maps/test-fixtures/sh-streaming-hinted-door.prl` |
| Fixture SHA-256 | `37a50c71755cc6533d9a381b025e609f7896ed10ec9829a9ba34893d75fda6b0` |
| Fixture bytes | 25,561 |
| Temporary directory | `/private/tmp/postretro-sh-hints-task6.3CC2BA` (owned by this check; removed after recording) |

The production compiler binary was already built as
`target/debug/prl-build`. The explicit cold commands were:

```sh
target/debug/prl-build content/dev/maps/sh-streaming-hinted-door.map \
  --sh-probe-spacing 4 \
  -o /private/tmp/postretro-sh-hints-task6.3CC2BA/a.prl --no-cache
target/debug/prl-build content/dev/maps/sh-streaming-hinted-door.map \
  --sh-probe-spacing 4 \
  -o /private/tmp/postretro-sh-hints-task6.3CC2BA/b.prl --no-cache
```

Both direct bakes completed in under 0.1 seconds and emitted only the expected
fixture warning that, with no baked light entities, static world geometry uses
the white-lightmap placeholder. That warning is intentional fixture setup, not
a streaming error. `cmp -s` confirmed `a.prl == b.prl` and that both equal the
committed fixture byte-for-byte. Each file is 25,561 bytes with the fixture
SHA-256 above.

## Exact id-49/id-50 evidence

The compiler test helper `section_from_container_meta` reads the production
`ContainerMeta`, validates its table bounds, finds the section entry, and
slices only the entry's validated `(offset, size)` range. It deliberately does
not use an extraction CLI. The focused cold-bake test compares those exact
id-49 and id-50 bytes for two fresh pipeline bakes and the committed fixture.

| Section | Entry version | Offset | Bytes | SHA-256 |
| --- | ---: | ---: | ---: | --- |
| id 49 `ClusterDirectory` | 2 | 18,781 | 1,272 | `8cb8691d06060bdeabcb7e1185706f51942e6739a1d85c9a430f4388a4da78c1` |
| id 50 `ClusterShPayloads` | 1 | 20,053 | 5,508 | `5f0b81da7dad3a5d495550552a82d14026964897deda33dbcb321c5f6b421c9f` |

The v4 PRL contains 18 sections. The hinted directory holds sorted seam portal
IDs `20..=25`; its policy records pin cluster 14 and assign priority 3 to
clusters 6, 7, and 8. The direct far endpoint selected by the pinned-side
doorway seam remains unpinned. The priority region and direct far endpoint are
separate contracts: priority covers neighboring far-room clusters, while the
seam planner warms the exact opposite endpoint.

## No-hint preservation and controller diagnostics

The fixed no-hint compiler test retained cell membership `[0, 1]` and one
cluster range `(member_start=0, member_count=2)`. It now hashes id-50 emitted
from that same production-baked canonical directory:
`60287285b1ffcbae6c889974bded26085e18df4a220bc3b528651b7b83090d47`.
The earlier `817ab1de29cad06ddb23245954f7617fe8f275ae96be4e4237c480800045d960`
hash remains frozen as a separately named synthetic two-cluster sparse-row
codec golden; it was not the hash of this canonical no-hint partition.

The normalized controller trace intentionally excludes wire version,
generation, and content tag. It still matches the Slice 3 baseline:

| Tick | Class / targets | Requests | Suppressed | Evictions |
| --- | --- | --- | --- | --- |
| Near cell 0 | `0: Visible; 1,2: Prefetch` / `[0,1,2]` | `[0]` | `[]` | `[]` |
| Pressure | `0: Visible` / `[0]` | `[]` | `[1,2]` | `[1,2]` |
| Recovery at cell 3 | `0: Hysteresis; 1,2: Prefetch; 3: Visible` / `[0,1,2,3]` | `[3,1,2]` | `[]` | `[]` |

The GPU-free pin regression keeps the pin plus owner closure targeted under
pressure, performs no eviction, and reports a 15-byte non-evictable logical
overshoot. The separate renderer-floor regression reports the named
`GpuCapacity` failure when growth is impossible; it does not evict a pin. These
small synthetic values are policy diagnostics, not a claim about live GPU
allocation on this host.

## Focused gates

| Gate | Command | Result |
| --- | --- | --- |
| Hinted compiler → format → loader cold round-trip | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build hinted_doorway_compiler_format_loader_round_trip_is_deterministic -- --nocapture` | pass: 1; two `--no-cache` bakes and exact id-49/id-50 ContainerMeta comparisons |
| No-hint id-50/membership golden | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build no_hint_two_cell_fixture_preserves_id50_hash_and_cell_membership -- --nocapture` | pass: 1 |
| Shared v2 directory validation | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-format cluster_directory -- --nocapture` | pass: 18; one unrelated ignored manual helper |
| Validated id-50 loader path | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-loader sh_stream -- --nocapture` | pass: 13 |
| No-hint runtime trace | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro no_hint_controller_trace_preserves_target_request_suppression_and_eviction_order -- --nocapture` | pass: 1 |
| Closed-door real visibility | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro compiled_hinted_doorway_keeps_closed_visibility_and_warms_far_seam_endpoint -- --nocapture` | pass: 1 |
| Impossible pool growth | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-renderer impossible_streamed_pool_growth_is_a_named_gpu_capacity_error -- --nocapture` | pass: 1 |
| Renderer binding contract | `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-renderer forward_pipeline_sampled_texture_request_matches_bgl_definitions -- --nocapture` | pass: 1 |
| Formatting | `cargo fmt --all --check` | pass |

## Bounded manual smoke

`cargo build -p postretro` completed before the check and produced
`target/debug/postretro`. At engine launch, a separate watchdog was armed to
send `TERM` at 55 seconds. The engine was launched against the committed hinted
PRL with stdout/stderr redirected to the owned temporary directory. After the
check, `lsof` found no process holding the smoke log and `lsof -c postretro`
returned no engine process, so the process was stopped and reaped within the
60-second bound.

The 479-byte log contained only macOS LaunchServices/XPC failures. It did not
reach observable adapter selection, PRL loading, an SH compose frame, or a
visual doorway view. Therefore seam-pop observation, frame time, live pool
accounting, and rendered fallback continuity are **not-yet-evaluable on this
host**. This is not evidence against shipping the capability: the deterministic
and behavioral gates above remain the feature criteria. On an adapter-capable
machine, use this same fixture and mode with a repeatable closed/open doorway
route to tune timings, pool pressure, and visual continuity.

## Integrated boundary review

The reviewed path is coherent with the intended one-way contracts:

1. Compiler-only convex regions resolve only after final portals/cells, then
   feed seam-constrained canonical id-49 partitioning and rebuilt id-50 chunks.
2. Shared format validation recomputes canonical membership from persisted seam
   IDs before the loader admits the directory.
3. Loader retains validated portal endpoints with cluster adjacency in the
   manifest; planner topology consumes those endpoints and hint records.
4. Planner adds `SeamWarm` without altering render `VisibleCells`, keeps pins
   and owner closure protected, and orders optional work by effective priority.
5. Renderer receives only generation-matched drain batches before compose;
   shader bindings, portal traversal, and the next-drain sampling promotion
   contract remain unchanged.

No additional cross-boundary gap was found in this review. The durable compiler
and renderer contracts now document these semantics; the FGD and level-design
reference document their authoring surface.
