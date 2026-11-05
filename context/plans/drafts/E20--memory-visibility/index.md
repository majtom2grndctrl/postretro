# E20--memory-visibility

Brief · resumable · Epic 20 · reads: `context/lib/index.md` §1.1 · `context/lib/rendering_pipeline.md` §7.8 (Capture measurement, Lightmap-family byte meter), §10 (Target hardware), §12 · `context/lib/networking.md` §Not netcode: the live introspection channel · `context/lib/development_guide.md` §3.5, §4.1, §6.4 · `context/lib/testing_guide.md` §Resource bounds · read at 61a3a35e2, cited paths unchanged at 44881f797 · evidence: `research.md`

## Problem
The owner asked what is using memory on a large map, and answering it took a throwaway spike. On `stress-warren-hallway-inspection`, one Intel Mac, a 10-minute walk read a VRAM delta of p50 3,037 MiB and peak 3,264 MiB against the 2 GB target. A single-pose capture read about 2,270 MiB. It also read SH pool capacity at 728 MiB, against 1,440 MiB in the walk, and a host `upload_scratch` at 689 MiB, against 90 MiB. So a static pose misleads in both directions. Attribution was a hand tally of meters and source reading, with about 250 MiB of VRAM and a 152 MiB host block left unexplained. The existing meters also gave a wrong slack answer: SH occupancy counts source bytes and SH capacity counts physical bytes, and nothing in either says so. This is a developer-raised capability, long open as "resident and mandatory bytes per GPU resource" in `large-map-spatial-residency.md`. When this is done, an agent runs one command on a map, with no human reading logs, and gets one report. The report says what is using GPU and host memory across a sustained run, how much of each pool is slack in the pool's own units, what stayed unattributed, and where each number came from.

## Decisions
- **One renderer-owned ledger, several readers.** The capture report, observe-live, the dev panel and the walk report all read one ledger, and none keeps a tally of its own. It extends the row model in `residency.rs`, and the SH and lightmap reports feed it (§4.1: consumers get plain data, never `wgpu` types). This takes up what `done/sh-probe-streaming--measurement-harness` scoped out as "general renderer allocation tracking outside SH".
- **Coverage is by construction, not opt-in.** Every buffer and texture created through `GpuCreate` enters the ledger with no action by its owner. Anything no family claims shows in an `unclaimed` row, so a forgotten owner reads as a visible row, not a smaller total. `GpuCreate` today counts objects per thread on purpose and sees no bytes and no frees, so the ledger is a new shared store beside those counts, not a change to them.
- **Known gaps are named.** The chokepoint gate covers buffers and textures in `crates/renderer/src`. Query sets, the swapchain, `egui-wgpu` under `dev-tools`, wgpu-internal staging and driver padding sit outside it. Query sets join the gate. The rest gets a derived row where bytes are computable, and otherwise lands in `unattributed`.
- **The ledger total is requested bytes, and says so.** It is never presented as driver memory. Beside it the report carries an independent total where one exists, and `unattributed` is their difference. With no independent total, `unattributed` reports unavailable.
- **Slack compares like with like.** Each pool reports capacity, live, mandatory and high-water bytes, all physical, in the units capacity is counted in. Dense and sparse report separately. Source-payload figures may stay, labelled, but no ratio or slack is ever computed from them.
- **Pools expose the state that makes layout questions answerable.** Each pool also reports its free-range count and largest free run. This brief claims nothing about why any pool grew, and the SH growth question is handled elsewhere.
- **Every number carries its source and scope; absent is never zero.** Source names the origin. Scope is process or system-wide, and which backend or OS. It follows the `availability`/`reason` shape of `cpu_stages`, and levers and findings are labelled Metal-only or all-backends (§10).

  | Independent total | Metal | DX12 | Vulkan |
  |---|---|---|---|
  | `Device::generate_allocator_report` (allocated and reserved bytes, per-allocation names) | absent | yes | yes |
  | wgpu hal byte counters (`counters` feature) | never written | yes | yes |
  | OS adapter figure | `ioreg`: system-wide, macOS-only | none without `unsafe` | none without `unsafe` |

