# streaming-drain-budget-spike

Brief · compact · spike · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency, Lightmap cell-block residency), §12 (GPU Pass Timing, CPU Stage Timing, Machine-state confounders) · `context/lib/experimental_spikes.md` · `context/lib/testing_guide.md` §Resource bounds · read at e0d708672 · evidence: `research.md`

## Problem
The `lightmap-compact-storage` build raised this as a review question; it is an anticipated need, not an observed pop or hitch. SH chunks and lightmap block pairs share one per-drain install budget: one byte cap per frame, one admission order. The reasons for sharing still hold. Three assumptions underneath it were never measured. First, the 8 MiB cap was a first guess that the SH streaming work said should be tuned on real content, and it was not. The showcase map from the adjacent game project joins two large rooms, so its demand bursts are far larger than any map the cap was sized against. Second, the cap counts decoded bytes as if a byte costs the same frame time for both resources, but an SH install also recomputes the affected compose rows. Third, at equal class SH is admitted before lightmaps; that order was inherited unchanged from SH, although the owner's no-pop-in ruling already treats a lightmap miss as the less tolerable one. The one piece of evidence so far points away from SH crowding lightmaps out (`research.md`). Nobody can say how many late blocks the budget causes, because lightmap late blocks do not record why they were late. When this is done, lightmap misses report their cause as SH misses do, and a findings note gives, for each assumption, the measured numbers and a recommendation: keep it, change it, or open a brief.

## Decisions
- **A spike, not a change of default.** Build-to-learn under `experimental_spikes.md`. The deliverable is the findings note. A recommended change to the cap, its unit or the tie-break becomes its own brief, so admission behaves identically with the spike's levers unset.
- **The shared structure is not reopened.** One budget owner, one admission order across resources, and the issuer and decode pool stay as `rendering_pipeline.md` §4 and `plans/done/spatial-residency--lightmap-cell-blocks` set them. Separate caps per resource would repeat the retired-cap bug (`plans/done/sh-streaming--warm-set-and-io-contract`).
- **Lightmap miss causes are permanent diagnostics.** The lightmap walk's `left_behind` count lumps blocks that were not demanded, not yet read, over the install budget, or refused by the pool. It splits into cause buckets that line up with SH's miss causes, so both resources report "held by the drain budget" on one definition. A counter of drains in which the budget separated an SH item from a lightmap item of the same class lands beside it. Both bring the lightmap side up to §4's always-on miss-cause reporting and stay after the spike; admission never reads them.
- **The first slice gates the rest.** The cause buckets and the same-class counter run first, on realtime showcase and campaign-test walks at the production cap. The cap sweep and the install-cost model run only if the budget holds back more than a threshold the executor states and reports, or if same-class cuts occur. The severity read runs only if same-class cuts occur. A gate that stays closed is a finding, recorded in the note with its numbers.
- **Two cap levers, the in-hand cap pinned.** Walk sweeps use a measurement override like the walk's pool-layer override; an in-engine dev-tools slider lets the owner watch budget pops live. This diverges from `experimental_spikes.md` §Tuning levers, which names only sliders, because the walk harness has no debug UI. Neither is a player setting, and unset means the production budget. The lightmap in-hand cap is defined as four times the drain cap; both levers pin it at its production value so only admission moves.
- **Install cost is measured on a warm staging pool, in the renderer.** The base is the renderer drain test `lightmap_drain_install_timing`, plus an equivalent test for SH install and compose. A cold staging pool costs about ten times a warm one per MiB (`plans/done/spatial-residency--lightmap-cell-blocks/findings.md`), so first drains are discarded and reported apart. The model is per resource: fixed cost per item plus cost per MiB, CPU and GPU. GPU time comes from a Metal System Trace reduced per encoder (§12), because the Mac perf target has no timestamp queries.
- **Severity starts from the owner's ruling.** The cell-blocks decision "Miss policy is stricter than SH's" holds that the no-pop-in rule rules out SH's ambient-floor tolerance for lightmaps. The visual read of forced misses tests that ruling at the showcase's poses; it does not start neutral.
- **Fixtures and machine.** The adjacent game project's showcase map at 0.0125 m (the frozen copy used by `lightmap-compact-storage`), campaign-test, and the generated stress warren. Runs follow §12's machine-state rules and `testing_guide.md` §Resource bounds; frame and GPU time are reported, not gated.
- **Not this brief:** a second decode thread at one core is the open owner question from `lightmap-compact-storage`; an adaptive or time-based budget is a candidate recommendation, not something to build here.

