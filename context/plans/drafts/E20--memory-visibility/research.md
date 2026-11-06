# E20--memory-visibility — research

Read at `61a3a35e2`; every cited source and context path shows no diff against `origin/main` at `44881f797`. The spike was uncommitted when this was read and its output sat in a temporary directory, so everything the build needs from it is carried here. Nothing below cites a spike path as durable.

Provenance tags: **[source]** read in code this session · **[re-derived]** recomputed from the spike's raw output · **[reported]** stated in the drafting request, not re-derived · **[inferred]** reasoned from source, not observed.

## Spike

**What ran.** An env-gated patch in the windowed frame loop built a route, then each frame moved the local pawn and camera along it, and every 2 s appended a CSV row of SH and lightmap meters. A shell loop sampled `ioreg`, `ps` and `footprint` every 2 s beside it. Separate runs took one offscreen capture with its measurement report, and heap stacks through `MallocStackLogging=lite`, `heap -s`, `malloc_history -allBySize` and `atos`.

**Route logic.** Portal adjacency over cells neither solid nor exterior, with portal polygon centroids as crossing points. BFS from the camera cell gave the reachable set. Then up to 40 far-point targets, each the reachable cell farthest from every chosen target, with BFS legs between them. The pose held for 15 s, then moved at the configured speed. This re-invents `WalkKind::FarPointTour`.

**Sampling recipe.** `ioreg -c IOAccelerator -r -d 1`, grep `inUseVidMemoryBytes` and `vramFreeBytes`, take the largest non-zero `inUseVidMemoryBytes`, subtract a pre-launch reading. `footprint -p <pid>` line 2 gives the process footprint. Launch needed `caffeinate -dimsu` and a `pgrep -x ScreenSaverEngine` check before and during. Worktree launch needed `--mod dev`, `--baked-root <main checkout>`, `--windowed`, and a precompiled `content/dev/start-script.js`.

**CSV columns.** `t,x,y,z,cell`, SH `cap, occ, target, sampleable, evict, miss`, lightmap `pool, layers, peak, res_lm, res_sm, mand_lm, mand_sm, evict, growths, not_resident`.

## Final results

**`ready/spatial-residency--sh-bounded-pool/research.md` supersedes the figures below where they differ.** Its Metal trace is the run of record; raw spike data was discarded. The walk below ran without textures, through a wrong `--baked-root`, so its VRAM reads low: the run of record reads p50 3,037 MiB, peak 3,264 MiB, with about 250 MiB unattributed. The spike also measured the SH growth: live slots equalled the high-water mark at every dense growth, with no free ranges, so fragmentation is ruled out on that walk. The spike and its data are now on local branch `spike/memory-measure-tour`.

## Measured evidence

Map `stress-warren-hallway-inspection`, AMD Radeon Pro 5300M, Metal, windowed, 10 minutes at 12 m/s. One map, one machine, one run of each.

| Claim | Value | Provenance |
|---|---|---|
| VRAM delta over pre-launch baseline, walk | p50 2,782 MiB, peak 3,044 MiB | [re-derived] from 264 samples |
| VRAM delta, single-pose capture | p50 2,215, peak 2,335 MiB by my reduction; request says about 2,270 | [re-derived], differs by method |
| Pre-launch `ioreg` baseline | 798 MiB (walk) vs 1,171 MiB (capture) | [re-derived]: the system-wide baseline moves between runs |
| SH active capacity, walk | 728 MiB at t=0, 794 MiB at t=2 s, 1,440 MiB by t=18 s (3 s after moving began), then flat to the end | [re-derived] from CSV; the cause is not established here |
| SH active capacity, capture | 728 MiB (`763361072`) | [re-derived] from the capture report |
| Lightmap pool, walk | 490 MiB at 34 layers plus spare, 14 MiB per layer; passes through 15 and 28 layers | [re-derived] |
| Lightmap resident, walk | p50 180 MiB, max 346 MiB | [re-derived]; see meter audit before reading as slack |
| Host `upload_scratch` | 689 MiB in capture, 90 MiB live in play | [reported] |
| Process RSS peak | 1,364 MiB walk, 2,239 MiB capture | [re-derived] |
| `footprint` | p50 612 MB, max 628 MB in the walk | [re-derived] |
| `footprint` graphics category | about 155 MB against about 2.8 GB VRAM | [re-derived]: it does not track VRAM here |
| Largest unattributed host stack | 160,169,984 B, about 152 MiB, under the event loop | [re-derived] size; [reported] attribution |
| Unattributed VRAM in the hand tally | about 50 MiB | [reported] |

**Withdrawn.** The spike's SH "held" figures are not a slack measurement and must not be used as one. See the meter audit.

