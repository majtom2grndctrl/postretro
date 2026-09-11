# Compile-Time Peak RAM — research notes

Ephemeral. Mechanism evidence and the memory-residency lifecycle behind the
`index.md` spec. Names functions and line numbers as of the 2026-09-09 tree —
these drift; the spec states only what survives.

## The observed crash

`xtask dist`, stage 6 bake of `stress-warren-hallway-inspection.prl`. The
`Direct SH Delta Bake` progress bar reached 100%, then:

```
memory allocation of 12163350528 bytes failed
... exited with exit code: 0xc0000409
```

`0xc0000409` is the Windows fail-fast the Rust allocator raises when `alloc`
returns null — a genuine host OOM, not a stack smash. The build was a debug
`xtask.exe`; debug vs. release does not change the allocation size.

### The number decomposes exactly

`12,163,350,528 bytes = 6,081,675,264 × u16`. Two exact readings, and they
coincide:

- `659,904 CSR entries × (PROBES_PER_CELL 64 × delta_probe_f16_stride(6) 144 × 2 B)` = `659,904 × 18,432`.
- `42,233,856 compacted tiles × 144 × 2 B`, where `42,233,856 = 659,904 × 64`.

At uniform L0 full validity, every cell keeps all 64 probes, so the compacted
size equals the dense size — which is why the two readings land on the same
byte count. The failing allocation is the exact-sized `Vec::with_capacity` in
valid-probe compaction (`delta_sections.rs::compact_dense_valid_probe_payload`,
the `capacity = compact_tile_count × probe_stride` buffer), not a doubling
`collect`.

## Why it completes, then OOMs

The `advance(1)` that drives the bar fires once per CSR entry inside
`delta_sh_cache::bake_or_load_delta_subblocks`. The bar hits 100% when the last
sub-block is computed — the SH math is done. The fatal allocation is a *later*
stage assembling the finished result. Compute was never the constraint;
materialization of the finished payload is.

## Task 1 calibration (2026-09-10)

The working-set gate was measured with the compiler's default map settings,
`--no-cache`, `--no-tui`, and a zero working-set budget. Zero deliberately
causes a refusal immediately after the plan CSRs are built, so the diagnostic
reports the exact pre-bake projection without allocating a dense delta payload.
The table gives the dense sum and its normal (non-`--sh-analyze`) 3x host-RAM
projection. Two shares cover delta dense-plus-compaction residency. The third is
reserved for the co-resident id 34/id 35 originals and clones:

| Map | Cumulative dense bytes | 3x projected peak bytes |
|---|---:|---:|
| `campaign-test` | 74,907,648 | 224,722,944 |
| `gate-heavily-lit` | 9,289,728 | 27,869,184 |
| `kinematic-platform` | 40,200,192 | 120,600,576 |

`campaign-test` is the heaviest measured dev map. The observed
`stress-warren-hallway-inspection` direct-delta CSR alone is exactly
12,163,350,528 dense bytes (the decomposition above), so its normal projected
peak is at least 36,490,051,584 bytes before adding ids 27 and 45. A fresh
attempt to reach the warren plan phase was stopped safely during its unrelated
lightmap stage, which projected roughly 35 minutes; it did not reach SH and did
not allocate a delta payload. Therefore the warren figure here is a
conservative lower bound, not a newly measured cumulative total.

The shipped `--sh-delta-working-set-max-size` default remains 16 GiB
(17,179,869,184 bytes). It is 16,955,146,240 bytes above the heaviest measured
dev-map peak and 19,310,182,400 bytes below the warren direct-delta lower-bound
peak, leaving a measured separation even before the warren's other delta bakes
are counted. `--sh-analyze` with coarsening enabled uses the documented 4x factor
instead. Analysis without coarsening retains no dense clone and stays at 3x.

For an admitted fixture, `campaign-test` was built twice with an initially
empty temporary cache directory (first run cold, second run warm), with outputs
outside the workspace. Both complete PRLs have SHA-256
`04b29d910f1ac6df04137f8c4b290cbf21a3822b85f4f367ee9ed93b0c865965`.
This exercises ids 27, 41, and 45 and confirms that the plan-phase gate leaves
the admitted cold/warm artifact bytes unchanged.

## The copy chain — the payload is materialized whole, several times over

Peak host residency is a chain of whole-payload contiguous buffers, each
`delta_subblocks.len() × 2` bytes, several alive at once:

