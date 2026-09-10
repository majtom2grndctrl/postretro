# Compile-Time Peak RAM — SH-Delta Bake

## Goal

Make prl-build **survive and measure** production-scale SH-delta bakes instead of
OOMing mid-build: refuse a map that would exhaust host memory *before* baking it,
with a diagnostic that names which lights drive the demand, and shrink the
serialize stage's peak by not holding several whole-payload copies at once.
Emitted `.prl` bytes stay byte-for-byte identical — this is an allocation-lifecycle
and pre-bake-refusal change, not a format, quality, or bake-math change.

Refusing is the outcome for a map over budget; *compiling* such a map is the job
of forward-predicted adaptive density (out of scope). This spec is what lets the
compiler reach that decision without crashing, and it produces the per-light
measurement that adaptive-density work consumes.

## Epic: compile-time peak RAM

One track of a shared effort — **bound prl-build's peak RAM across every heavy
bake stage.** The anti-pattern is common: a bake materializes its whole output
payload (times a copy-chain of drop, compaction, and serialization buffers) in
host RAM before writing.

- **SH-delta track (this spec).** The direct/indirect/animated SH-delta bakes.
- **Lightmap track.** `lighting-scale--lightmap-bake-incremental-flush` (draft) —
  the same allocation-lifecycle pattern for the lightmap atlas bake. Sibling
  under this contract, not restated here.
- **Shared contract.** Emitted `.prl` bytes byte-identical; only allocation
  lifetime and pre-bake acceptance change.
- **Asymmetry between the tracks.** The lightmap bake partitions cleanly by atlas
  layer, so incremental flush lets a previously-OOMing lightmap bake *complete*.
  The SH-delta bake cannot: valid-probe compaction depends on `cell_levels`
  decided by a whole-section coarsening fix-point over the dense payload, so the
  dense buffer must stay resident and cannot be streamed away. This track
  therefore *gates* the hard case rather than completing it.
- **Follow-up, not a track.** A survey of the remaining bakes (shadowmask,
  cell-visibility, chunk-light-list) for the same anti-pattern. Forward-predicted
  adaptive density and runtime probe streaming are separate efforts (see Out of
  scope).

## Scope

### In scope

- A **pre-bake working-set gate** on the three SH-delta bakes: estimate peak host
  memory from the CSR before any sub-block is baked, and refuse the compile with a
  per-light diagnostic when it exceeds a configurable budget. The compiler weighs
  the map before lifting it.
- The gate's projection and per-light histogram as the **measurement substrate**
  for forward-predicted adaptive density, and its over-budget branch as the
  **seam** where a future "coarsen instead of refuse" attaches.
- **Serialize-streaming**: write `.prl` sections to the output file incrementally,
  freeing each large section's payload after it is written, so the serialize stage
  never holds several whole-payload copies at once. Bytes on disk unchanged.
- Compile-time footprint reporting sufficient to verify the win and surface a
  map's SH-delta demand, matching the house logging discipline.

### Out of scope

- **The emitted-section cap / loadability limits.** The per-section loader floor
  (128 MiB per `lighting-scale--sh-adaptive-coarsening-v2`, the WebGPU spec floor)
  and the aggregate bake cap (`DeltaSectionConfig.max_payload_bytes`, 256 MiB) stay
  exactly as the owner locked them. The engine is driving the SH-delta binding
  *down* toward 128 MiB (`perf-animated-sh-light-culling`); a peak-RAM spec must
  not touch that regime. `enforce_payload_cap` is unchanged.
- **Bounding the dense intermediate itself.** The dense buffer stays resident
  bake-through-coarsening (see Asymmetry above); that residency is the RAM floor
  this spec accepts. Lowering it means restructuring coarsening — separate, and
  adjacent to adaptive density.
- **Forward-predicted adaptive density.** The lever that makes an over-budget map
  *compile*. Separate; this spec's gate feeds it and leaves the seam for it.
- **Runtime probe / asset streaming, runtime residency, GPU bindings.** On-disk
  bytes byte-identical; the runtime load path is untouched. Runtime streaming is
  the `large-map-spatial-residency` follow-up.
- **PRL format / wire changes.** No new section, no version bump, no field added.
- **Changing device limits.** `REQUIRED_STORAGE_BUFFER_BINDING_SIZE` stays.
- **The lightmap track**, owned by `lighting-scale--lightmap-bake-incremental-flush`.
- **Bake math, probe values, levels, drop, or coarsening output.** Untouched.

## Direction

**Problem.** prl-build holds each bake's whole output payload — times a
copy-chain — in host RAM before writing, and the SH-delta dense payload must
additionally stay resident through a whole-section coarsening pass. Peak RAM
therefore scales with total payload, unbounded and unguarded, so a dense stress
map exhausts memory mid-build — *after* the bake completes, in valid-probe
compaction, where the finished dense result is copied into its compacted buffer.
See `research.md` for the copy-chain and the exact byte decomposition.

