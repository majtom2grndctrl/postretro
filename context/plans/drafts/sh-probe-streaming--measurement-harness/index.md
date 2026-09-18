# SH Probe Streaming — Measurement Harness

## Goal

Build reusable disk-footprint, renderer-accounted SH-residency, and steady-state
frame-time instruments before SH streaming. Record whether reducing whole-level SH
footprint moves frame time without treating an unavailable harness as evidence against
streaming.

## Shared context

- This is Slice 1 of `sh-probe-streaming`. It adds measurement only. It does not add
  clustering, partial reads, residency selection, upload/eviction, or authored hints.
- The compiler currently logs only section count, aggregate payload bytes, and largest
  payload after `write_and_validate_sections` writes a PRL.
- `Renderer::install_level_geometry` is the level-owned GPU install boundary.
  `ShVolumeResources`, `DirectShResources`, `BillboardDirectScatterResources`, and the
  SH compose resources allocate the textures and buffers attributed here.
- Offscreen capture enters through `CaptureScene`, `run_capture_inner`,
  `Renderer::new_offscreen`, and `Renderer::capture_frame_indirect`. It renders one
  static VM-free instant and currently returns a PNG only.
- `FrameTiming` is `Some` only when `POSTRETRO_GPU_TIMING=1` and the adapter grants
  both `TIMESTAMP_QUERY` and `TIMESTAMP_QUERY_INSIDE_ENCODERS`. Disabled or unsupported
  GPU timing is absent today. An unsupported request logs a warning only.
- Renderer owns every `wgpu` call. Measurement code outside the renderer receives
  plain Rust report values, never GPU handles or `wgpu` types.
- Fine and coarse builds use the same source map, revision, render settings, camera,
  and receiver state. Only `--sh-probe-spacing` changes.

## Measurement schema v1

Capture JSON adds an optional `measurement` block:

| Field | Type | Bounds | Notes |
|---|---|---|---|
| `report` | string path | non-empty; must not alias scene, map, or PNG output | Path is resolved like `output`: relative to the current working directory. Parent must exist. Staged sibling rename publishes it last. |
| `warmup_frames` | integer | `1..=10000` | Warmup frames prepare timing state. They never enter sample statistics. |
| `sample_frames` | integer | `1..=100000` | Number of completed sample frames recorded for CPU timing. |

Omitting `measurement` preserves legacy capture behavior and writes no report. Unknown
measurement fields are rejected. `report` may replace a regular file but not a directory,
symlink alias, scene, map, or PNG output. The report stays staged until the final PNG is
published. Any later failure removes the staged report. The visible report is published
last.

Report root:

| Field | Unit / type | Meaning |
|---|---|---|
| `schema` | string | Always `postretro.capture.measurement.v1`. |
| `revision` | string or null | Git revision when available. Null if the tree is unavailable. |
| `map.path` / `map.bytes` | path / bytes | Input PRL path and file size. |
| `capture.output` | path | PNG output path. |
| `capture.resolution` | pixels | `[width, height]`. |
| `capture.camera` | scene units / degrees | Exact parsed camera. |
| `workload.warmup_frames` / `workload.sample_frames` | frames | Validated counts. |
| `adapter.name` / `backend` / `device_type` | strings | Plain adapter identity retained by renderer. |
| `renderer_accounted_sh` | bytes | Ledger rows plus no-double-count total. |
| `cpu_completion` | milliseconds | `strategy`, `cadence`, raw samples, median, p95. |
| `gpu_timing` | object | `availability`, optional `reason`, optional 120-frame windows. |

Timing availability strings are exact: `available`, `not-requested`, `unsupported`,
`plain-build-unavailable`, `not-yet-windowed`. Availability precedence is strict:
`not-requested`, `unsupported`, and `plain-build-unavailable` win before window analysis.
Use `not-yet-windowed` only when `FrameTiming` is active but no full sample window
completes. Reasons are exact:
`env-disabled`, `adapter-missing-timestamp-features`, `dev-tools-accessor-unavailable`,
`window-not-complete`. Optional numeric values are omitted until present, never encoded as
zero. Optional SH allocation rows are either absent because the source section is absent
or present with `bytes: 0` only when a real zero-byte source was accepted. Dummy/fallback
rows are present and marked.