| Stage | Site | Whole-payload buffer |
|---|---|---|
| Dense assemble | `delta_sh_cache.rs` `bake_or_load_delta_subblocks` (`.flatten().collect()`) | dense `Vec<u16>`, resident from here through coarsening |
| Exact-zero drop | `delta_drop_policy.rs::rebuild_csr_indexed` (`retained_payload = Vec::with_capacity(payload.len())`) | a second dense copy |
| Coarsening / envelope | `sh_runtime_envelope.rs` fix-point; runs a full `compact_direct_valid_probes` internally | transient compaction copy per iteration |
| Valid-probe compaction | `delta_sections.rs::compact_dense_valid_probe_payload` (`Vec::with_capacity(capacity)`) | **the buffer that OOM'd** |
| Serialize (before Task 2) | `pack.rs` `to_bytes()` (byte image) → `.clone()` into `SectionBlob` → concat into one `file_buf` for the whole `.prl` → `Cursor` readback | ~3–4 further whole-payload copies; historical measurement context |
| Serialize (current) | `pack.rs` plans descriptors from payload-free lengths, writes the table and one encoded payload at a time to a temporary file, then reopens that file for validation | one serialized payload at a time; no whole-`.prl` accumulator or in-memory readback |
| `--sh-analyze` (opt-in) | `pipeline.rs` clones the entire post-drop dense sections | doubles dense residency in that mode |

**What is actually co-resident, and across how many bakes.** Within one bake the peak
is the dense buffer plus the compaction buffer it is rewritten into (≈2×); the
exact-zero drop rebuild is an *earlier* transient, freed before compaction allocates,
not a third simultaneous copy. Across the compile, `pipeline.rs` runs three delta
bakes (in run order: indirect id27, animated-direct id45, direct id41), assembles all
three into `PostBakeDeltaSections`, and holds every dense payload from its bake through
the single
shared compaction — so the real peak scales with the **sum** of the three dense
payloads, and `--sh-analyze` clones all three at once. The gate charges 3× normally:
2× for the delta chain plus one cumulative-delta-sized base-copy reserve. Coarsened
`--sh-analyze` charges 4× by adding its simultaneous delta clone. The gate's budget
must bound the cumulative dense across the three bakes, not one bake in isolation
(Task 1). The gate is scoped to the three delta bakes (owner-locked); the base id34/id35
whole-volume dense buffers that `pipeline.rs` clones and holds co-resident between the
delta bakes (`sh_analyze_base_indirect` / `sh_analyze_base_direct`) are *not* counted by
it directly. One full factor is reserved as headroom for those copies, so a
delta-dominated admission cannot spend the entire budget on the exact delta-only peak.
A peak dominated by a base clone remains outside this delta gate's reach.

Before Task 2, `write_prl` (`level-format/src/lib.rs`) wrote the **whole section
table — every section's offset and length — before any payload byte**. The compiler
therefore pre-serialized eager `*_bytes` locals and accumulated an in-memory `file_buf`.
A bare writer redirect would have removed only the whole-`.prl` accumulator, not the
co-resident section payloads. That design required payload-free length queries and
per-section serialization in the write loop.

Task 2 now plans each section's descriptor from its byte length, writes the header and
full table to a temporary output file, then encodes and writes one payload at a time.
The writer rejects a payload whose encoded length differs from its table entry. After it
flushes and closes the file, readback reopens the temporary file and validates every
section before publication. This removes the eager large-payload locals, whole-file
accumulator, and in-memory `Cursor` readback while preserving the serialized bytes.

## The blocker: the dense payload cannot be streamed away

Valid-probe compaction needs each cell's `cell_levels`. Those are decided by the
coarsening classifier + runtime envelope (`pipeline.rs` coarsening pass), which
is a **whole-section fix-point over the dense payload**: the envelope re-scores
dense levels each iteration, runs a full compaction internally for byte
accounting, and its smoothing couples neighboring cells. So the dense
`Vec<u16>` must stay fully resident from bake through coarsening. It cannot be
dropped per-entry without restructuring coarsening itself (out of scope; bleeds
into forward-predicted adaptive density).

Consequence: the dense **intermediate** can be far larger than the **emitted**
section. Bounding the emitted section to the runtime binding limit does *not*
bound the dense working set. The warren's 12 GB dense payload might coarsen to a
shippable section, but the compiler must hold all 12 GB to run the coarsening
that would shrink it.

Every post-bake consumer (`DeltaView`, `DenseDeltaView`, drop, compaction)
reads entries by their own CSR-derived offset — per-entry, no cross-entry random
access. The only whole-section requirement is the coarsening *level decision*.
So streaming is blocked at the level decision, not at the payload access
pattern.

