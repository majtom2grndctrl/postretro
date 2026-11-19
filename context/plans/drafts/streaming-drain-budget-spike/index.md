# streaming-drain-budget-spike

Brief · compact · spike · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency, Lightmap cell-block residency), §12 (GPU Pass Timing, CPU Stage Timing, Machine-state confounders) · `context/lib/experimental_spikes.md` · `context/lib/testing_guide.md` §Resource bounds · read at e0d708672 · evidence: `research.md`

## Problem
Reviewing the `lightmap-compact-storage` build, the owner asked whether the 8 MiB drain cap is right or only inherited. No pop or hitch has been traced to the cap. The showcase's realtime walks do show late lightmap blocks that the pool does not explain (`research.md`). SH chunks and lightmap block pairs share one per-drain install budget: one byte cap per frame, one admission order. Three assumptions underneath it were never measured:
- **The cap's size.** The SH streaming work marked 8 MiB as a first guess to tune on real content. The shared budget then carried it over unchanged so SH would behave as before. The adjacent game project's showcase map joins two large rooms, and its demand bursts are far larger than any map the cap was sized against.
- **The byte unit.** The cap counts decoded bytes as if a byte costs the same frame time for both resources, though an SH install also recomputes the affected compose rows.
- **The tie-break.** At equal class SH is admitted first, an order also carried over from SH.

A cap can be wrong in either direction. Too small, and items arrive late; too large, and one drain costs a hitch. Nobody can say how many late blocks the budget causes, because a lightmap miss does not record its cause. The one piece of evidence so far points away from SH crowding lightmaps out (`research.md`). When this is done, lightmap misses report their cause in play as SH misses do. A findings note answers the owner's question with numbers, and recommends for each assumption: keep it, change it, or open a brief.

## Decisions
- **A spike, not a change of default.** Build-to-learn under `experimental_spikes.md`. The deliverable is the findings note; the spike changes no default, so admission behaves identically with its levers unset. A retuned cap may land as a direct build, since the SH streaming work marked its first guesses as tuning rather than contract. A change to the unit, the tie-break or the shared structure becomes its own brief.
- **The shared structure is not reopened.** One budget owner, one admission order across resources, and one read issuer stay as `plans/done/spatial-residency--lightmap-cell-blocks` set them, on its warrant that bounds are policy, owned once. The shared decode pool stays as `lightmap-compact-storage` set it. The tie-break question covers drain admission only. Decode pickup keeps SH first at equal class, and the note says where that order could confound a result. The note may still recommend reopening the structure, for instance if the visual read finds that a visible block one drain late breaks no-pop-in.
- **Lightmap miss causes are production diagnostics.** The lightmap controller's drawn-not-resident block-frames split into cause buckets in production code, surfaced where §4 surfaces SH's: the streaming log line, the dev-tools Streaming tab and the capture report. Each block-frame lands in exactly one bucket. A bucket takes SH's name wherever the cause is the same. Lightmap-only causes, such as waiting on the in-hand cap or deferral by the pool (a retiring generation or the device limit), get their own. The executor may amend the cause list where the code splits differently, and reports the change. Each surface names its unit: block-frames for lightmaps, episodes for SH. "Held by the drain budget" means ready and past this drain's cut. SH's existing bucket of that name also holds chunks the renderer deferred, which walks never produce. It stays as it is, and the note says so. The buckets stay after the spike, and admission never reads them.
- **Spike tallies stay in test code.** Two walk tallies serve the gate and go no further. The first counts drains whose budget cut admits an item of one resource and leaves behind a same-class item of the other. The second classifies SH late cluster-frames on every frame, as lightmap blocks are classified. SH's episode buckets take a miss's cause from its first frame, which files a burst's budget holds as reads in flight. The per-frame tally makes the two resources' shares comparable.
- **The first slice gates the rest, on either side of the cap.** The cause buckets and the walk tallies run first, on realtime walks at the production cap, reporting the largest drains in bytes. The cap sweep and the install-cost model run when any of these holds:
  - the budget holds back more than a stated share of late items (the cap may be too small);
  - the largest drains, at the lightmap warm install rate already measured, cost more than a stated share of a frame (the cap may be too large);
  - same-class cuts exceed a stated count.

  The severity read runs only if same-class cuts exceed that count. A gate that stays closed is a finding, recorded in the note with its numbers.
