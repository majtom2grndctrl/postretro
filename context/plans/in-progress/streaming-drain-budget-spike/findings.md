# Findings: streaming drain budget

Task 9 of `plan.md`. Measured values for the brief's measured rows, and a recommendation for each of the three assumptions. Findings are measure-and-report, not gates (`context/lib/experimental_spikes.md`).

## Answer

**8 MiB is inherited, not measured, and it is not the main limit.** It was SH's first guess, `69774368c` (`sh-streaming--warm-set-and-io-contract`, which listed it among "first guesses to be tuned from the new diagnostics on real content"). `fd3342f28` (`spatial-residency--lightmap-cell-blocks` Task 6) carried it over unchanged as the shared budget, so SH would behave as before.

On the showcase the budget itself holds back almost nothing: 2.2% of late lightmap block-frames and 0% of late SH cluster-frames at the production cap. What it does hold is in-hand bytes. Ready pairs the budget leaves behind fill the in-hand cap, which is four drain caps. The next demanded read then waits. That wait is 73% of the far-point walk's late lightmap block-frames.

Raising the drain cap to 16 MiB clears the in-hand wait and cuts late items on the far-point walk: lightmap 180 → 145 and 147 block-frames, SH 44 → 37 cluster-frames. The binding limit then moves to the read permits. Past 16 MiB a larger cap buys less: 32 MiB cuts the far-point walk a further 12% (to 130 and 127) and the random walk by 3 block-frames.

The cap cannot simply be raised, because one byte does not cost the same for both resources. A warm lightmap install costs about 0.20 ms/MiB of CPU; an SH install costs about 1.0–1.3 ms/MiB, five to six times as much. A 16 MiB lightmap drain of the showcase's shape costs about 3.5 ms, inside the 4 ms ceiling. But the showcase already admits SH drains of 7.6–7.9 MiB at the production cap, about 8–11 ms of SH install, and up to 10.8 MiB at 16 MiB, about 11–15 ms (the walk's largest SH share of any drain, `sh_max_drain_decoded_bytes`). The byte cap is too small for lightmaps and too large for SH. The GPU trace points the same way, at an idle clock: per MiB, SH's upload copies cost twice lightmap's before the compose they trigger (*Install cost*).

## Recommendations

| Assumption | Recommendation | Basis |
|---|---|---|
| Cap size | **Change, inside the byte-unit brief, not as a direct build.** Lightmap work per drain should rise to about 16 MiB. A shared byte cap of 16 MiB would also let SH drains reach about 11 MiB, roughly 11–15 ms of CPU. Avoid 32 MiB: near-cap lightmap drains are exposed to cold restaging | Sweep; install cost |
| Byte unit | **Change: new brief.** Charge each resource for its estimated install time, fixed cost per item plus cost per MiB, instead of decoded bytes. Then size the budget in time (about 4 ms) | SH costs 5–6× a lightmap byte per MiB; SH drains already reach 7.9 MiB at the production cap on both maps |
| Tie-break (SH first at equal class) | **Keep.** | Same-class cuts 33–48 per showcase walk, under the 100 that would run the severity read. No late SH cluster-frame is held by the budget, and completed far-point is 79 late blocks with SH off against 78 with SH on |

