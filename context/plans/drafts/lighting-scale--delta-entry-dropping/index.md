# Lighting Scale — Conservative Delta Entry Dropping

## Goal

Reduce desktop map lighting storage and compose-buffer traffic without changing
probe density, runtime atlas layout, or any SH sampler. Omit a delta CSR entry
only when its omitted runtime contribution is bounded below a fixed cumulative
error budget.

The base indirect atlas already stores only valid probes at rest. This feature
preserves that landed contract and applies only to ids 27, 41, and 45.

## Scope

### In scope

- Conservative entry dropping for the existing sparse delta sections:
  `DeltaShVolumes` (id 27), `DirectShDeltaVolumes` (id 41), and
  `AnimatedDirectShDeltaVolumes` (id 45).
- One compiler-only drop policy. It evaluates f16 payloads in f32 and bounds
  the cumulative omitted RGB radiance in each affinity cell at `0.001` per
  channel over all supported runtime states.
- Runtime-radiance bounds: ids 27 and 45 include maximum non-negative
  authored color, intensity, brightness, and curve interpolation scale; id 41
  covers promotion weights in `[0, 1]` and its direct-light residual.
- Rebuild each section's existing CSR from retained `(cell, light, subblock)`
  records in canonical cell/light order. A missing entry means zero; every
  retained entry keeps its current dense 64-probe payload stride.
- Preserve id-41 selected-light coverage. A selected slot retains at least one
  CSR entry even if all of its entries pass the drop test, so existing loader
  and pack contracts remain valid.
- `--sh-delta-max-size` compiler option. Default: 256 MiB. It caps aggregate
  raw `delta_subblocks` bytes across ids 27, 41, and 45 after dropping. A
  compile over the requested cap fails before packing and names the three
  payload sizes and overage. CLI override is explicit; no map KVP exists.
- One info summary per delta section: input, retained, and dropped entries;
  raw payload and CSR bytes; largest accepted bound. One cap summary per bake.
- Manual measurements on campaign-test and stress-warren, including section
  bytes, load-time host-memory peak, resident delta buffers, compose timings,
  and active/inactive promotion and animation states on the desktop target.

### Out of scope

- Adaptive probe density, L1/L2 storage, sampler indirection, depth-moments
  format changes, and compacted composed-atlas VRAM.
- Any change to id-34 valid-probe compaction or id-35 direct-atlas storage.
- PRL section-version bumps, new delta fields, validity-dependent delta
  payload strides, loader format changes, or renderer/WGSL changes.
- Automatic quality reduction to fit the cap. The cap fails loud.
- Solving the known host-RAM OOM before delta sections are materialized.

## Acceptance criteria

- [ ] Retained ids 27, 41, and 45 entries keep the existing fixed dense
  64-probe stride and canonical CSR order. Their existing parsers, loaders,
  render-cpu buffer builders, renderer uploads, and WGSL loops need no change.
- [ ] For each affinity cell and runtime state, the f32 RGB sum of all dropped
  terms is at most `0.001` per channel. Tests cover authored light color and
  intensity, brightness/color curves including interpolation extrema, inactive
  descriptors, and promotion weights 0 and 1.
- [ ] Dropping a section entry produces the same composed result as replacing
  that entry with a zero payload, within the cumulative error bound.
- [ ] Every id-41 selection slot remains represented by at least one retained
  entry. Existing promotion, direct-delta, and shadowmask contracts remain
  usable; no selected light is double-counted above the error bound.
- [ ] An all-empty id-27 or id-45 CSR remains structurally valid under current
  format rules. Existing loader behavior remains: id-27 loads as base-only;
  id-45 normalizes to absent.
- [ ] The aggregate post-drop raw delta payload defaults to at most 256 MiB.
  `--sh-delta-max-size` overrides that default. An over-cap bake exits nonzero,
  emits no PRL, and reports per-section bytes, cap, and overage.
- [ ] No renderer, renderer CPU, loader, level-format, or WGSL production code
  changes. The dense indirect/direct composed atlases and all SH sampling
  behavior remain unchanged.
- [ ] Compiler output is deterministic for identical inputs. Focused tests and
  byte comparisons pin canonical CSR rebuilding and retained payload order.
- [ ] Findings record before → after section/payload ratios, peak host memory,
  resident delta-buffer bytes, and 120-frame compose timings on the desktop
  target. They state that dense composed-atlas VRAM is unchanged.

