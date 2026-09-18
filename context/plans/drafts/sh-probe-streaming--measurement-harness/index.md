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
- `FrameTiming` already records per-pass timestamps when
  `POSTRETRO_GPU_TIMING=1` and the adapter grants both required timestamp features.
  Unsupported timing degrades to CPU frame measurements with an explicit reason.
- Renderer owns every `wgpu` call. Measurement code outside the renderer receives
  plain Rust report values, never GPU handles or `wgpu` types.
- Fine and coarse builds use the same source map, revision, render settings, camera,
  and receiver state. Only `--sh-probe-spacing` changes.

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
      unsupported/unavailable reason. Partial output is staged so failure never leaves a
      valid-looking report.
- [ ] CPU timing represents completed GPU work rather than command-enqueue time. The
      report names the completion strategy and cadence so two runs use the same method.
- [ ] CPU-only tests cover section sums, unknown section ids, allocation formulas,
      shared-allocation no-double-counting, scene validation, percentile calculation, and
      JSON absence semantics. GPU coverage remains ignored/on-demand and self-skips only
      when no adapter exists.
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

Move the existing SH texture allocation decisions and byte formulas out of the 1,200-line
`render/sh_volume.rs` into a focused renderer-internal module. Direct-SH,
billboard-scatter, and compose constructors consume the same allocation descriptions rather
than re-deriving extents or formats. Preserve every dummy, device-limit fallback, texture
format, usage, and binding. This task adds no logging.

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

Extend `CaptureScene` with an optional measurement block containing report path, warmup
frames, and sample frames. Validate positive bounded counts and reject output aliases.
Render through the extracted capture path without per-sample readback. Add a renderer API
that submits and completes measurement frames and exposes the existing GPU-timing snapshot
plus adapter identity as plain report data. Write one staged JSON report, then capture the
existing PNG. Keep `POSTRETRO_GPU_TIMING=1` as the sole timestamp feature gate.

### Task 7: Run and record the premise read

Create `measurements/premise.md` and retain small JSON reports beside it. Preferred run:
same production-quality stress-map source on the GTX 1660 Super, release cold bakes at
1.0 m and 8.0 m spacing with `--no-cache` and `RAYON_NUM_THREADS=8`, fixed
camera/resolution, 120 warmup frames, 600 sampled frames, and three alternating runs per
variant. Record median of run medians, p95s, per-pass windows, disk bytes, and SH resident
bytes. Delete generated PRLs, scratch caches, and unneeded PNGs after recording.

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

The finding distinguishes a successful negligible A/B from an unavailable measurement.
The former triggers an owner go/no-go discussion. The latter retains the owner's stated
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