CPU completion timing must measure completed GPU work. The renderer owns the completion
mechanism and report string, initially `device-poll-wait-after-submit`. Cadence is
`once-per-sample-frame`: submit one prepared static render, wait for completion through
the renderer-owned API, then record elapsed wall time. It must not measure command enqueue
time.

Warmup and samples are isolated. Warmup frames may drive `FrameTiming` state but their CPU
samples are discarded. Drop/reset `FrameTiming` window state at the warmup-to-sample
boundary. GPU windows are 120 completed sample-frame `FrameTiming` samples. Report every
full window that completes during samples with pass labels, average milliseconds, and
skip counts. A partial trailing window is reported only as `partial_frames`; it contributes
no per-pass average. If timing is otherwise active and no full sample window completes,
`gpu_timing.availability` is `not-yet-windowed` with reason `window-not-complete`.

## SH residency ledger v1

| Family | Owner / current source | Source ids | Formula source | Dummy / fallback | Total |
|---|---|---|---|---|---|
| Base indirect atlas | `ShVolumeResources`, `sh_volume.rs` | 34 | `compact_base_atlas_allocation` + `base_atlas_allocation_bytes`; BC6H physical 4x4 blocks | Missing/unusable/device-limit fallback binds 1x1 Rgba16Float. Accepted empty BC6H allocates one 4x4 BC6H block. Accepted empty Rgba16Float allocates 1x1 Rgba16Float. | yes |
| Depth-moment texture | `ShVolumeResources`, `sh_volume.rs` | 34 | actual `Rgba16Uint` extent | Missing/empty/limit fallback binds 1x1x1 dummy | yes |
| SH grid info buffer | `ShVolumeResources`, `sh_volume.rs` | 34 | `build_grid_info_bytes` binding size | Always non-empty | yes |
| Animated-light descriptors and samples | `AnimatedLightBuffers`, `sh_volume.rs` | 34 plus scripted reserve | created buffer contents length, including scripted sample reserve | One dummy record when no animated lights. Ids 27/45/48 may reference descriptor indices but do not own this payload. | yes |
| Scripted-light descriptors | `ShVolumeResources`, `sh_volume.rs` | runtime reserve | created buffer contents length | One dummy descriptor when empty | yes |
| Direct SH base atlas | `DirectShResources`, `direct_sh_resources.rs` | 35 | direct atlas extent/format used for creation | Missing/empty/limit fallback binds 4x4 BC6H dummy | yes |
| Direct SH dynamic params | `DirectShResources`, `direct_sh_resources.rs` | 35/41/45 | `build_dynamic_direct_params_bytes` length | Always present | yes |
| Direct SH composed / intermediate atlas | `DirectShResources`, `direct_sh_resources.rs` and `direct_sh_compose.rs` | 35/41 | actual composed texture descriptor | Only when compose path needs it | yes, once |
| Animated direct SH storage, indirection, grid, and scale buffers | `animated_direct_sh_compose.rs` | 45 | exact storage/uniform bytes used for bind groups | Empty payloads padded to valid binding minimums | yes |
| Billboard direct-scatter base volume | `BillboardDirectScatterResources`, `billboard_direct_scatter.rs` | 47 | `width * height * depth_or_array_layers * Rgba16Float` bytes | Missing/limit fallback binds dummy and clears scatter | yes |
| Billboard direct-scatter composed volume | `BillboardDirectScatterResources`, `billboard_direct_scatter.rs` and billboard compose module | 47/48 | `width * height * depth_or_array_layers * Rgba16Float` bytes | Only when animated deltas compose | yes, once |
| Indirect delta buffers | `ShComposeResources`, `sh_compose.rs` | 27 | padded storage bytes: subblock, light, and descriptor-index payloads use minimum nonzero padding; affinity offsets come from affinity grid/cell metadata then pad | Empty buffers padded to valid binding minimums | yes |
| Probe indirection buffer | `ShComposeResources`, `sh_compose.rs` / `sh_indirection` | 34 | `probe_indirection_storage_bytes` | Invalid probes still encoded as sentinel words | yes |
| Delta compaction metadata buffer | `ShComposeResources`, `sh_compose.rs` | 27 | `compaction_meta_words` generated from affinity grid/cell metadata, then padded | Empty metadata padded to valid binding minimum | yes |
| Compose grid/origin buffers | `ShComposeResources`, direct compose, animated direct compose, billboard compose | 27/41/45/48 | exact uniform bytes used for bind groups | Always non-empty when owning compose resource exists | yes |
| Direct and billboard delta CSR buffers | `direct_sh_compose.rs`, `animated_direct_sh_compose.rs`, billboard compose | 41/45/48 | padded storage bytes: subblock, light, and descriptor-index payloads use minimum nonzero padding; affinity offsets come from affinity grid/cell metadata then pad | Empty buffers padded to valid binding minimums | yes |