## Tasks

### Task 1: compiler seams and cap option

Split the oversized compiler CLI and pipeline seams needed by this feature
before extending them. Extract the size-option parsing path from `main.rs` and
the post-bake delta-section handoff from `pipeline.rs` into focused modules,
with behavior-preserving tests. Add `--sh-delta-max-size`, parsing the same
human-readable byte syntax as existing size options, defaulting to 256 MiB.
Thread the resolved cap through the compiler configuration to the post-bake
handoff; it has no wire, FGD, or runtime representation.

### Task 2: bounded drop policy and canonical CSR rebuild

Add a focused compiler module that evaluates and drops existing delta entries
without changing their retained payloads. It receives decoded section data and
the corresponding runtime radiance inputs, calculates a conservative
per-entry bound in f32, accumulates bounds per cell, and keeps entries when
the cell would exceed the `0.001` RGB budget. Rebuild offsets, light lists, and
subblocks from kept records in canonical order. Preserve a deterministic final
id-41 entry per selected slot. Unit-test exact payload/order preservation,
empty cells, cumulative bounds, radiometric scales, curves, and id-41
promotion-weight residuals.

### Task 3: apply drops and enforce cap

Apply the policy after ids 27, 41, and 45 are baked and before packing. Keep
the current format, loader, render-cpu, renderer, and WGSL contracts intact.
Emit the per-section drop summaries. Sum only raw `delta_subblocks` payload
bytes after dropping; if the total exceeds the configured cap, return a named
compile error before pack/write. Extend compiler and loader-facing regression
tests for id-41 selected-light coverage, all-empty id-45 normalization, and
deterministic compiled CSR order.

### Task 4: measure desktop behavior

Compile campaign-test and stress-warren at the recorded settings. Record
before → after raw payload and full-section bytes for all three deltas, cap
result, and map compile behavior. On GTX-16-series-class and Radeon Pro
5500M-class hardware, record delta-buffer bytes, load-time host-memory peak,
total process/GPU memory, and 120-frame GPU timings for indirect/direct/
animated-direct compose under active and inactive relevant lights. State the
unchanged dense composed-atlas footprint and any map that still exceeds the
desktop cap.

## Sequencing

**Phase 1 (sequential):** Task 1 — extracts the configuration and post-bake seams Task 3 consumes.

**Phase 2 (sequential):** Task 2 — establishes the bounded drop and CSR-rebuild contract.

**Phase 3 (sequential):** Task 3 — consumes Tasks 1–2 and changes compiler output.

**Phase 4 (sequential):** Task 4 — measures the integrated compiler and runtime result.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP | CLI |
|---|---|---|---|---|
| Delta payload cap | compiler configuration | n/a | n/a | `--sh-delta-max-size` |
| Dropped entry | existing CSR absence | unchanged existing empty cell range | n/a | n/a |

## Wire format

No format changes. IDs 27, 41, and 45 retain their current section versions,
header fields, fixed 64-probe subblock stride, CSR offsets, and light-index
spaces. Dropping only omits a current CSR record and its index-parallel dense
subblock; absence already means zero contribution.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1 — Sampling stays dense | Existing renderer | Tasks 1–3 must not change runtime contracts | Renderer/WGSL diff audit; boot and visual A/B |
| I2 — Retained stride/order | Task 2 | Task 3 packaging | Format tests; deterministic CSR tests |
| I3 — Bounded omitted radiance | Task 2 | Task 3 policy inputs | Runtime-scale, cumulative, and zero-payload compose tests |
| I4 — Promoted-light integrity | Task 2 | Task 3 id-41 application | Coverage and promotion regression tests |
| I5 — Explicit cap | Task 1 | Task 3 enforcement | CLI and over-cap compile tests |
| I6 — No silent quality fit | Task 3 | Task 4 measurements | Named failure test; findings |

## Risks

- The cap is a post-drop payload policy, not a host-memory solution for the
  upstream dense bake. The known 1.0 m stress OOM remains separate work.
- The 256 MiB cap is below the existing 2 GiB per-storage-binding requirement,
  but it is not a total VRAM budget. Runtime measurement remains required.
- The `0.001` bound is deliberately conservative but must remain a fixed safety
  policy for this feature; agents must not tune it to make a map fit.
