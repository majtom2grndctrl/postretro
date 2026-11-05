# spatial-residency--sh-bounded-pool research

Raw data was discarded after this summary. Spike walk, shot and diff code stays on local branch `spike/memory-measure-tour`. One Mac, Radeon Pro 5300M 4 GB, Metal.

## Allocation, hallway, today

`Metal Resource Events`, live at exit: 3,011 MiB. Peak 3,577 at t = 24 s, old and new pools alive together. `ioreg` in-use released at exit: 2,953 and 2,975.

| Consumer | MiB |
|---|---|
| SH dense | 1,300 |
| SH sparse | about 140 |
| Lightmap pool | 490 |
| Shadow pools, spot and cube | 427 |
| Everything else | about 650 |

The driver's in-use plus free falls short of the card by up to 1,400 MiB on big maps, but Metal's list matches in-use, and the gap appears with no engine running. Treated as driver-held. A real 2 GB card was not tested.

## What each change bought, hallway

2 km, 7 m/s, 225 shots.

| Setup | Live p50 | Peak slots | SH pool MiB | Misses |
|---|---|---|---|---|
| Today | 403,876 | 712,475 | 1,440 | 323 |
| Hold only | 403,580 | 604,289 | 1,440 | 0 |
| Reference counts only | 131,472 | 483,725 | 1,440 | 577 |
| Both | 129,538 | 244,500 | 720 | 0 |
| Both, 3-layer cap | 129,538 | 244,524 | 559 | 0 |
| Both, 2-layer cap, first-come | 124,344 | 203,514 | 397 | 2, plus 1 refusal |
| Both, 4 cells per cluster | 104,994 | 197,914 | 397 | 0 |
| Both, 1 cell per cluster | 89,161 | 172,281 | 397 | 0 |

**Misses are not comparable across the hold.** The spike hands the targeter an empty visible set on non-portal frames, and misses are counted from that set, so a hold row's zero means misses there went uncounted, not that they stopped. Zero on portal frames is real. On the hallway's 44 fallback frames, up to 1,002 cells were drawn with 52 to 158 clusters targeted; whether those surfaces were lit is unmeasured. The spike also retains nothing extra, so it is not the hold this brief decides.

PRL growth: 4 cells +1.8%, 1 cell +8.6%. Evictions 657, 1,227, 3,675. All at the 60 Hz cap; no frame over 100 ms with reference counts.

## Bound

Worst mandatory demand over all camera cells, L = 16 m, closure removed, cluster grain, today's limit.

| Map | Slots | Whole map |
|---|---|---|
| Hallway | 168,017 | 731,008 |
| `campaign-test` | 39,634 | 41,726 |
| `occlusion-test` | 18,327 | 18,327 |
| `stress-street` | 117,707 | 117,707 |

Two layers hold 203,522 slots. At L = 32 m the hallway is 216,952, past two layers, so the default lead matters.

## Id 51 against what was seen

Four-direction walks, level camera, median slots at cell grain.

| Map | All around, here | All around, next 28 m | Id 51, L = 16 m |
|---|---|---|---|
| Hallway | 14,290 | 29,018 | 50,763 |
| `campaign-test` | 18,931 | 28,296 | 28,966 |
| `occlusion-test` | 17,621 | 18,113 | 18,159 |
| `stress-street` | 107,349 | 112,352 | 117,068 |

Drawn cell entries outside id 51 at L = 16 m: 4 of 126,344 on `stress-street`, none elsewhere. Halving L left SH live slots unchanged.

## Other maps

Reference counts and hold cost nothing visible, and save nothing: all three stay almost whole-map resident. `stress-street` with 4 cells cost 21 MiB of pool and 5.7% PRL, which is why smaller clusters are not a default. `campaign-test`'s 14 misses stopped being counted under the hold (see above), and four shots darkened evenly by 1 to 2.6 of 255. A hold-only run reproduces it, so the hold causes it, and it may be real loss the count cannot show. Candidate, unmeasured: `sh_probe_weight` renormalises over surviving corners, so a surface can lose a neighbouring probe and shift slightly.

## Limits

- One hallway route; the tour's straight lines enter solid cells, and it opens no doors.
- `stress-street` is the only large non-warren map, and it is generated. No large hand-built interior map exists.
- Shot 0011a varies with timing near `warren_lift_0`, independent of these changes.
- Without labels, buffer names come from sizes. The 1.56% over 96 × 1024² × 4 B and 36 × 512² × 4 B identifies the shadow pools.

## Ordering pins

| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| R1 | First reference while the last is freed | A evicted, B installed on one drain, both orders | One live copy, data intact, no stale zero word |
| R2 | Evict and re-request | Same frame, slot re-tenanted | Resident, references counted once |
| R3 | Read in flight for a victim | Evicted to admit mandatory work, then read completes | Read dropped, nothing retained |
| R4 | Partial install failure | After k of n references, including a first-written node | Node freed, every count restored |
| R5 | Optional eviction behind reader epoch | Slots retiring while mandatory work waits | No per-frame growth or further eviction; installs when freed |
| R6 | Growth with retirement pending | Growth, then epoch passes | Contents kept until the epoch, then freed |
| R7 | Portal, fallback, portal | Fallback run longer than hysteresis | Residents not aged out, not dropped on exit |
| R8 | Teleport during hold | Camera cell jumps while held | Mandatory demand follows the new cell |
| R9 | Settling ends on a fallback frame | Full demand falls to hold | What Settling loaded stays |
| R10 | Reload mid-flight | Reads in flight, retirement pending, references live | Nothing from the old level remains |

## Alternatives not taken

| Option | Why not here | Status |
|---|---|---|
| Slim the slot | 1,664 B resident against 128 B at rest; dropping `direct_intermediate` might save about 150 MiB, droppability unproven | Next lever, multiplies with this brief; could stand in for smaller clusters |
| Direct owners only | No wire change, but modelled p50 144,489 against 81,410 with no owners, and keeps the ordering problem | Rejected on numbers |
| Brick- or cell-keyed demand | Modelled p50 51,873, lower still, but replaces the cluster unit | Larger change; revisit if this bound falls short |
| Fix slots and growth first, references later | Hold alone leaves live p50 flat; references take it from 403,876 to 131,472 | Rejected on measurement |

## Direction review

`/validate-plan` read at 2851e10fb: Direction sound. It treats three repo commitments as load-bearing: the 2 GB target (nothing player-visible failed on a 4 GB card), no full PVS bake (so the bound is sampled and the growth net is needed), and pre-release with no compatibility. One worst cell sets the pool for the whole level. Restoring owners would cost a rebake and about 200 lines of runtime diff, so this is not a one-way door.