- **Host memory is declared rows plus an outside total.** Known owners declare rows: streaming buffers, upload scratch, level data. The process total comes from the harness, not the engine, because `capture`, `observe-live` and `observability` add no dependencies (§6.4) and `unsafe` needs approval (§3.5). Allocation-stack tools stay a manual macOS drill-down recipe. They need an unstripped rebuild and are not this tool.
- **A sustained run is the primary evidence.** A capture's memory block stays useful and is stamped as a static pose in sync-proof streaming, since capture refuses async mode (`require_sync_proof_mode`). A reader can then never mistake it for play. This keeps `measurement-harness`'s rule of no camera paths in capture.
- **A route is data.** A run reads a route file and a speed, and no route logic lives in the frame loop. Generators lift the test-only `walk_measurement` far-point tour and `world_poses` standing-eye poses instead of the spike's portal-centroid router. The report stamps the route's kind and generator.
- **The walk is a launch mode, never a live verb.** The live channel stays read-only (`networking.md`). The run ends through normal shutdown and publishes its report by staged rename, as capture does. A failed or stalled run publishes nothing and exits non-zero.
- **Outside samplers never gate the run.** They keep their own cadence and merge by timestamp. A slow sampler thins its own series and never delays the walk.
- **The series is bounded.** A run has a fixed sample cap, and the snapshot is built only at sample cadence, never per frame.
- **The harness lifts the generic half of paired-ab's launch protocol.** That brief left it to "a later brief" (`gpu-pass-paired-ab/research.md` §Launch harness). It covers build, content prerequisites, foreground and screen-saver checks and OS sampling, and every failure names its cause. It is a developer tool in `xtask` beside `capture`, so it stays out of `docs/`.
- **The reducer answers the question.** It reads a report and writes a summary: top families at p50 and peak, per-pool slack, unattributed bytes, and the comparison with the 2 GB target (`index.md` §1.1). Python, stdlib only, in `tools/`, as `gpu-pass-paired-ab` set out, with its unittest in preflight. It reports the target as a fact and never fails on it.
- **Gating follows §6.4.** The ledger records in every build, like `LightmapStreamingLiveDiagnostics`. Surfaces sit behind `capture`, `observe-live` and `dev-tools`. The walk gets a dependency-free feature not implied by `dev-tools`, and dist stage 1 stays clean.
- **The spike does not land.** Its env vars, frame-loop hook and CSV are discarded, and the `live_diagnostics` gate widening becomes unnecessary once the ledger exists.
- **Durable capture at promotion:** `rendering_pipeline.md` §12 gains a memory accounting subsection and §7.8's meter paragraph points at it. `networking.md` names the new live-only section. §6.4 and dist stage 1 gain the feature. `index.md` gains a router row.
- **Non-goals:**
  - The SH pool fix and any allocator or growth-policy change: this brief makes the question answerable and does not depend on the answer.
  - The player-facing memory tier and VRAM auto-detection: `large-map-spatial-residency.md` owns both.
  - CI memory gates: machine state moves these numbers.
  - Draw counts: the same `large-map` line asks for them, but they are not memory. CPU frame time has already shipped in the Performance tab.
  - Continuous telemetry: the roadmap's "Streaming telemetry + structured log sink" owns the stream, and this report is bounded.
  - Scripted events, combat and entity churn in the run: the report states it does not exercise them.

## Acceptance
### Automated
**Ledger coverage (GPU rows gated by `POSTRETRO_REQUIRE_GPU`):**
- [ ] A buffer and a texture created with no owning family appear in `unclaimed` at their full size.
- [ ] Texture bytes count mip levels, array layers and block compression, on fixtures of each.
- [ ] A dropped resource leaves the ledger once its GPU work completes. Live bytes return to their earlier value.
- [ ] Pool growth shows active and retiring generations apart, and retiring returns to zero after release.
- [ ] Creations on the upload thread and the main thread land in one ledger, and per-thread creation counts stay as they are today.
- [ ] A direct query-set creation outside the chokepoint fails the gate.
- [ ] A surface resize moves the swapchain row, or that row reports unavailable with a reason.