Every ledger row cites source section ids when data-backed and `derived` when created only
to compose another resident resource. Shared compose targets have one owner row with
multiple source ids; consumers cite the owner and do not add bytes again.

## Scope

### In scope

- A per-section PRL payload report derived from the descriptors actually written.
- A structured, renderer-accounted SH resident-byte report at level install.
- Optional repeated-frame measurement in the existing offscreen capture workflow.
- Machine-readable measurement output with adapter, workload, sample, and fallback
  metadata.
- A bounded A/B protocol and a committed Slice 1 finding.

### Out of scope

- Driver-reported process VRAM, OS-wide memory, PCIe traffic, cache-miss counters, or
  external profiler integration.
- General renderer allocation tracking outside SH.
- Dynamic gameplay, scripts, particles, camera paths, or an event loop in capture.
- Any SH streaming implementation or PRL format change.
- CI performance thresholds or cross-adapter pixel/timing comparisons.

## Measurement definitions

| Term | Definition |
|---|---|
| PRL payload bytes | Sum of every emitted `SectionDescriptor.byte_len`; excludes container header and section-table bytes. |
| Renderer-accounted SH bytes | Exact requested byte sizes of level-owned SH textures and buffers created from ids 34, 35, 27, 41, 45, 47, and 48, plus separately named shared/derived compose allocations. Excludes driver padding and unrelated renderer resources. |
| Steady-state frame time | Repeated rendering of one prepared static capture scene after warmup, with PNG readback excluded from timed samples. Report median and p95 CPU completion time. |
| Per-pass GPU time | Existing `FrameTiming` pass averages when timestamp queries are enabled and supported. Absence is reported, never encoded as zero. |
| Material delta | A repeatable A/B difference larger than run-to-run noise and useful to the owner. The report carries raw values; this brief does not invent a universal millisecond threshold. |

## Acceptance criteria

- [ ] A compiler run logs one row for every emitted PRL section with stable section id,
      known `SectionId` name when available, and exact payload bytes. The reported rows
      sum exactly to the existing aggregate payload count and to the payload portion of
      the flushed file.
- [ ] Section-footprint accounting is derived from the `PlannedSection` descriptors
      passed to the writer. Optional sections appear iff emitted. Unknown future ids are
      reported by number instead of dropped or panicked.
- [ ] Level install emits one structured SH resident-byte report. It names ids 34, 35,
      27, 41, 45, 47, and 48 separately when present; names shared/derived allocations
      separately; reports dummies/fallbacks explicitly; and produces a no-double-count
      total for the allocations it covers.
- [ ] Resident-byte formulas share the same format, extent, layer, and device-fallback
      decisions used to create each GPU resource. BC6H uses physical 4x4 block bytes;
      storage/uniform buffers use their allocated binding size, including non-empty dummy
      allocations where wgpu forbids zero-sized bindings.
- [ ] Existing PRL bytes, capture PNG bytes on one adapter, render pass order, bind-group
      layouts, and SH sampler behavior remain unchanged when measurement is not requested.
- [ ] A capture scene may request measurement output while preserving the existing
      single-frame scene vocabulary and PNG output. Omission keeps the legacy one-frame
      path byte-compatible.
- [ ] Measurement mode prepares the level, textures, lights, receiver draws, visibility,
      and camera once; runs a validated warmup count; then records a validated sample
      count without PNG readback inside the timed loop. It writes the PNG after timing so
      the scene remains inspectable.
- [ ] The JSON measurement report records revision when available, map path and file
      size, resolution, camera, warmup/sample counts, adapter name/backend/device type,
      renderer-accounted SH bytes, CPU median/p95, and per-pass GPU samples or a named
      availability reason from schema v1. JSON stays staged until the PNG is successfully
      published; the report publishes last. Any later failure cleans the staged report so
      failure never leaves a valid-looking report.