**Fidelity limits.** Route includes non-walkable heights. Pawn teleports each frame, ignoring collision. 12 m/s is above `walk_measurement`'s 8 m/s `RUN_SPEED`. Sampling with `footprint` slowed the shell loop enough to miss the heap trigger on one run [reported]. Whether teleporting fires trigger volumes was not checked.

## Keep and discard

| Spike element | Verdict | Why |
|---|---|---|
| Moving through the map to size pools | Keep | The one result that changed the answer: static pose reads low on GPU and high on host |
| Time series, not a single reading | Keep | Pools grow in steps; peak and p50 differ by hundreds of MiB |
| Pool meters beside a total | Keep, fix units | The reconciliation idea was right, the meters' units were not |
| Hand tally of per-resource bytes | Replace | Ledger |
| `ioreg` as VRAM total | Keep as labelled fallback | The only Metal total without `unsafe`; system-wide and macOS-only |
| `footprint` / `ps` totals | Keep, outside the engine | Process total only, on its own cadence |
| `heap`, `malloc_history`, `atos` | Manual recipe | macOS-only, needs an unstripped rebuild, slows the run |
| Portal-centroid router | Discard | In-tree far-point tour and standing-eye poses exist |
| Per-frame teleport | Owner question | See Rivals |
| `std::process::exit(0)`, thread-local state, env-var config, CSV | Discard | Incidental; no shutdown, no publication safety, no schema |
| `live_diagnostics` gate widening | Discard | Ledger removes the need |
| The map, the speed, `tex.py`, `texkeys.py`, `prl_sections.py` | Discard | One investigation's inputs and helpers |

## GpuCreate audit

| Question | Finding | Provenance |
|---|---|---|
| Does every renderer allocation route through it? | For buffers and textures in `crates/renderer/src`, yes by rule. `gpu_create_gate.rs` is a `syn` AST gate over method calls, UFCS paths and macro tokens, skipping test-only code and scanning feature-enabled code. The gate's kinds are `create_buffer`, `create_buffer_init`, `create_texture`, `create_texture_with_data`, `create_bind_group` and both pipeline kinds. | [source]; gate not run, cargo was off-limits |
| Raw sites left? | Grep hits outside `gpu_create.rs` sit under `#[cfg(test)]` or in test files, spot-checked in `sh_volume.rs` and `indirect/runtime.rs`. `create_query_set` is the one production exception. | [source] |
| What bypasses it? | `create_query_set` in `frame_timing.rs` (not a gate kind); `surface.configure` swapchain textures; `egui-wgpu`, optional under `dev-tools`, its own crate, outside the scan; wgpu-internal staging for queue writes; driver padding. | [source] for the first three; [inferred] for the last two |
| Other crates creating GPU objects? | No. Outside the renderer, `wgpu::` appears only in `render-cpu/src/device_limits.rs`. | [source] |
| Does it see bytes? | No. Counts objects only, per thread by design: a question about one thread's work must not see the upload thread's. `new_buffer_init` meters initial data for chunking, a different concern. | [source] |
| Does it see frees? | No. It returns plain `wgpu` handles, and the renderer's non-test code never calls `.destroy()`. | [source] |
| Can a label attribute a row? | My grep found no `label: None` in non-test renderer code, single-line form only. Labels look usable as a start. | [source], partial |

**Verdict.** The seam is honest about creation and silent on size, ownership and lifetime. A ledger built on it can be complete for what passes through, but needs a new free signal. A texture view or bind group can keep a texture alive after its handle drops, and wgpu holds resources until submitted work completes, so handle-drop liveness can read low [inferred]. An independent total is what exposes that.

## Backend capability

| Capability | Finding | Provenance |
|---|---|---|
| `Device::generate_allocator_report` | Default hal impl is `None`. DX12 and Vulkan override it. Metal does not. Reports per-allocation name, offset and size, block sizes, `total_allocated_bytes`, `total_reserved_bytes`. Not behind a wgpu feature at the call site. | [source], wgpu 30.0.1 |
| Hal `buffer_memory`, `texture_memory` | Written in DX12 and Vulkan. Never in Metal, which counts objects only. Needs wgpu's `counters` feature, which the workspace does not enable. | [source] |
| Native queries | `as_hal` is `unsafe` on device, adapter, buffer and texture. Metal `currentAllocatedSize`, DXGI and `VK_EXT_memory_budget` all route through it. | [source] for `as_hal`; [inferred] per-API |
| Windows backend | `Backends::PRIMARY`, with `WGPU_BACKEND` as an override. Which one a given machine selects is unverified. | [source] / unverified |
| Allocation names | Whether a wgpu label reaches `AllocationReport.name` was not traced. If so, Windows gets per-allocation attribution for free. | unverified |

`large-map-spatial-residency.md` reaches the same conclusions on wgpu 29, and they hold on 30.

## Meter audit