**Other levers matter more than the cap.**
- **The in-hand cap** is where the far-point burst waits at production. Raising it alone does not help: at 64 and 128 MiB the in-hand wait falls to 0 and the permit wait rises from 0 to 131, so the total stays at 179–180.
- **The read permits** (32) become the limit once in-hand bytes free up, at either lever. A permit lever was not added: the decode and completion queues are sized to the permit count, so raising it changes a resource bound. It belongs in the follow-up that retunes the cap.
- **Outside reach** is 251 of the random walk's 306 late block-frames at every cap. No drain or in-hand setting moves it; it is the lead distance against the walk's speed.
- **Decode at one core** does not change the picture: far-point 180 late block-frames at default and at one core, SH 44 and 47.
- **The staging pool** discards a buffer just larger than a request when the two cannot both be kept under its 64 MiB limit (`acquire_inner`). It bites only for certain pairs of size classes: a request class at or under 32 MiB beside a free buffer larger than both it and 64 MiB less it, such as 32 and 36 MiB. Classes come from staged bytes, which pad each row to 256 B, not from decoded bytes. In the install-cost run, whose shuffled shapes mix sizes, drains alternating between the 32 and 36 MiB classes created a buffer on nearly every drain and ran at the cold rate, about 59 ms each. Back-to-back near-cap drains in play were not measured: at a 32 MiB cap the sweep's admitted-bytes p95 is 15.8 MiB. So a 32 MiB play cap exposes its near-cap lightmap drains to cold restaging. SH stages per chunk (at most about 4.6 MiB per acquire) and never reaches those classes. The settling cap is 32 MiB, so lightmap-heavy settling drains that straddle 32 MiB of staged bytes on consecutive drains could pay the same; no settling run measured it. A small direct fix, separate from the budget.

## Gate decision (AC 16)

Thresholds were stated in `plan.md` before the matrix. Medians of 3, realtime, SH on, after the spawn step.

| Side | Threshold | Measured | Result |
|---|---|---|---|
| Cap too small | Budget holds > 10% of a resource's late items | Lightmap 4 of 180 block-frames (2.2%) far-point, 0 of 306 random; SH 0 cluster-frames on every walk | closed |
| Cap too large | Largest multi-item drain > 16 MiB (4 ms at 0.25 ms/MiB) | Every top-5 showcase drain is a single lightmap pair (14.0 MiB far-point, 9.68 random); multi-item drains stay at or under about 8 MiB (p95 7.95–7.99) | closed |
| Same-class cuts | > 100 per showcase walk | 33–48 | closed; no severity read |
| Other evidence | In-hand wait > 25% of late lightmap block-frames | 132 of 180 (73%) far-point | **open**: the sweep runs |

## Matrix (AC 15)

Realtime medians of 3, SH on, after the spawn step. Repeats agree within 2 block-frames.

| Walk | Cores | Late lightmap block-frames (by cause) | Late SH cluster-frames | Same-class cuts | Largest drain |
|---|---|---|---|---|---|
| Showcase far-point | default | 180: in-hand 132, in flight 44, budget 4 | 44, all read in flight | 35 | 14.0 MiB, one pair |
| Showcase far-point | 1 | 180 | 47 | | 14.0 MiB |
| Showcase random | default | 306: outside reach 251, permit 44, in flight 7, in-hand 4 | 9, outside reach | 44 | 9.68 MiB, one pair |
| Showcase random | 1 | 306 | 9 | | |
| Campaign-test, both walks | both | 0 | 0 | 1 | |

SH's episode buckets equal its per-frame tally's causes on these walks. The spawn step is reported apart in each walk result (the `_spawn` keys) and is outside every share above. Raw walk results were kept in the session scratchpad, not in the repository.

## Cap sweep (AC 17)

Showcase, realtime, SH on, default cores, one lever moved at a time from production; `walk-sweep` at `e9b92a4f9`. Production repeated on that binary matches the matrix exactly. Points that moved the far-point total were run twice; repeats agree within 3 block-frames.

| Lever | Far-point late lightmap (budget / in-hand / permit / in flight) | Far-point late SH | Random late lightmap | Random late SH | Largest drain | Far-point admitted bytes p95 |
|---|---|---|---|---|---|---|
| Drain 2 MiB | 562 (40 / 292 / 181 / 49) | 491 | 414 | 176 | 14.0 MiB, one pair | 5.2 MiB |
| Drain 4 MiB | 322 (13 / 133 / 108 / 68) | 68 | 346 | 9 | 14.0 MiB, one pair | 6.1 MiB |
| **Production 8 MiB** | 180 (4 / 132 / 0 / 44) | 44 | 306 | 9 | 14.0 MiB, one pair | 7.9 MiB |
| Drain 12 MiB | 174 (3 / 34 / 91 / 46) | 37 | 299 | 9 | 14.0 MiB, one pair | 11.6 MiB |
| Drain 16 MiB | 145, 147 (0 / 0 / 86 / 59) | 37 | 298 | 9 | 16.0 MiB, 16 items | 15.6 MiB |
| Drain 32 MiB | 130, 127 (0 / 34 / 34 / 62) | 37 | 295 | 9 | 32.0 MiB, 26 items | 15.8 MiB |
| In-hand 16 MiB | 183 (1 / 137 / 0 / 45) | 44 | 306 | 9 | 14.0 MiB, one pair | 8.0 MiB |
| In-hand 64 MiB | 179 (6 / 0 / 131 / 42) | 44 | 306 | 9 | 14.0 MiB, one pair | 7.9 MiB |
| In-hand 128 MiB | 180 (6 / 0 / 131 / 43) | 44 | 306 | 9 | 14.0 MiB, one pair | 7.9 MiB |