- [ ] CPU timing represents completed GPU work rather than command-enqueue time. The
      report names the completion strategy and cadence so two runs use the same method.
- [ ] CPU-only tests cover section sums, unknown section ids, allocation formulas,
      shared-allocation no-double-counting, scene validation, percentile calculation, and
      JSON absence semantics: no report when `measurement` is omitted, named timing
      reasons, and absent-versus-zero optional SH rows. GPU coverage remains ignored/on-demand
      and self-skips only when no adapter exists.
- [ ] `measurements/premise.md` records the bounded A/B protocol, exact commands, source
      map, map revision/hash, fine/coarse spacing, worker count, cache mode, profile,
      camera, resolution, warmup/samples, machine/adapter/driver, section bytes,
      renderer-accounted SH bytes, CPU median/p95, per-pass GPU values where available,
      raw report paths, cleanup, and interpretation.
- [ ] A successful representative A/B with negligible delta is recorded as evidence that
      challenges the runtime-performance premise and is surfaced to the owner before
      Slice 2. Failure to obtain a runnable automated harness is recorded as
      `not-yet-evaluable`, not as a failed premise and not as a reason to discard SH
      streaming.
- [ ] If automated capture cannot run within the bounded attempts, the finding records at
      least one owner-approved observational fallback when hardware is available:
      `campaign-test` or another mid-sized map on this MacBook Pro, or the largest runnable
      stress map on the GTX 1660 Super. Fine/coarse builds and observation conditions stay
      identical except for SH spacing.

## Tasks

### Task 1: Extract the PRL output seam

Split the planned-section descriptor, output writer, readback validation, and their focused
tests out of the 3,900-line `crates/level-compiler/src/pack.rs`. Keep
`pack_and_write_portals_with_billboard_scatter` as the section producer and preserve the
write-one-payload-at-a-time lifetime. This task is behavior-preserving and adds no report.

### Task 2: Extract capture orchestration seams

Split capture-only renderer methods out of the 1,100-line
`renderer_render_frame.rs`, and split scene preparation/receiver setup from the 1,600-line
`capture/driver.rs`. The prepared state must own or borrow everything needed to render the
same static frame repeatedly without rerunning PRL load, texture install, visibility, or
receiver collection. Keep the public one-frame behavior unchanged. This task adds no timing
mode.

### Task 3: Extract SH allocation decisions

Move the existing SH allocation decisions and byte formulas out of their current decision
sites into a focused renderer-internal module. Current sites include the about-2,000-line
`render/sh_volume.rs`, `direct_sh_resources.rs`, `billboard_direct_scatter.rs`,
`sh_compose.rs`, `direct_sh_compose.rs`, `animated_direct_sh_compose.rs`, and billboard
scatter compose code. Direct-SH, animated-direct-SH, billboard-scatter, and compose
constructors consume the same allocation descriptions rather than re-deriving extents or
formats. Preserve every dummy, device-limit fallback, texture format, usage, and binding.
This task adds no logging.

### Task 4: Report PRL section footprint

Build a pure footprint report from the extracted planned-section descriptors before their
`FnOnce` encoders are consumed. The output seam logs all rows and the aggregate after
readback validation succeeds. Tests construct known and unknown ids, optional section sets,
and a minimal written container; assertions compare descriptor sums to the file payload
after subtracting header/table bytes.

### Task 5: Report renderer-accounted SH residency

Add a plain Rust allocation ledger owned by the renderer. Each SH resource constructor
records its actual allocation description into the ledger at creation; shared compose
targets have one owner and may cite multiple source ids without appearing twice in the
total. `Renderer::install_level_geometry` emits and retains the finished report so capture
can serialize it after install. Do not estimate by reserializing loaded sections.

### Task 6: Add repeated offscreen measurement

Extend `CaptureScene` with the schema v1 optional measurement block containing report path,
warmup frames, and sample frames. Validate positive bounded counts and reject output aliases.
Render through the extracted capture path without per-sample readback. Add a renderer API
that submits and completes measurement frames and exposes the existing GPU-timing snapshot
plus adapter identity as plain report data. Task 6 adds production/plain availability
reasons even though the current accessor is dev-tools-gated. Write one staged JSON report,
capture and publish the existing PNG, then publish the JSON report last. Keep
`POSTRETRO_GPU_TIMING=1` as the sole timestamp feature gate.