**SH.** `logical_occupancy_bytes` sums dense slots at `logical_bytes_per_dense_slot`, which is base plus direct source tiles only, and its comment says compose outputs are excluded. It also adds sparse source bytes. `active_capacity_bytes` counts physical capacity. `StreamingGpuPools` holds `base`, `total`, `direct_base`, `direct_intermediate`, `direct_total` and `depth_moments`. [source]

A physical slot of 1,664 B would fit BC6H base 64 + BC6H direct 64 + three Rgba16Float 8×8 tiles at 512 each, making logical about 128 B per slot, about 13× smaller. [inferred] from the texture list, not read from this map's header.

`ShStreamingAllocationSummary` exposes neither live dense slots, nor address high-water, nor the dense and sparse capacity split. [source] The state exists: `dense_slots` and `SparsePool` use `FirstFitRanges`, whose `capacity` is an address high-water and whose `free` map gives free runs. [source]

**Lightmap.** `resident_*_bytes` sum `BlockFacts` upload bytes, the block extent's payload. `pool_bytes` is requested texture bytes with the spare layer. Same texture formats and no compose outputs, so there is no SH-sized gap. It is still not exact: the numerator omits slot alignment rounding (`BlockFacts.texels` carries the rounded slot) and the denominator includes the spare. [source] Whether upload bytes equal physical bytes for each plane was not proven. Treat 180 of 490 MiB as indicative only.

**Lesson.** An agent read two honest meters and got a badly wrong answer, because their units differ and nothing says so. Hence the same-unit Decision.

## Capture as a host

`require_sync_proof_mode` bails on async mode, so capture cannot run play's streaming path. [source] Capture loads no mod manifest and renders world plus receivers, with no event loop. [source] `done/sh-probe-streaming--measurement-harness` lists camera paths and an event loop out of scope. So capture is a sound reader and a poor host, which is why the walk stays out of it.

## Existing walk harness

`session/lightmap_residency/walk_measurement/` is `#[cfg(test)]`, runs from `#[ignore]` tests and has no GPU: a CPU mirror of the pool, and SH does not stream. [source] It offers seeded `WalkKind` paths, `RUN_SPEED`, `Walk::build`, a deterministic `completed` reads mode, and `world_poses`: a `postretro-world-poses 1` pose file, a standing-eye grid at 1.5 m over floors with clearance rays, and record and replay. Its findings record the windowed game stalling at "Window ready" because an occluded window got no redraw while the owner used the machine (`done/spatial-residency--lightmap-cell-blocks/findings.md`, AC 23). No `Occluded` handling exists in `crates/postretro/src`. [source]

Not usable as the host, since it has no GPU and no SH. Usable as the route source.

## Overlaps

| Item | Relation |
|---|---|
| `large-map-spatial-residency.md` "Still needed" and its "no resident bytes per resource" learning | The lurking item. This closes the memory half. CPU frame time has shipped; no draw-count row was found in the dev panel. |
| Roadmap Epic 20 | No row covers memory. Nearest is "Streaming telemetry + structured log sink", which owns continuous streams. |
| `E20--scripted-run-capture` | Research only, undrafted. Its session-plus-renderer driver is the natural offscreen walk host. Open question. Its "one shared vocabulary, not a parallel scene format" argues for the route living with `RunSpec` if that host wins. |
| Roadmap "Record / replay" | A candidate route source, for an owner's recorded walk. |
| `gpu-pass-paired-ab` | Sibling tool, no shared code. Precedent for the feature gate, the `tools/` reducer and preflight. Its research leaves the generic launch harness to a later brief, which is this one. |
| `E20--frame-capture`, `agentic-observability`, `E20--live-socket-channel` | Done. The surfaces this extends. "Agentic" here means capture report, batch dump, live socket: machine-readable, one vocabulary. |
| `done/sh-probe-streaming--measurement-harness` | Origin of the row model. Its out-of-scope items (allocation tracking outside SH, OS memory) are taken up here; its no-camera-paths rule is kept. |
| `E25--map-transitions` | Consumer: acceptance row 19 wants peak resident bytes with two levels, and §Two-level peak memory is unmeasured. |
| SH pool overshoot investigation | Separate, out of scope. This brief supplies answerable numbers only. |

## Rivals

- **Owner-declared rows everywhere.** No type churn, matches `texture_row`. A forgotten owner vanishes silently, the defect at hand.
- **Input-driven walk.** Real collision and speed through `RunSpec` movement commands. Needs a path follower and stuck detection, or a stuck pawn reads as low memory.
- **External sampler only, over observe-live.** No in-engine walk: a human plays and an agent samples. Cheap, and works once the live section lands, but not unattended.
- **Counting global allocator.** Portable heap total and high-water, but `unsafe impl GlobalAlloc`. `dhat` and Tracy's allocator hooks are alternatives, not assessed.
- **Reuse capture with a camera path.** Unattended and windowless, but sync-proof and world-only, so it would not measure play.