Random-walk outside reach is 251 at every point. Drain CPU per cap is not measured by the walk, which stands in for the renderer; it is the install-cost model applied to admitted bytes, below.

Below production, late items rise on both resources and SH suffers most: at 2 MiB, late SH cluster-frames rise elevenfold on far-point. Above production, the in-hand wait falls to 0 by 16 MiB, and SH late falls from 44 to 37 from 12 MiB up. SH's late cluster-frames stay read in flight at every cap, so the SH gain comes through the shared read issuer and decode pool, not through SH admission; this run does not isolate which.

The number of frames that carry a drain falls as the cap rises (13,700 paced frames at production, 8,000 at 16 MiB, 7,500 at 32 MiB, of 76,525), so a larger cap trades more frequent small drains for fewer large ones.

## Install cost (AC 12, 13, 18)

CPU first; GPU under *GPU* below. Release build, Radeon Pro 5300M (Metal), warm staging pool, `--test-threads=1`, nothing else running. Tests: `lightmap_install_cost_model` (`renderer/src/lighting/lightmap/stream/install_cost_measurement.rs`) and `sh_install_cost_model` (`renderer/src/render/sh_streaming/gpu/install_cost_measurement.rs`), both ignored, sharing the fit and the cold classifier in `renderer/src/render/install_cost_model.rs`.

**Shapes.** Lightmap: 29 shapes, pair edges 64 to 1280×1216, drain totals 2–32 MiB, 1 to 780 pairs per drain, shuffled over 12 rounds (348 drains). SH: 23 shapes, chunks of 1, 4, 16 and 64 bricks (65 KiB to 4.1 MiB), 1 to 501 chunks per drain (276 drains). Item size and item count vary independently, so the fit separates per-item from per-MiB cost (AC 13).

**Cold drains (AC 12, P12).** A drain is cold when any staging pool it draws from creates a buffer during it, wherever it falls in the run; cold wins over every other verdict. Cold drains are left out of the warm fit and listed apart. A GPU-gated test runs drains of 4×64², 4×64², 1×1024², 4×64², 1×1024² on the real stream state and asserts cold, warm, cold, warm, warm: a drain larger than every earlier one is cold mid-run. Drains with pool growth, repack, deferral or upload-scratch regrowth are excluded and listed as such.

| Resource | Fixed per item | Per MiB | Runs | Spread |
|---|---|---|---|---|
| Lightmap pair | 15 µs | 0.19–0.20 ms | 4 | within about 4%; residual rms about 0.3 ms |
| SH chunk, install alone | 43–76 µs | 0.95–1.33 ms | 7 | about ±20%; one run 25% slow throughout |
| SH chunk, drain + compose | 64–72 µs | 1.00–1.16 ms | 2 (runs 6–7, batch built outside the timed span) | runs disagree by about 15% |

The lightmap per-drain term is small and unstable (−47 to 185 µs). SH fits with a per-drain term give a negative intercept, so the SH model is per item plus per MiB. The whole drain costs the same as the install counter once the batch is built outside the timed span, and compose adds about 0.2–0.4 ms per drain. Runs 1–5 timed a payload copy production never makes, inside the drain and drain + compose series; those series were re-measured in runs 6–7, and the install-alone series, which never included the copy, agrees across all seven runs. Within one SH shape the slowest of its 12 warm drains is often 2–3× the median; with 12 samples per shape that is one outlier, not a p95, and the cause was not found. Debug build: lightmap 642 µs per drain + 21 µs per pair + 0.17 ms/MiB; SH install 120 µs per chunk + 1.05 ms/MiB (runs 1–5; debug drain + compose not re-measured).