### Task 7: Run and record the premise read

Create `measurements/premise.md` and retain small JSON reports beside it. Preferred run:
same production-quality stress-map source on the GTX 1660 Super, release cold bakes at
1.0 m and 8.0 m spacing, fixed camera/resolution, 120 warmup frames, 600 sampled frames,
and three alternating runs per variant. Record median of run medians, p95s, per-pass
windows, disk bytes, and SH resident bytes.

Exact command shape:

```bash
RAYON_NUM_THREADS=8 cargo run -p postretro-level-compiler -- <stress.map> -o measurements/sh-probe-streaming/premise/fine-1.0m.prl --release --sh-probe-spacing 1.0 --no-tui
RAYON_NUM_THREADS=8 cargo run -p postretro-level-compiler -- <stress.map> -o measurements/sh-probe-streaming/premise/coarse-8.0m.prl --release --sh-probe-spacing 8.0 --no-tui
POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- capture measurements/sh-probe-streaming/premise/fine-1.0m.scene.json
POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- capture measurements/sh-probe-streaming/premise/coarse-8.0m.scene.json
```

Scene files point `map` at the matching PRL, `output` at a throwaway PNG under
`measurements/sh-probe-streaming/premise/`, and `measurement.report` at the matching
run JSON. `--release` selects the exact cold bake and bypasses cache like `--no-cache`;
do not claim the Cargo release profile does this. Keep the same input map, revision,
settings, scene, camera, receiver state, machine, adapter, and driver except for
`--sh-probe-spacing`. Delete generated PRLs, scratch caches, and unneeded PNGs after
recording.

Bounded fallback order:

1. Attempt the automated offscreen run once on this MacBook Pro and once on the GTX 1660
   Super when available. A missing adapter, device loss, unsupported timestamp queries, or
   failure to render the large PRL ends that attempt after its diagnostic is captured.
2. On this MacBook Pro, observe the existing 120-sample window-title frame-time read on
   identical fine/coarse `campaign-test` builds at the same stationary pose. Record at
   least three settled windows per variant. Use per-pass values only if GPU timing is
   stable.
3. On the GTX 1660 Super, repeat the observation on the largest stress map that boots.
   Record a full-map failure as `not-yet-evaluable`; do not substitute a different map
   without naming it.

Terminal status is one of `measured`, `manual-observation`, or `not-yet-evaluable`.
`measured` requires automated fine/coarse JSON reports. `manual-observation` requires map,
machine, adapter, driver when known, revision, exact PRL paths or hashes, spacing values,
camera/pose description, resolution, window-title CPU frame-time windows, GPU timing
availability, and every diagnostic that blocked automation. `not-yet-evaluable` requires
the attempted machines, map, commands, failure diagnostics, and why no approved observation
was possible. The owner has approved the observational fallback for this slice. Do not add
a new approval gate.

The finding distinguishes a successful negligible A/B from unavailable measurement. The
former triggers an owner go/no-go discussion. The latter retains the owner's stated
direction that PostRetro still needs SH streaming and does not block later slice drafting.

## Sequencing

**Phase 1 (concurrent):** Tasks 1, 2, 3 — independent behavior-preserving splits.

**Phase 2 (concurrent):** Task 4 consumes Task 1; Task 5 consumes Task 3.

**Phase 3 (sequential):** Task 6 consumes Tasks 2 and 5.

**Phase 4 (sequential):** Task 7 consumes all instruments and records the premise read.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Measurement-off output is unchanged | Tasks 1–3 | Report hooks and capture scene extension | AC 5, 6; existing capture and compiler tests |
| Every reported total is no-double-count | Tasks 4–5 | Optional sections and shared compose allocations | AC 1–4; pure sum tests |
| Timed work is the real static render workload | Task 6 | Accidental reload, recollect, PNG readback, or enqueue-only timing | AC 7–9; report metadata and ignored GPU test |
| GPU work stays renderer-owned | Tasks 3, 5, 6 | Adapter identity, completion, timestamps, allocation accounting | Shared context; review |
| Unavailable is not negligible | Task 7 | Adapter/device loss and oversized-map failure | AC 11–12; premise record |

## Open questions

None. The owner chooses whether a successful negligible result stops the epic. This brief
only ensures that an unavailable automated harness cannot make that decision by accident.