**Prior commitments.** `warm-cache--delta-sh-and-graph-bakes` (in-progress) built
the shared per-entry helper the three delta bakes route through and commits to
byte-identical output; this work builds on that helper and sequences after it.
`lighting-scale--lightmap-bake-incremental-flush` establishes the peak-RAM /
allocation-lifecycle pattern and byte-identity contract this spec mirrors, and
explicitly handed the SH-delta case to "a separate spec" — this one. The
emitted-section cap and the 128 MiB per-section loader floor are an owner decision
(`lighting-scale--sh-adaptive-coarsening-v2`, 2026-08-28) that the SH-delta
binding is being driven toward, not away from (`perf-animated-sh-light-culling`) —
this spec does not touch them. No divergence from a prior commitment.

**Alternatives rejected.**
- *Raise the emitted-section cap to the runtime binding limit (2 GiB − 8).* The
  binding limit is a documented stopgap the engine is removing; raising a bake cap
  to it would 16× the locked 128 MiB per-section floor, emit sections the loader
  still rejects, and bake content that becomes unloadable when the stopgap goes.
  It also does not address the dense intermediate that actually OOMs. Rejected;
  the cap regime stays with the work that owns it.
- *Full per-entry streaming of bake → coarsen → compact.* Blocked: the coarsening
  level decision is a whole-section fix-point over the dense payload with
  neighbor-coupled smoothing, so the dense buffer cannot be dropped per-entry
  without restructuring the envelope — large, and adaptive-density-adjacent.
- *Forward-predicted density now.* The real payload-reduction lever, but a
  separate, larger effort. This spec makes the compiler survive and measure first,
  and leaves the seam so that work slots into the gate's over-budget branch.

## Acceptance criteria

- [ ] Compiling `content/dev/maps/stress-warren-hallway-inspection.map` no longer
  OOMs: because it is over budget it exits non-zero with a diagnostic naming the
  estimated peak bytes and the top dominating lights, **before** any multi-gigabyte
  allocation.
- [ ] **Gate admits (permit side):** a map whose estimated peak SH-delta host
  memory is within budget compiles to completion, producing the same section it
  produces today.
- [ ] **Gate refuses (refuse side):** a map whose estimate exceeds budget is
  refused before the bake's parallel sub-block loop runs; the dense payload is
  never allocated, and the process exits non-zero with the per-light diagnostic.
- [ ] The estimate accounts for the copy-chain, not a single dense buffer: it is
  the projected dense payload times a documented factor covering the copies the
  post-bake pipeline holds simultaneously (drop rebuild, compaction buffer, and the
  `--sh-analyze` doubling when that mode is on), so an admitted map does not OOM one
  copy past the gate.
- [ ] The shipped default budget admits every current dev map (`campaign-test`,
  `gate-heavily-lit`, `kinematic-platform`) and rejects
  `stress-warren-hallway-inspection`; the budget is CLI-configurable, mirroring the
  existing payload-cap flag.
- [ ] The refusal diagnostic reports the per-light CSR-entry contribution (the
  measurement forward-predicted adaptive density consumes), computed from the CSR
  without baking.
- [ ] For a fixture that emits an SH-delta section and passes the gate, the
  compiled `.prl` is byte-identical before and after this change (cold and warm
  cache). SHA-256 recorded in `research.md`.
- [ ] The serialize stage holds at most one section's serialized payload beyond the
  owned section structs — the whole-`.prl` in-memory accumulator and the per-section
  byte-image clone are gone; readback validation reads the written file. Verified
  structurally (no whole-`.prl` `Vec<u8>` accumulator, no payload clone on the
  append path) and by the byte-identity criterion above.
- [ ] A normal (non-verbose) build gains no new per-item log spam: the gate emits
  one info summary line (estimated peak, budget, decision) with the per-light
  histogram behind `-v`; the serialize footprint is one info line.

## Tasks

### Task 1: Pre-bake working-set gate + measurement (thin slice)

