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
- One compiler-only drop policy. For every candidate record, bound each
  decoded f16 RGB texel of all 64 local probes in f32 through the actual
  compose operation. Accumulate componentwise absolute bounds per affinity
  cell: one `0.001` RGB budget for id 27's indirect output, and one shared
  `0.001` RGB budget for ids 41 and 45's direct output.
- The direct bound proves every promotion weight `w ∈ [0, 1]`, including the
  id-41 signed subtraction, non-negative clamp, and f16 output rounding. A
  documented conservative equivalent is allowed only when it bounds those
  operations for every output component and every `w`.
- Script-mutable animated descriptors in ids 27 and 45 are never droppable.
  Derive a compiler-only `script_mutable_descriptor_slots` mask from every
  script-membership-manifest target and every map-authored `_animated`
  reservation. Map each raw `MapLight` source index through the current
  `OctahedralShVolumeSection::slot_for_map_light` table, then into the
  descriptor and `AnimatedBakedLights` index spaces used by ids 27 and 45;
  ignore `ANIMATED_SLOT_NONE` targets because they are runtime-only and have
  no id-27/45 entry. Thread that mask into the drop policy and retain every
  marked entry. For
  immutable descriptors, bound runtime radiance with the supremum of the exact
  runtime Catmull–Rom curve over its complete cycle, including internal extrema
  and positive overshoot after runtime non-negative clamping. This changes
  storage eligibility only; it does not change curve or scripting behavior.
- Scan cells in increasing index order. Within each cell, preserve the source
  records' ascending index and index-parallel payload pairing. Recompute CSR
  offsets from retained records. A missing entry means zero; every retained
  entry keeps its current dense 64-probe payload stride.
- Preserve id-41 selected-light coverage. If a selection would lose every
  entry, retain its highest canonical cell record, even when it passes the
  drop test, so existing loader and pack contracts remain valid.
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
- [ ] For each affinity cell and runtime state, all 64 local-probe f16 RGB
  texels of dropped records remain within `0.001` per channel after the same
  compose semantics as runtime: id 27 has its own indirect budget; ids 41 and
  45 share one direct budget. Tests cover signed id-41 subtraction, clamp, f16
  rounding, the full `w ∈ [0, 1]` promotion interval (endpoints and an
  interior clamp-transition case), and cumulative dropped records.
- [ ] Script-mutable animated ids 27 and 45 entries are retained. Immutable
  animated entries use the supremum of the exact Catmull–Rom cycle, including
  internal extrema and positive overshoot, without changing authored curves or
  scripting behavior. Fixtures cover a manifest-derived static target, a
  runtime-only manifest target, an `_animated` target, an authored immutable
  curve, and a later
  `setLightAnimation` target.
- [ ] Dropping a section entry produces the same composed result as replacing
  that entry with a zero payload, within the output-path cumulative budget.
- [ ] Every id-41 selection slot remains represented by at least one retained
  entry. Existing promotion, direct-delta, and shadowmask contracts remain
  usable; no selected light is double-counted above the error bound.
- [ ] An all-empty id-27 keeps its current base-only behavior. An all-empty
  id-45 normalizes to absent before packing. Tests cover both cases.
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
without changing retained payloads. It must bound every decoded f16 RGB texel
of all 64 local probes through its output compose path, accumulate per-cell
componentwise absolute error against the separate indirect and shared-direct
budgets, and retain records that would exceed either budget. Prove the direct
bound for every promotion weight `w ∈ [0, 1]`, including the signed
subtraction, clamp, and f16 rounding; test both endpoints and an interior
clamp-transition. Derive `script_mutable_descriptor_slots` from every
manifest target and `_animated` reservation, mapping raw `MapLight` indices
through `OctahedralShVolumeSection::slot_for_map_light` into the descriptor
and `AnimatedBakedLights` namespaces before passing the mask to this policy.
Ignore `ANIMATED_SLOT_NONE` runtime-only targets: they have no id-27/45 entry.
Never drop a marked id-27 or id-45 entry. For immutable descriptors, use the
exact runtime Catmull–Rom cycle supremum, including internal extrema. Rebuild
by increasing cell index while preserving each cell's original ascending
index/payload pairing and recomputing offsets. If needed, retain the highest
canonical cell record for an id-41 selection. Unit-test compose bounds,
signed/clamped/f16 direct output, cumulative records, manifest-derived static
and runtime-only manifest fixtures, `_animated` mutable fixtures, an authored immutable curve, a later
`setLightAnimation` target, curve extrema, exact payload/order preservation,
and empty cells.

### Task 3: apply drops and enforce cap

Apply the policy after ids 27, 41, and 45 are baked and before packing. Keep
the current format, loader, render-cpu, renderer, and WGSL contracts intact.
Normalize an all-empty id-45 to `None` before packing; preserve id-27's
existing empty-section behavior. Emit the per-section drop summaries. Sum only
raw `delta_subblocks` payload bytes after dropping; if the total exceeds the
configured cap, return a named compile error before pack/write. Extend
compiler and loader-facing regression tests for id-41 selected-light coverage,
id-45 absence normalization, id-27 behavior, and deterministic compiled CSR
order.

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
| I7 — Script behavior unchanged | Task 2 | Task 3 application | Mutable-descriptor retention and curve-extrema tests |

## Risks

- The cap is a post-drop payload policy, not a host-memory solution for the
  upstream dense bake. The known 1.0 m stress OOM remains separate work.
- The 256 MiB cap is below the existing 2 GiB per-storage-binding requirement,
  but it is not a total VRAM budget. Runtime measurement remains required.
- The `0.001` bound is deliberately conservative but must remain a fixed safety
  policy for this feature; agents must not tune it to make a map fit.