## Two different limits, often conflated

- **Emitted-section cap** — a *loadability* limit, **owned elsewhere, out of
  this spec's scope.** The runtime binds each delta section as one storage buffer,
  checked per-binding against `max_storage_buffer_binding_size`
  (`renderer_init_resources.rs` `REQUIRED_STORAGE_BUFFER_BINDING_SIZE = 2 GiB − 8`
  — a documented **stopgap**; the comment names `perf-animated-sh-light-culling`
  as driving the SH-delta binding under the 128 MiB WebGPU spec floor). Today's
  compiler cap (`DeltaSectionConfig.max_payload_bytes`, default 256 MiB, aggregate
  over ids 27/41/45) plus the 128 MiB per-section loader floor are an owner
  decision (`sh-adaptive-coarsening-v2`, 2026-08-28). This spec does not touch
  them: raising a bake cap toward the 2 GiB stopgap would 16× the locked floor and
  bake content that becomes unloadable when the stopgap is removed.
- **Dense working-set budget** — a *host-RAM* limit. Bounds the dense
  intermediate the compiler must hold through coarsening. No such guard exists
  today; it is what this spec adds. Orthogonal to the loadability cap above.

The instrumentation history: a `--sh-probe-spacing 2.0` stress bake emitted a
401 MB direct payload (21,764 CSR entries); the historical overflow was 1.22 GiB
(~71k entries). This crash is 659,904 entries — the warren hallway is the
coarsening *floor* case (uniform lighting, least coarsenable — per the
`sh-probe-density-coarsenability-spike` findings, 18.5% coarsenable vs. 83.8%
theatrical), so it is the adversarial worst case for every payload-reduction
lever and a good regression fixture.

## Memory-residency lifecycle

```mermaid
stateDiagram-v2
    [*] --> Bake: build_csr → publish_total(entries)
    Bake --> DenseResident: collect dense Vec<u16> (bar 100%)
    DenseResident --> Coarsen: whole-section fix-point (dense required)
    Coarsen --> Compact: cell_levels decided → Vec::with_capacity(compact) ← OOM here
    Compact --> Cap: enforce_payload_cap (reads length only — too late)
    Cap --> Serialize: planned lengths → one temp-file payload at a time → reopened readback
    Serialize --> [*]: publish validated file
    note right of DenseResident
        Dense payload pinned here through Coarsen.
        Pre-bake gate must fire BEFORE this state.
    end note
    note right of Serialize
        write_prl writes the section table (all lengths) first.
        One-section residency needs a payload-free length query
        per section + per-section serialize in the write loop.
    end note
```

## File-size flags (split-before-extend watch)

- `crates/level-compiler/src/delta_sections.rs` — **2125 lines**. A restructure
  that extends it pushes it past 2000; ~half is `#[cfg(test)]` and the
  `EmittedDeltaSectionRef` view. Candidate for a split before extension.
- `crates/level-compiler/src/pack.rs` — **3097 lines**, mostly `#[cfg(test)]`
  from ~1264. The serialize-streaming change lands in the non-test half.
- `crates/level-compiler/src/delta_drop_policy.rs` — 473 lines.
- `crates/level-format/src/direct_sh_delta_volumes.rs` — 619 lines.

## Prior commitments touched

- `warm-cache--delta-sh-and-graph-bakes` (landed, `done/`): built
  the shared per-entry helper `bake_or_load_delta_subblocks` and routes all
  three delta bakes through it. Its contract is byte-identical output and a
  reassemble-then-drop seam; the pre-bake gate sits at that helper's call sites
  (before the sub-block loop), so the helper's signature stays untouched. Landed,
  so no ordering constraint remains.
- `lighting-scale--lightmap-bake-incremental-flush` (draft): the same
  peak-RAM/allocation-lifecycle pattern for the lightmap bake; explicitly scopes
  out "The SH storage-buffer / delta footprint problem … separate spec." That note
  defers a *runtime* GPU storage-buffer footprint — which this compile-time
  host-RAM spec also excludes — not this bake's host memory. Sibling track under
  the epic, not a hand-off.
- `lighting-scale--sh-adaptive-coarsening-v2` (done) locked the cap regime
  (256 MiB aggregate bake cap + 128 MiB per-section loader floor, 2026-08-28). This
  spec leaves it untouched — the emitted cap is out of scope.
- Forward-predicted adaptive density (`sh-probe-density-coarsenability-spike`
  findings) is the only path to a *small dense working set* on a large emitted
  map. Separate effort; the gate's refusal points at it.