Add a shared guard that refuses a delta bake before it materializes its dense
payload. The projected dense bytes are `entry_count × subblock_f16_len × 2`, where
`entry_count = affinity_lights.len()` and `subblock_f16_len = FORMAT_PROBES_PER_CELL
× delta_probe_f16_stride(TILE_DIMENSION)` — both known immediately after
`build_csr`, before `bake_or_load_delta_subblocks`. Estimate **peak** as those
dense bytes times a documented copy-chain factor (the post-bake pipeline holds the
dense buffer plus a drop rebuild plus the compaction buffer, and doubles the dense
residency under `--sh-analyze`; see `research.md`), so the gate bounds the real
peak, not one buffer (AC "estimate accounts for the copy-chain"). Add a function,
shared across the three delta bakes, taking the entry count, sub-block f16 length,
the `affinity_lights` CSR array, and a byte budget; when the estimate exceeds the
budget it returns an error carrying a diagnostic: estimated peak, `entry_count`,
and the top-N light contributors by CSR-entry count (tally `affinity_lights`
occurrences — computable pre-bake from the CSR alone; the direct bake may enrich
rows with `static_index` from its `selected` list where cheap, matching today's
post-bake `DirectDeltaBakeStats` labels). This per-light tally is the measurement
AC "refusal diagnostic reports the per-light contribution" requires and the input
forward-predicted adaptive density consumes. Structure the over-budget branch as a
clean decision point — refuse now — so a future "coarsen instead of refuse" (adaptive
density) attaches there without reshaping the call sites. Call the guard in each of
the three delta bakes right after `affinity_lights` is built and before the
sub-block loop; on refusal the compile fails non-zero (mirror
`enforce_payload_cap`'s `anyhow::ensure` style) after one info summary line, with
the histogram gated behind `args.verbose` at the `pipeline.rs` call site (matching
how `lightmap_bake` diagnostics are gated). Plumb the budget as a new field carried
with `DeltaSectionConfig` (or a sibling compiler-config struct) and a CLI flag,
mirroring the `max_payload_bytes` plumbing at its `main.rs` parse site and its
`pipeline.rs` threading; the default is pinned by the admit-dev-maps /
reject-warren AC. This is the thin slice: CLI → bake → refusal diagnostic, crossing
every seam, and it alone prevents the observed OOM. Place the guard at the call
sites, not inside the shared helper, so the helper's signature and `warm-cache`'s
in-progress work are undisturbed.

### Task 2: Serialize-streaming the section write

Restructure the pack write path so it does not hold multiple whole-payload copies
of a section at serialization. Today it builds each section's byte image, clones it
into a `SectionBlob`, concatenates every section into one in-memory `file_buf`
holding the entire `.prl`, writes that once, then re-reads it through a `Cursor` for
readback validation. Replace this with a streamed write: emit sections directly to
the output file writer via the existing `W: Write` path (each section-table entry
already carries its payload length, and a delta section's payload length is
derivable from its preceding CSR/mask tables, so lengths are known up front),
freeing each large section's serialized bytes after it is written; validate
readback by re-reading the written file rather than an in-memory copy; and drop the
per-section byte-image `.clone()` on the append path (capture the byte length for
the size log without cloning the payload). The on-disk bytes must be byte-for-byte
identical — lifetime-only. Watch file size: `pack.rs` is ~3097 lines (mostly tests)
and `delta_sections.rs` ~2125; if the streamed writer needs a `write_to<W: Write>`
on the section type, add it in the level-format section module rather than growing
`pack.rs`, and split `delta_sections.rs` along its existing view/test seams before
extending it if the change lands there. Satisfies the serialize-footprint criterion
and preserves byte-identity. Independent of Task 1 (distinct files: pack /
level-format vs. the bake call sites and config).

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; crosses CLI → bake → diagnostic,
prevents the OOM, and falsifies the "gate before materialize" boundary before any
fan-out.
**Phase 2 (sequential):** Task 2 — serialize-streaming. Independent of Task 1 in
files; sequenced after only to keep the byte-identity fixture (established by Task
1's admitted-map path) as the shared regression check.

**Cross-plan dependency:** land after `warm-cache--delta-sh-and-graph-bakes` (Task
1's gate sits at the helper's pre-loop call sites; sequencing after avoids churn on
files warm-cache is still editing).

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Emitted `.prl` bytes byte-identical for any map the gate admits (lifetime + pre-bake-refusal only; no payload value/order/format change) | Task 1, Task 2 | Streaming that reorders/truncates section bytes; a gate that alters the bake for an admitted map | AC "byte-identical", AC "serialize holds at most one section" |
| The gate fires before dense materialization; no dense payload is allocated on refusal | Task 1 | Computing the estimate after the sub-block loop, or inside the helper's `collect` | AC "gate refuses", AC "no OOM" |
| The gate estimate upper-bounds real peak (dense × copy-chain factor), so an admitted map does not OOM downstream | Task 1 | A single-buffer estimate that ignores the drop/compaction/`--sh-analyze` copies | AC "estimate accounts for the copy-chain" |

## Open questions

- **Default working-set budget value.** Pinned by constraint (admit current dev
  maps, reject the warren) and the copy-chain factor; the concrete default byte
  count and the factor are set at implementation from the measured projections, not
  fixed here.
- **Copy-chain factor precision.** The factor is a documented estimate of
  simultaneous whole-payload copies. If it proves too coarse (rejecting maps that
  would fit, or admitting maps that OOM), tightening it is a follow-up; the gate's
  budget knob is the interim lever.