**Same-unit slack (red on today's meters):**
- [ ] A dense SH pool with known live slots reports live bytes as slots times the full physical slot, compose outputs included.
- [ ] Dense and sparse rows sum to the pool's active capacity.
- [ ] On every pool row, live never exceeds high-water and high-water never exceeds capacity.
- [ ] Mandatory bytes equal the physical bytes of the current mandatory set, on an SH and a lightmap fixture.
- [ ] A lightmap pool reports slot-aligned live bytes, and its spare layer on its own line.
- [ ] An alloc and release sequence that leaves holes reports a largest free run below total free bytes. A compact sequence reports them equal.
- [ ] No report field or reducer output is a ratio with a source-payload figure on either side.

**Honest absence:**
- [ ] Every total carries a source and a scope.
- [ ] With no independent total, `unattributed` reports unavailable with a reason, never zero.
- [ ] With an independent total, ledger plus `unattributed` equals it. A negative difference is reported as negative, not clamped.

**Readers agree:**
- [ ] One snapshot read through the capture block, the live section and the walk report gives equal rows.
- [ ] A capture's memory block names static pose and sync-proof streaming.
- [ ] The live section reports `not-requested` until asked, reads without consuming, and a batch runspec requesting it is rejected by name.

**Run lifecycle:**
- [ ] A completed run exits zero through normal shutdown and publishes one report.
- [ ] A run killed mid-walk, or with no frame progress past its stall limit, publishes nothing, leaves a prior report untouched and exits non-zero.
- [ ] A run past the sample cap keeps a bounded series and states that it thinned.
- [ ] With no reader asking, no snapshot is built and no per-frame allocation is added.
- [ ] A sampler that sleeps longer than its cadence delays no walk step and shifts no sample in the engine's series.
- [ ] The harness names a missing content prerequisite or a failed machine-state check, and stops before launch.

**Route:**
- [ ] A route file round-trips byte-equal, and one seed generates one route.
- [ ] Every generated pose stands at eye height over a floor with player clearance.
- [ ] The report stamps route kind, generator and speed. An unwalkable route says so.

**Reducer (synthetic fixtures, preflight-run unittest):**
- [ ] A fixture series yields the expected p50 and peak per family, per-pool slack and unattributed bytes.
- [ ] A report from another schema version is an error.
- [ ] A total over 2 GB is reported and is not a failure.

**Gating:**
- [ ] A build with no diagnostic feature still records the ledger.
- [ ] The walk feature is in no default feature set, and dist stage 1 passes no feature.
- [ ] No memory-tool env var has effect in a build without its feature, beyond one warning that names it.
- [ ] The spike's frame-loop hook and env vars are absent from the tree.

### Manual
- [ ] **Unattended answer, Mac.** Run the harness on `stress-warren-hallway-inspection`, then give a fresh agent only the summary. It names the top families and each pool's slack, and they match the raw series.
- [ ] **Reconciliation, Mac.** Record ledger total, independent total and `unattributed` over the same run. Unattributed above 5% of the total becomes a named finding in the plan of record. This is measure-and-report, not a gate.
- [ ] **Static versus sustained.** On the same map, the report shows the capture block and walk peak disagreeing, and a reader can tell why from their stamps.
- [ ] **Windows.** Repeat on the GTX 16-series floor on each backend that runs: allocator totals present, reserved minus allocated reported, per-platform section recorded.
- [ ] **Stall, Mac.** Occlude or lock the session mid-run. The run fails by name and publishes no report.
- [ ] **Dev panel, visual.** The Performance tab's memory block shows ledger families and pool slack, and tracks a walk.

## Path
Non-binding.
- **Ledger store.** Shape: `GpuCreate` records label, family and bytes on creation, and a tracked handle reports the free. `wgpu::Device` has nowhere to hold state, so the store is process-wide or the trait moves to a wrapper. Rival: owner-declared rows only, which skip the type churn across the renderer but lose coverage by construction.
- **Liveness is the riskiest assumption.** No free hook exists, the renderer calls no `.destroy()`, and a texture view or bind group can outlive its handle. A handle-drop ledger may read low, which reconciliation will show.
- **Seams.** `texture_row`, `allocation_total_bytes`; `FirstFitRanges` (its `capacity` is the address high-water, and `free` gives free runs) in `dense_slots` and `SparsePool`; `LightmapPoolModel`, `BlockFacts.texels`, band headroom; `ShStreamingAllocationSummary`; `MeasurementReport`, `stage_measurement_report`; the `cpu_timing` live-only section; `draw_performance_tab`; `start_pose::place_local_pawn`.
- **Route reuse.** `WalkKind::FarPointTour`, `world_poses/poses.rs` and its `postretro-world-poses 1` file are `#[cfg(test)]` today and must move out of test-only code. Check portal-derived paths against standing-eye poses: the tour yields cells, not eyes.
- **Harness precedents.** `xtask capture`, `build_scripts_sidecar`, and §12 Machine-state confounders.
- **Consumers to shape for.** `E25--map-transitions` wants peak resident bytes with two levels, so rows carry a lifetime: device, level set or retiring.
- **First slice.** The ledger on one level install, read through capture, reconciled on Mac against `ioreg` and on Windows against the allocator report. It falsifies coverage and liveness before any route work.
- **Files past ~800 lines this extends:** `frame_loop/mod.rs`, `capture/report.rs`, `capture/driver.rs`, `debug_ui/mod.rs`. Split first. The walk driver is its own module, with one call from the frame loop.

## Open questions
- Which host runs the walk first? The windowed session is what the spike proved, but an occluded window stalls it, so it cannot run beside an owner using the machine. An offscreen session is `E20--scripted-run-capture`'s driver, which is undrafted. Build on the windowed host and keep the driver host-agnostic, or draft scripted-run first? — owner: Dan — **blocks build**
- Pose-driven or input-driven walk? Pose-driven is robust and needs no stuck detection, but it writes the pawn each frame, which reads against Epic 20's "external state writes that fight sim authority" non-goal. Input-driven through the runspec command vocabulary honours collision and speed, and adds a path follower. — owner: Dan — **blocks build**
- Metal independent total: stay on `ioreg` (system-wide, baseline drifts), or approve one `unsafe` read of `currentAllocatedSize` through `as_hal` for a per-process figure? — owner: Dan — **blocks build**
- Host total: harness-side OS sampling only, or approve a dependency or an `unsafe` counting allocator for a portable in-engine figure? — owner: Dan — **blocks build**
- One brief or two? The ledger with its readers stands alone, and the run with its harness consumes it. The Problem needs both, so this drafts them together. Splitting would ship the ledger sooner. — owner: Dan — **blocks build**
- Do mod authors get this? The dev panel reaches them through the SDK engine's `dev-tools`. The walk feature does not. — owner: Dan — **blocks build**
- Ledger liveness mechanism, after the first slice — **delegated**: the executor decides and reports it at the plan stop
- Family names, schema and field names, env var and flag names, sample cadence and cap, stall limit — **delegated**: the executor decides and reports in the plan of record
- Which backend `Backends::PRIMARY` selects on the Windows machine — **delegated**: the executor records it in the Windows section