- **Two cap levers, the in-hand cap pinned.** An in-engine dev-tools slider lets the owner watch budget pops live. Walk sweeps need a measurement override as well, because the walk harness has no debug UI. The override diverges from `experimental_spikes.md` §Tuning levers, which names only sliders. Neither lever is a player setting. Both replace the play cap only: unset means the production budget, and settling keeps its own. The lightmap in-hand caps keep their production byte totals at every lever value, so only admission moves. The executor may add further test-only levers the first slice points to, such as an in-hand cap coupled to the drain cap or a flipped tie-break, and reports what each showed.
- **Install cost is measured on a warm staging pool, in the renderer.** Lightmap install cost comes from the renderer drain test, and SH's from a matching test of install plus the compose its installs trigger. A drain the staging pool creates a buffer for runs at the cold rate, about ten times the warm rate per MiB. Those drains are reported apart, wherever they fall in a run. The model is per resource: fixed cost per item plus cost per MiB, CPU and GPU.
- **Severity starts from the owner's ruling.** The cell-blocks decision "Miss policy is stricter than SH's" rules out SH's ambient-floor tolerance for a lightmap block refused at capacity. The visual read of forced misses, at the showcase's poses, tests whether that ruling extends to a block one drain late.
- **Fixtures and machine.**
  - **Maps:** the adjacent game project's showcase at 0.0125 m, and campaign-test.
  - **Showcase copy:** the owner names the adjacent project when the spike runs on the Mac. Use the frozen copy `lightmap-compact-storage` used, if it can be recovered. Otherwise a fresh copy of the project's source with worldspawn `_lightmap_density` set to 0.0125; the note records its hash, and prior numbers are re-measured on it rather than compared across copies.
  - **Cores:** walks run at default cores and at emulated one core, where decode rather than the budget may be the limit.
  - Frame and GPU time are reported, not gated.
- **Not this brief:** a second decode thread at one core is the open owner question from `lightmap-compact-storage`. An adaptive or time-based budget may be a recommendation.

## Acceptance

### Automated (honesty gates)
- [ ] With every spike lever unset (both cap levers and any test-only lever the executor adds), completed-mode walks with SH on and with SH off give the same late blocks, installs, payload hash, late SH chunks and SH installs as values recorded at the read commit from the same command, on both maps.
- [ ] A controller fixture draws one block in each lightmap cause the controller distinguishes (as briefed: outside the reach, read in flight, waiting on the in-hand cap, held by the drain budget, deferred by the pool, refused, failed; or as the executor amends and reports them), and each lands in its own bucket and in no other (pins P6–P8, P15).
- [ ] In a build without test code, the lightmap cause buckets appear wherever SH's miss buckets do: the throttled streaming log line, the dev-tools Streaming tab, and the capture report's streaming JSON. Each surface names block-frames for the lightmap buckets and episodes for SH's.
- [ ] For every walk run, the lightmap cause buckets sum to the controller's drawn-not-resident block-frames, and the spawn-step buckets to the spawn step's. Drawn blocks a held frame leaves uncounted are reported on their own line (pins P9, P13).
- [ ] A drain whose budget cut admits an item of one resource and leaves behind a same-class item of the other increments the counter once, wherever inside that class the cut falls (pins P1, P2). A drain that stops between different classes or admits everything does not. Nor does a drain with no items, a drain with items of one resource only, or one in which the lightmap side offered nothing because its batch was still outstanding (pins P3, P4).
- [ ] Each walk run with SH on reports late SH cluster-frames by cause, classified on every frame as lightmap blocks are, after the spawn step with the spawn step's apart, beside SH's episode buckets (pins P10, P13, P14).
- [ ] With the cap override set to the production value, completed-mode results equal the unset run exactly.
- [ ] From either lever, a play drain admits against the lever-set cap. At half the production cap, a drain the production cap admits whole stops at the lever-set cap. At twice the production cap, a drain the production cap would cut admits whole.
- [ ] At half and at twice the production cap, from either lever, with every added lever unset, a read waits at the same in-hand and band in-hand byte totals as with the levers unset.
- [ ] At a lever-set cap, a drain whose first item exceeds the cap admits that item and nothing after it.
- [ ] With either lever set, a settling drain still admits against the settling budget (pin P5).
- [ ] If the gate opens: install-cost tests count a drain as cold whenever the staging pool creates a buffer for it, wherever it falls in the run. They report warm drains only and count cold drains apart (pin P12).
- [ ] If the gate opens: install-cost tests vary item size at a fixed drain byte total, and item count at a fixed item size, so fixed cost per item and cost per MiB separate for each resource.
- [ ] Grep gate: the cap slider builds only with dev-tools; the walk override, both walk tallies and any test-only lever the executor adds live only in test code. No player setting names the drain cap.

