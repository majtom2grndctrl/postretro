# Findings: play budget sweep

Settles the brief's blocking open question: does a 4 ms play budget cost SH pop-in, and if so does the budget rise? Measure-and-report (`context/lib/experimental_spikes.md`); the owner picks the value.

## Answer

**On both bakes, no play budget from 3 to 8 ms changes pop-in.** Late lightmap block-frames after the spawn step are identical at every budget and at the 8 MiB byte cap: showcase 27 far-point and 20 random, campaign-test 0. Late SH after the spawn step is 0 on every walk at every budget. Not one late item after the spawn step is held by the drain budget or the in-hand cap. Every one is a read still in flight, or, on the showcase random walk, 2 block-frames waiting on a read permit.

**What the budget moves is the cost of the worst drain.** That cost has a floor: the largest single SH cluster. The first item of a drain is always admitted, so a cluster costing more than the budget drains alone. The showcase's largest cluster is 4.43 MiB, an estimated 4.85 ms. Campaign-test's is 5.0 MiB, 5.47 ms. Below about 5 ms, a lower budget does not lower the worst drain. It only adds lone over-budget drains and same-class cuts.

| Play budget | Worst drain, showcase | Worst drain, campaign-test | Lone over-budget SH drains, showcase far-point / random | Same-class cuts, showcase far-point / random |
|---|---|---|---|---|
| 8 MiB byte cap (today) | 8.68 ms | 8.74 ms | — | 78 / 21 |
| 3 ms | 4.85 ms (lone) | 5.47 ms (lone) | 19 / 5 | 117 / 37 |
| 4 ms | 4.85 ms (lone) | 5.47 ms (lone) | 3 / 1 | 72 / 22 |
| 5 ms | 5.00 ms | 5.47 ms (lone) | 0 / 0 | 38 / 16 |
| 6 ms | 5.99 ms | 5.47 ms | 0 / 0 | 25 / 13 |
| 8 ms | 7.95 ms | 6.89 ms | 0 / 0 | 11 / 6 |

Worst drain is the estimate's maximum (`drain_estimate_us_max`), far-point walk. On the random walk it is 4.85, 4.85, 4.97, 5.97 and 7.92 ms from 3 to 8 ms. Medians of 3. Repeats agreed exactly on every late count and every worst drain; same-class cuts varied by at most 1.

**Lightmaps drain in fewer, larger batches.** The 4 ms budget admits the same lightmap items as the byte cap, 46,940 against 46,898 on the showcase far-point walk, in 44% fewer drains (4,215 against 7,531). Pop-in does not change, because no lightmap was waiting on the budget at the byte cap.

## Recommendation

**Owner decision, 2026-11-19:** the play budget is 4 ms, and the showcase acceptance row is re-based on the byte cap of the same bake. Both are recorded in `index.md`.

| Question | Recommendation | Basis |
|---|---|---|
| Play budget | **4 ms holds; no rise.** The SH pop-in the open question feared does not appear on either bake. 5 ms gives the same worst drain on campaign-test and 0.15 ms more on the showcase, and buys only fewer cuts, which cost no pop-in here. Above 5 ms the worst drain grows with the budget and buys nothing | Late items identical at every budget; worst drain floored by the lone SH cluster |
| Lone SH clusters | **Expected, and the follow-up's case.** At 4 ms the showcase drains 3 clusters alone (far-point) at up to 4.85 ms, and campaign-test 1 at 5.47 ms. Splitting large clusters belongs to the warm-set follow-up the brief already names | `sh_item_bytes_max` 4.43 and 5.0 MiB |
| Acceptance row thresholds | **Re-base on the byte cap of the same bake.** The brief's "lightmap at most 150, SH at most 44" came from the spike's showcase bake, which is lost. The current showcase runs 27 and 0 at the byte cap. Write the row as "no worse than the byte cap on the same bake" | Fixture below |

## Spawn step (settling only)

The walk admits its level-entry burst at the play budget. Production settles that burst at the settling budget and draws nothing (`walk_measurement/budget_tallies.rs`). So these figures do not show player-visible pop-in. They bear only on the settling budget, which the brief delegates.

| Play budget | Byte cap | 8 ms | 6 ms | 5 ms | 4 ms | 3 ms |
|---|---|---|---|---|---|---|
| Showcase SH cluster-frames late at the spawn step | 750 | 754 | 776 | 782 | 829 | 1,034 |

Campaign-test's spawn step is 5 at every budget.

## Limits

- **The cost is estimated, not measured.** Every per-drain time here is the spike's install-cost model: lightmap pair 15 µs + 195 µs/MiB, SH cluster 68 µs + 1,080 µs/MiB. The walks are GPU-free, so they measured no install CPU and no stutter. The brief's manual row on the playback route checks the model.
- **This bake does not reproduce the spike's limit.** On the spike's showcase bake, the in-hand wait was 73% of late lightmap on the far-point walk. Here it is 0 at every budget. So this sweep cannot confirm the spike's lightmap gain from larger drains. It shows only that the time budget costs no pop-in where reads are the limit.
- **These are two maps.** Neither represents real content yet. A map where the budget does hold items back would turn the trade back on, and the brief's diagnostics would show it.

## Fixture and method

- **Showcase.** The adjacent game project's showcase map, current version. The `.map` hashes `4a95234a69ae6ef9…` (sha256), baked with `--lightmap-density 0.0125 --no-cache -j 6` in 6,953 s. The PRL is 787 MB and hashes `0fac5bb6e07fb0e7…`. The bake warns that lightmap pool demand is 1,540 MiB against a 384 MiB budget, and SH mandatory memory 534 MiB against 512 MiB. The spike's frozen showcase bake is lost, so its figures are not compared here.
- **Campaign-test.** `content/dev/maps/campaign-test.map` at default density, baked in 125 s. The PRL is 115 MB and hashes `4cae0756bec4ed37…`.
- **Builds.** `prl-build` and `scripts-build` at `c8497e6dd`. The walk harness (`lightmap_residency_walks_from_prl`, release, `--ignored`) at `4e0194cc0`, which adds the test-only `POSTRETRO_LIGHTMAP_WALK_DRAIN_TIME_US` lever.
- **Honesty check.** With the lever unset, the lever build matched the build before it on every result key except timing: 117 shared keys, campaign-test, completed reads, SH on and off. So unset means production.
- **Runs.** Realtime reads (16.7 ms per frame), SH on, default cores, both default walks per invocation. Configs: the byte cap, then 3, 4, 5, 6 and 8 ms. 3 repeats, spread across configs rather than run back to back. The machine ran nothing else. 36 invocations, all valid.
- **Figures** are after the spawn step unless a section says otherwise. "Lone over-budget" counts drains whose single admitted item costs more than the budget. For the byte cap it is measured against 4 ms.
