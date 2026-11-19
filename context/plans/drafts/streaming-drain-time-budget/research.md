# streaming-drain-time-budget — research

Figures from `plans/done/streaming-drain-budget-spike/findings.md` (release, Radeon Pro 5300M Mac, warm staging pool) unless noted.

## What 4 ms admits
| Resource | Model (CPU, warm) | At 4 ms |
|---|---|---|
| Lightmap pair | 15 µs/pair + 0.19–0.20 ms/MiB | about 18 pairs of 1 MiB; 1 pair of 14 MiB (the showcase's largest) alone |
| SH chunk, drain + compose | 64–72 µs/chunk + 1.00–1.16 ms/MiB | about 3.3–3.7 MiB of 1 MiB chunks |

The SH figure assumes 1 MiB items. An SH item is a whole cluster (`DrainRank::sh` keys on `cluster_id`), and no measurement confirms the showcase's cluster sizes; one over about 3.5 MiB is admitted alone past 4 ms. The first slice records them.

- Lightmap gains: a 4 ms budget admits about 18 MiB of lightmap work, past the 16 MiB point where the sweep's in-hand wait fell to 0 (far-point late 180 → 145 and 147).
- SH loses per-drain throughput on bursts: production admits SH drains up to 7.9 MiB (`sh_max_drain_decoded_bytes`, showcase and campaign-test), about 8–11 ms. A 4 ms budget halves that.
- Sweep evidence on SH below production: at a 4 MiB byte cap, far-point late SH rose from 44 to 68 cluster-frames. That cap also cut lightmap work, so it overstates the time budget's SH cost. The SH late cluster-frames stayed read-in-flight at every cap, so SH admission was not their limit at production. Whether halving SH's per-drain bytes moves them is what the first slice measures.

## Why CPU
- The drain runs on the frame thread; its CPU cost lands in that frame (`render_sh_drain`, `lightmap_drain` stages, `rendering_pipeline.md` §12).
- GPU per drain was measured only at the install-cost tests' idle clock (median 64–286 MHz): lightmap about 42 µs/pair + 0.22 ms/MiB, SH copies 70 µs/chunk + 0.43 ms/MiB, compose 2.4 ms + 0.47 ms/MiB. Upper bounds, not a model to charge.

## In-hand coupling
- `MAX_IN_HAND_PAIR_BYTES = 4 × MAX_INSTALL_DECODED_BYTES_PER_DRAIN`; the band takes half (`lightmap_streaming/controller.rs`). The spike's sweep pinned both at their production totals while the drain cap moved.

## Prior owner rules
- One read issuer, one budget owner; mandatory before optional across resources; no second cap at a boundary (`plans/done/spatial-residency--lightmap-cell-blocks/index.md`).
- The 4 ms drain ceiling: a quarter of a 60 Hz frame (`plans/done/streaming-drain-budget-spike/plan.md`, gate rule "cap too large").