**Priced at the fit** (release, range over runs):

| Drain | Lightmap | SH install (runs 1–5) | SH drain + compose (runs 6–7, per item + per MiB fit) |
|---|---|---|---|
| 8 × 1 MiB (production cap) | 1.7–1.9 ms | 7.3–11.2 ms | 8.5–9.9 ms |
| One 14 MiB pair (the showcase's largest) | 2.8–2.9 ms | — | — |
| 16 × 1 MiB (16 MiB cap) | 3.4–3.6 ms | 16.1–22.1 ms | 17.0–19.7 ms |
| 26 items, 32 MiB | 6.8–6.9 ms warm; often cold at about 55–75 ms | 32–44 ms | 33.6–39.0 ms |

**Cold rate.** Lightmap cold drains run at 1.7–2.7 ms/MiB, about ten times the warm rate, as the brief expected. Each lightmap run has the same 52 cold drains: the first, and 51 near 32 MiB from the staging-pool behaviour under *Answer*. At 16 MiB the staging buffers are 16 and 20 MiB, which the pool keeps together, so no drain after the first runs cold. SH runs have 2–5 cold drains at 1.3–3.6 ms/MiB.

**In play.** The showcase walks admit SH drains of up to 7.9 MiB at the production cap, and campaign-test's reach 7.9 MiB too. At the fitted SH install rate that is about 8–11 ms of CPU, before compose. In-engine dev-build logs on the stress hallway recorded a 34 ms SH drain that grew no pool (`plans/done/sh-streaming--pool-growth-device-limit/plan.md`). That figure is consistent with this rate.

**GPU.** A Metal System Trace (`rendering_pipeline.md` §12) of each install-cost run, same binary and shapes. With `POSTRETRO_INSTALL_COST_TRACE_DIR` set, each test brackets every measured drain with labelled marker command buffers (`mark_drain`) and writes the run order there (`CostRun::write_sequence`). A drain's GPU time is the union of the process's GPU intervals between its two markers: its copy encoders, any compose passes, and the VRAM paging the trace shows in that window. Some marker rows are missing from the trace, as §12 warns. Those drains are dropped, not imputed: 121 of 348 lightmap drains and 59 of 276 SH drains. That leaves 193 and 198 warm drains.

| Series | Per drain | Per item | Per MiB | rms |
|---|---|---|---|---|
| Lightmap drain window (copies + paging) | — | 42 µs | 0.22 ms | 1.2 ms |
| SH upload copies | — | 70 µs | 0.43 ms | 0.6 ms |
| SH compose passes after the install | 2.4 ms | 8 µs | 0.47 ms | 1.3 ms |
| SH whole window (copies + compose + paging) | 2.8 ms | 90 µs | 0.95 ms | 1.8 ms |

A lightmap pair costs about three times as much per item on the GPU as on the CPU. Drains of a few large pairs run at 0.19–0.29 ms/MiB, and drains of 195–780 small pairs at 1.1–1.6 ms/MiB, so the linear fit is loose. Priced: 8 × 1 MiB costs 2.1–2.5 ms of lightmap GPU, and 16 × 1 MiB 4.2–4.4 ms. SH 8 × 1 MiB costs about 4 ms of copies and 6 ms of compose, 9–11 ms in its window; SH 16 × 1 MiB costs about 8, 10 and 19 ms. Per MiB, SH copies cost twice lightmap's before compose. The GPU agrees with the CPU on the byte unit: one MiB does not cost the same for both resources.

**These GPU figures are upper bounds, not pinned-clock costs.** Each test waits for the GPU after every drain, so the GPU idles between drains and drops into the low-clock regime §12 describes. The Radeon's core clock, polled every 2 s, had a median of 165–286 MHz over the lightmap run, peaking at 422 MHz. Over the SH run the median was about 64 MHz, peaking at 1,015 MHz. Temperature stayed at 67–87 °C. Before the runs, VRAM in use was 1.03 GB of 4 GB, with 2.68 GB free. Copies on the DMA channels depend less on core clock than the compute passes do, so the compose figures are the loosest. A saturated-clock figure needs a trace in-engine at a showcase burst, where the drain's encoders share a busy frame.

**Caveats.** The lightmap fixture's irradiance is Rgba16Float; shipped irradiance is BC6H, so equal bytes hold more texels here. The SH fixture is synthetic all-L0 chunks of Rgba16Float atlas blocks and probe patches; which format the showcase streams was not established. SH eviction, which shares a drain with installs in play, costs about 0.4 ms/MiB more and is outside these figures.

## Records (AC 22)

- **Showcase copy.** The frozen `.map` from `lightmap-compact-storage` (sha256 `6f937774718de8bb…`, the adjacent game project's 0.1 m source with worldspawn `_lightmap_density` 0.0125), rebaked at `32ac4217e` with `--no-cache` in 3,072 s: PRL sha256 `7de8f52003383790…`, pool 1,744 MiB. The old PRL no longer loads, so every prior number was re-measured on this bake. Completed baselines reproduce `lightmap-compact-storage`'s figures exactly. Campaign-test was rebaked at the same commit.
- **SH-first decode pickup.** At equal class the decode pool picks SH first. Lightmap pairs and SH chunks share its mandatory queue, so a lightmap pair's "read in flight" includes time queued behind SH decodes. That confounds the in-flight bucket on the far-point burst, and the SH gain above 8 MiB, which arrives without any change to SH admission.
- **SH's held-by-budget bucket** also holds chunks the renderer deferred under retirement pressure (`research.md` P11). Walks accept every SH chunk, so walks never produce that case; in play the bucket may overstate the budget's share.
- **Held-frame gap (P9).** The controller counts every drawn block on a held frame, so the gap the brief expected is 0 (`plan.md` Corrections). The walk reports the line anyway; it is 0 on every run.
- **Amendments to the lightmap cause list.** `waiting_on_permit` added, since the permit cap is a separate limit from the in-hand bytes. `awaiting_renderer_drain` added for a ready block not offered while the previous batch was outstanding (P4). `refused` widened to band reads held back for pool headroom. `read_in_flight` widened to targeted blocks still waiting for the issuer, as SH files them.
- **Added test-only lever.** An in-hand cap override (`POSTRETRO_LIGHTMAP_WALK_IN_HAND_CAP`), with the band half following it. It showed the in-hand wait is a symptom: removing it moves the same block-frames to the permit wait.

## Reproduction

The walk harness is `lightmap_residency_walks_from_prl` (ignored), driven by the `POSTRETRO_LIGHTMAP_WALK_*` environment variables (`walk_measurement/levers.rs`). Binaries were built outside `target/` per commit: baseline `32ac4217e`, spike `f7c35bcba`, sweep `e9b92a4f9`. Every timed walk ran alone on the machine under `nice -n 10` and `caffeinate`.

| Pin | Value |
|---|---|
| GPU trace | `xcrun xctrace record --template 'Metal System Trace' --env POSTRETRO_INSTALL_COST_TRACE_DIR=<dir> --launch -- <renderer test binary> <full test path> --exact --ignored --nocapture --test-threads=1`, then `xctrace export` of `metal-gpu-intervals`, reduced per drain between its markers |
| Machine | i9-9980HK (8C/16T), Radeon Pro 5300M 4 GB (Metal), 32 GB RAM, macOS |
| Walk build | test profile (dev, workspace opt-level 1, deps opt-level 2) |
| Fixtures | Showcase bake above; campaign-test 115 MB at `32ac4217e` |
| Modes | Completed (deterministic, two drains per frame) for honesty gates; realtime (one drain per frame against a 16.7 ms deadline) for the matrix and sweep |