### Manual (measured findings and owner reads)
- [ ] Per walk, realtime medians of 3: late lightmap block-frames and late SH cluster-frames by cause, SH's episode buckets, same-class cuts, and the largest drains in bytes, each drain's admitted bytes of both resources together with each resource's share; the spawn step's figures apart (pin P13). Walks: showcase far-point and random, and campaign-test, at default cores and emulated 1 core, SH on.
- [ ] The gate decision. For each side of the cap, and for same-class cuts: the stated threshold, the measured share held by the budget for each resource in its own named unit (the lightmap's from its block-frame buckets, SH's from the per-frame tally in cluster-frames, never from SH's episode buckets), and the largest drains' cost at the warm rate. Then which later rows therefore run.
- [ ] If the gate opens: a cap sweep with at least one value below the production cap and two above. For each cap: late items held by the budget, reads waiting on the in-hand cap, other causes, largest drain in bytes, and drain CPU p95/max.
- [ ] If the gate opens: install cost per resource, CPU and GPU, as fixed cost per item and cost per MiB, with build profile, pinned clock and VRAM state recorded.
- [ ] If the gate opens: SH's install cost includes the compose work its installs trigger, CPU and GPU, reported beside the install alone.
- [ ] If same-class cuts exceed the stated count: owner visual read of one forced lightmap-block miss and one forced SH-cluster miss at each fixed pose, against the no-pop-in ruling, including whether it extends to a block one drain late.
- [ ] Owner read with the dev-tools slider at a showcase burst: does a smaller or larger cap change what is visible?
- [ ] A findings note that answers whether the 8 MiB cap is right and where it came from. For each of the three assumptions (cap size, byte unit, tie-break), it recommends keep, change, or new brief, with the numbers behind each. Where the buckets show another lever matters more (the in-hand cap, decode at one core, pool refusal), the note names it.
- [ ] The note records the showcase copy's hash and whether prior numbers were re-measured on it; says where SH-first decode pickup could confound a result; says SH's held-by-budget bucket also holds chunks the renderer deferred, which walks never produce (pin P11); and lists any amendment to the lightmap cause list and any test-only lever added, with what each showed.

## Path
- **Seams:**
  - The budget: `admit_drain`, `MAX_INSTALL_DECODED_BYTES_PER_DRAIN` and `SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN` (`streaming/drain_budget.rs`), and the drain's `admit_within` call in `session/level_streaming`.
  - The in-hand caps: `MAX_IN_HAND_PAIR_BYTES`, today four times the drain cap, and its band half (`lightmap_streaming/controller.rs`). Pinning them decouples that ratio.
  - Lightmap misses: `drawn_not_resident` and its visible-miss recording (`lightmap_streaming/controller.rs`). Surfaces: `session/lightmap_streaming_diagnostics.rs`, the Streaming tab (`renderer/src/render/debug_ui/streaming_tab.rs`), and `capture/report.rs`.
  - SH misses: `MissCause` (`sh_streaming/targeting.rs`) and its walk line (`sh_walk.rs`).
  - The walk: its lightmap late-block tally in `walk_measurement/frame_driver.rs`, `report.rs` and `results.rs`.
  - Install timing: decode pickup order (`streaming/decode_pool.rs`); staging-pool buffer creation (`renderer/src/render/uploads/staging_pool.rs`); `lightmap_drain_install_timing` (`renderer/src/lighting/lightmap/stream/install_timing_measurement.rs`); SH `install_cpu_max_steady_drain_micros`.
- **First slice:** the production buckets and the walk tallies on one realtime showcase far-point run. It falsifies the riskiest assumption, that the budget holds back enough to matter. At the read commit, campaign-test has no lightmap late blocks, so only its SH side can open the gate.
- **Cap override:** follow the walk's pool-layer override (`walk_measurement/levers.rs`), an env var that refuses a zero by name.
- **Install cost:** the chosen shape is the warm renderer drain test plus a matching SH test. The base test installs one item size, so varied sizes are new. The rival, a capture at fixed poses, mostly measures the cold staging pool. GPU time comes from a Metal System Trace reduced per encoder (§12), because the Mac perf target has no timestamp queries.
- **Forced misses for the visual read:** a dev-tools or capture-only hook that withholds one item from admission at a pose, never a player setting (`experimental_spikes.md` §Tuning levers).

## Open questions
- The gate thresholds on both sides of the cap, the same-class cut count, and the sweep's cap values: **delegated**. The executor states them after the first slice and reports them.
- Whether to add a Windows GPU-timestamp run beside the Mac trace: **delegated**. The Mac trace is the default; the owner may add the PC run.
- How a miss is forced for the visual read: **delegated**. The executor chooses the hook within the Path's limit.
