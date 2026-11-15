# lightmap-compact-storage — research

Measured on a shared 4-core box at 17c2f1e with spike builds. Showcase is the owner's showcase.map baked at 0.025 m (3,821 blocks, 22,085 faces); campaign-test is `maps/campaign-test.prl` (137 blocks). The walk harness is GPU-free with a warm page cache. Medians are over 3 interleaved runs.

## Lossless coding (ids 22/42)
| showcase | file | id 22 | id 42 | pack |
|---|---|---|---|---|
| raw | 1,152.2 MB | 522.6 MB | 215.1 MB | — |
| lz4 | 541.3 MB | 23.6% | 1.5% | 0.3 s |
| zstd-3 | 498.8 MB | 15.8% | 0.9% | 0.6 s |
| zstd-7 | 492 MB | — | — | +0.9 s |
| zstd-19 | 481.6 MB | 12.5% | 0.8% | 34.4 s |

- **Decode speed:** zstd-7 decodes as fast as zstd-3 or faster; levels 12 and above decode slower.
- **Integrity:** lz4 decoded 45 of 65 single-byte flips to wrong bytes with no error; zstd rejected all 65.
- **All-resident load, ids 22+42, one thread:** raw 942 ms, zstd-3 563 ms.
- **Campaign-test:** file 253.6 → 158.5 MB at zstd-3.
- **Frame-thread admit time:** about 95% lower (raw 627–904 ms per walk, coded 29–48 ms), because decode hands over per-plane buffers.
- **Showcase section sizes at 17c2f1e** (MiB of the 1,098.8 MiB payload): id 22 498.4, id 50 228.6, id 42 205.1, id 51 65.5. At the measured ratios, zstd-7 gives id 50 about 106 and id 51 about 4.6.
- **Id 50, per-chunk zstd-7:** 46.4% of raw size, decode about 6× per chunk on the SH pool. Whole-section coding would reach 38.6%, but chunks must stay independently readable.
- **Id 51, whole-section zstd-7:** 7.1% of raw size, about 63 ms decode at load.
- **Estimated showcase total** with packing and masks: about 300 MB.

## Decode placement (pinned realtime walks, zstd-7)
| cores | walk | issuer busy p95/max ms: raw / on issuer / on pool | late blocks: on issuer / on pool |
|---|---|---|---|
| 1 | random | 2.34/14.6 · 5.97/37.5 · 0.22/1.6 | 158 / 158 |
| 1 | far-point | 1.98/8.5 · 5.05/18.6 · 0.18/2.3 | 427 [427–511] / 427 [375–427] |
| 2 | random | 2.54/10.3 · 6.24/25.5 · 0.26/4.1 | 158 [158–167] / 158 [158–163] |
| 2 | far-point | 2.19/9.0 · 5.23/20.3 · 0.22/2.5 | 427 [270–427] / 427 [426–427] |

- **Raw baseline:** 427 and 158 in every run.
- **Effect of the pool:** it removes the issuer stall. Spread across runs remains, because at 1–2 cores the pool has one thread.
- **Completed-mode gate:** installs, uploads, late blocks and the payload hash are identical across raw, decode on the issuer and decode on the pool.

## Packing and layout
- **Placement efficiency:** 76.7% at HEAD; 96.0–96.7% with transposed placement, alignment 2 and multi-heuristic placement with trim.
- **Bit-identity:** transposed placement with alignment 2 is bit-identical before compression across 5 layouts, for irradiance, direction, masks and weight maps. Alignment 1 breaks the half-resolution direction plane.
- **Owners don't depend on layout:** the owner tables of the HEAD and repacked PRLs differ on 0 of 22,085 faces.

Showcase pool bound, simulated with the bound's own placement code and real owners:

| layout | lightmap | masks | total |
|---|---|---|---|
| HEAD | 62 layers | 42 layers | 550 MiB |
| repacked, whole-block masks | 294 MiB | 140 MiB | 434 MiB |
| owned-first, real rebake | 300 MiB | 96 MiB | 396 MiB |
| separate masks, plane B at the whole mask block | 294 MiB | 92 MiB | 386 MiB |
| separate masks, plane-B sub-rect | 294 MiB | 84 MiB | 378 MiB |

- **Campaign-test, separate masks:** 100 MiB, against 112 MiB with whole-block masks. The lightmap is capped at all-resident.
- **Mask chart alignment** of 1, 2 or 4 makes no difference.
- **The constant-mask-plane flag** was measured exact but saves only 550 → 542 MiB, so it was dropped.

## Pool headroom (completed mode; late blocks)
| PRL | walk | 5/4 | 6/5 | 9/8 | none |
|---|---|---|---|---|---|
| HEAD | far-point | 121 | 295 | 770 | 1435 |
| HEAD | random | 77 | 43 | 79 | 141 |
| repacked | far-point | 482 | 565 | 1035 | 1696 |
| repacked | random | 14 | 78 | 7 | 154 |

- **Repacked mask pool:** fixed at 43 layers for these runs. The runtime still placed whole-block planes, so only the lightmap pool was swept.
- **Larger lightmap pools:** at 59 layers the repacked layout has 56 far-point and 0 random late blocks. All-resident has 0.
- **No safety failures in any run:** no growths, deferrals or failed reads. Every late block was one admission left behind after band evictions.
- **Removing headroom** roughly triples repacks and the worst compaction plan.
- **Realtime mode** follows the same trend; the tables are in the spike data.

## Ordering pins
| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | owned face outside the plane-B sub-rect | mask block packed | the face owns no channel 2 or 3, so it never samples plane B or reads a clamped neighbour |
| P2 | no SH decode pool (SH off, sync-proof, or no id 50) | level load, lightmap streams | lightmap still decodes off the issuer |
| P3 | optional decodes queued, one running | other resource's mandatory pair delivered on the same tick | mandatory taken next; waits only for the running job |
| P4 | band pair raised to mandatory after its read was delivered | raise while the first job is queued or decoding | decodes at most twice, charged once, duplicate completion released |
| P5 | SH session replaced mid-level, lightmap kept | lightmap pairs queued on the pool | each released to the kept session and re-requested later; ledger reaches zero |
| P6 | lightmap declined mid-level | decodes queued | parked session releases every pair; SH unaffected |
| P7 | block leaves its target | while its decode is queued | skipped or dropped, charge released, read again if it returns |
| P8 | bilinear mask sample at an owned chart's border | any draw | reads that chart's padding ring, never a neighbouring mask chart |
| P9 | block with no owned face | bake and load | no plane stored, no mask placement or read, mask extents 0 |
| P10 | block carried across a bake-layer close | last cut face finishes after the close | mask layout and plane-B set fixed only after the last face finishes |
| P11 | block evicted and placement re-tenanted in one drain | table write | both words written in one write; no stale mask address sampled |

## Follow-ups
| item | where |
|---|---|
| Unread SH bodies in ids 34/35/41 | suggested session task `task_effc5bbc` (not yet a plan) |

## Rejected rivals
- **Owned-first packing** (charts of mask-owning faces packed first inside the lightmap layout) measured 396 MiB in a real rebake. It makes the lightmap layout depend on lights. A light edit can flip the owner estimate, repack the cell, change the fingerprint and miss every light's layer memo; a showcase cold bake took 1,156 s. Keeping it cheap would need chart-local layer records, a cache-format break across 10+ files.
- **A second UV in the world vertex** would bump id 17 and change every pipeline that binds world vertices. Atlas preparation also assigns UVs before owners exist.
- **Fitting mask addressing into today's 16-byte block entry** leaves no room for a plane-B sub-rect and caps layers at 2048.