## Acceptance

### Automated (honesty gates)
- [ ] With both levers unset, completed-mode walks give the same late blocks, installs and payload hash as before the spike, on both maps.
- [ ] For every walk run, the lightmap cause buckets sum to the run's late blocks, and the spawn-step buckets sum to its spawn misses.
- [ ] A drain in which the budget stops between an SH item and a lightmap item of the same class increments the counter once; a drain that stops between different classes, or admits everything, does not.
- [ ] With the cap override set to the production value, completed-mode results equal the unset run exactly.
- [ ] At every cap either lever sets, the lightmap in-hand cap stays at its production value.
- [ ] A cap of zero, or one too large to represent, is refused by name from either lever, never clamped silently.
- [ ] A drain whose first item exceeds the set cap still admits that item.
- [ ] Install-cost tests report warm drains only, with each run's discarded cold drains counted separately.

### Manual (measured findings and owner reads)
- [ ] Late lightmap blocks and late SH chunks by cause, plus same-class cuts per walk, realtime medians of 3: showcase far-point and random walks and campaign-test, at default cores and emulated 1 core, SH on.
- [ ] The gate decision: the stated threshold, the measured share held by the budget, and which later rows therefore run.
- [ ] If the gate opens: a cap sweep with at least one value below the production cap and two above, reporting late items held by the budget, other causes, largest drain in bytes, and drain CPU p95/max for each cap.
- [ ] If the gate opens: install cost per resource, CPU and GPU, as fixed cost per item and cost per MiB, with build profile, pinned clock and VRAM state recorded.
- [ ] If same-class cuts occur: owner visual read of one forced lightmap-block miss and one forced SH-cluster miss at each fixed pose, against the no-pop-in ruling.
- [ ] Owner read with the dev-tools slider at a showcase burst: does a smaller or larger cap change what is visible.
- [ ] A findings note recommending, for each of the three assumptions (cap magnitude, byte unit, tie-break), keep, change, or new brief, with the numbers behind each.

## Path
- Seams: `admit_drain`, `MAX_INSTALL_DECODED_BYTES_PER_DRAIN`, `SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN` (`streaming/drain_budget.rs`); the drain's `admit_within` call in `session/level_streaming`; `MAX_IN_HAND_PAIR_BYTES` and its band half (`lightmap_streaming/controller.rs`); lightmap late-block counting in `walk_measurement/report.rs` and `results.rs`; SH `MissCause` (`sh_streaming/targeting.rs`) and its walk line (`sh_walk.rs`); `lightmap_drain_install_timing` (`renderer/src/lighting/lightmap/stream/install_timing_measurement.rs`); SH `install_cpu_max_steady_drain_micros`.
- First slice: the cause buckets and the same-class counter on one realtime showcase far-point run. It falsifies the riskiest assumption, that the budget holds back enough to matter.
- Install cost: the shape chosen is the warm renderer drain test with a matching SH test. The rival, a capture at fixed poses, mostly measures the cold staging pool.
- Forced misses for the visual read: a dev-tools or capture-only hook that withholds one item from admission at a pose, never a player setting (`experimental_spikes.md` §Tuning levers).

## Open questions
- The gate threshold and the sweep's cap values — **delegated**: the executor states them after the first slice and reports them.
- Whether to add a Windows GPU-timestamp run beside the Mac trace — **delegated**: the Mac trace is the default; the owner may add the PC run.
- How a miss is forced for the visual read — **delegated**: the executor chooses the hook within the Path's limit.
