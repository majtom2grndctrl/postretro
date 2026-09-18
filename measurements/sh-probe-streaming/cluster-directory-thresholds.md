# Slice 2 cluster-directory threshold selection

**Status: defaults pinned (2026-09-18).** The selected compiler constants are
`primitive_limit = 64` and `cell_limit = 32`. This is compiler metadata tuning,
not evidence that SH streaming improves frame time; the Slice 1 premise remains
`not-yet-evaluable`.

## Protocol

One warmed, eight-worker compiler bake produced the finalized geometry, SH
metadata, CSR companions, and pack-time presence decisions. The dry-run test
then reused those immutable decoded products for every candidate; it did not
repeat global rays or mutate the bake.

| Control | Value |
| --- | --- |
| Map | `content/dev/maps/campaign-test.map` |
| Map SHA-256 | `7dfde79e6f9e73a91e988a173875932ab1c432d6be501fd90932222171f5b323` |
| Starting revision | `9f81b2ffb5ddd81f0d0fc0f7c883a4e51239603a` plus Task 4 working changes |
| Compiler mode | warm/default developer bake; cache enabled |
| Workers | `RAYON_NUM_THREADS=8`, compiler `-j 8` |
| Output | temporary `/private/tmp/postretro-sh-cluster-t4/campaign-test.prl` |
| Whole PRL | 174,659,235 bytes; SHA-256 `cc1b7f68182a988da366ec7334f5f1b9ef16ccfbe437cfcc986567921411c0d7` |
| Section 49 | version 1; 560,928 bytes; SHA-256 `c9322cd4550f7cf41aec23a17a0e473aa7a78cb0d61746cf02c81df97a7b9aa2` |
| Compiler elapsed | 199.35 s total; 0.16 s reported `ClusterDirectory`; 0.55 s packing |

Commands:

```sh
CARGO_TARGET_DIR=/Users/dhiester/Projects/Personal/postretro/target RAYON_NUM_THREADS=8 \
  cargo run -p postretro-level-compiler -- content/dev/maps/campaign-test.map \
  -o /private/tmp/postretro-sh-cluster-t4/campaign-test.prl --no-tui -j 8

POSTRETRO_CLUSTER_DRY_RUN_PRL=/private/tmp/postretro-sh-cluster-t4/campaign-test.prl \
  CARGO_TARGET_DIR=/Users/dhiester/Projects/Personal/postretro/target \
  cargo test -p postretro-level-compiler --bin prl-build \
  cluster_directory_threshold_dry_run_from_prl -- --ignored --nocapture
```

## Candidate distributions

Address counts are grid-relative logical units summed over all emitted resource
rows, not byte offsets. Dense and affinity maxima are the largest one-cluster
addressed sets. `owned/halo` makes boundary duplication explicit. Metadata
figures are conservative upper bounds that count canonical directory storage,
the two affinity bit vectors, one bounded tree node per covering reference, and
the largest per-cluster adaptive-node visited set. Serialization peak adds the
one encoded id-49 buffer; no SH-family payload is cloned or re-encoded.

| Primitive / cell limit | Clusters | Primitive p50 / p95 / max | Cell p50 / p95 / max | Directory bytes | Dense addresses / max cluster | Affinity addresses / max cluster | Affinity owned / halo | Cover refs | Construction upper bound | Serialization peak upper bound | Dry-run elapsed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 32 / 16 | 196 | 0 / 31 / 32 | 1 / 9 / 16 | 781,872 | 583,632 / 65,280 | 12,792 / 1,432 | 7,092 / 5,700 | 3,198 | 794,956 B | 1,576,828 B | 195.330 ms |
| **64 / 32** | **181** | **0 / 62 / 64** | **1 / 15 / 32** | **560,928** | **462,096 / 88,080** | **10,192 / 1,920** | **7,092 / 3,100** | **2,548** | **587,156 B** | **1,148,084 B** | **150.350 ms** |
| 128 / 64 | 166 | 0 / 0 / 128 | 1 / 6 / 64 | 367,920 | 409,200 / 104,592 | 9,044 / 2,304 | 7,092 / 1,952 | 2,261 | 416,636 B | 784,556 B | 126.488 ms |

No campaign cell was individually over budget. Many solid/exterior/disconnected
cells are required singleton clusters, which explains the zero primitive median
and the limited cluster-count reduction as limits increase.

The selected 64/32 midpoint cuts halo affinity addresses by 45.6% and directory
bytes by 28.3% versus 32/16, while avoiding the 128/64 candidate's 18.7% larger
maximum dense set and doubled cell bound. The result is nondegenerate (181
clusters), bounded, and does not claim an eventual resident-memory cap; Slice 3
must reconcile its floor with the observed maximum addressed set.

## Disconnected and budget-boundary fixture

The compact 12-cell synthetic graph has one connected eight-cell component,
three disconnected/zero-primitive cells, a frontier where an earlier candidate
does not fit but a later one does, and one indivisible 140-primitive cell. The
same three positive primitive thresholds produced:

| Primitive / cell limit | Clusters | Primitive max | Cell max | Flagged indivisible singletons | Directory bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| 32 / 4 | 8 | 140 | 3 | 1 | 496 |
| **64 / 8** | **6** | **140** | **4** | **1** | **400** |
| 128 / 16 | 5 | 140 | 8 | 1 | 352 |

Every disconnected cell remained assigned exactly once. The only threshold
exception was the deliberately indivisible 140-primitive singleton.

## Cold preservation spot check

The pre-Task-4 `gate-heavily-lit` artifact from
`cluster-directory-baseline.md` was compared with a directory-enabled bake
using the identical cold recipe (`--no-cache -j 1 --uncompressed-irradiance
--sh-density-force-scale 1`). All 22 pre-existing section ids, container-entry
versions, relative order positions, and body SHA-256 values matched exactly.
The only new body was id 49 v1:

| Evidence | Value |
| --- | --- |
| Baseline bytes | 30,480,375 |
| Directory-enabled bytes | 30,503,017 |
| Directory-enabled SHA-256 | `cf2aca25f95b0a4309bc8d764778d40d2c569eb3468ad38ce18fe6ca7a5e1bd7` |
| Id 49 SHA-256 | `376731e11d0b2f075261888e334b9df1e2bbf3144c338a83371af947a746d2e7` |
| Build elapsed | 49.66 s total; metadata < 0.01 s; packing 0.05 s |
| Existing bodies | exact match, no lossy exception |

This spot check also proved same-handle id-49 parsing before locked
publication. The full two-worker-count determinism and warm-cache proof remains
Task 7 scope.

## Artifact lifecycle

The temporary PRL is retained only through Task 4 verification and must be
removed before handoff. No PRL, PNG, or cache payload is committed. The source
map and this small record are the only durable measurement inputs/output.
