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

## The copy chain — the payload is materialized whole, several times over

Peak host residency is a chain of whole-payload contiguous buffers, each
`delta_subblocks.len() × 2` bytes, several alive at once:

| Stage | Site | Whole-payload buffer |
|---|---|---|
| Dense assemble | `delta_sh_cache.rs` `bake_or_load_delta_subblocks` (`.flatten().collect()`) | dense `Vec<u16>`, resident from here through coarsening |
| Exact-zero drop | `delta_drop_policy.rs::rebuild_csr_indexed` (`retained_payload = Vec::with_capacity(payload.len())`) | a second dense copy |
| Coarsening / envelope | `sh_runtime_envelope.rs` fix-point; runs a full `compact_direct_valid_probes` internally | transient compaction copy per iteration |
| Valid-probe compaction | `delta_sections.rs::compact_dense_valid_probe_payload` (`Vec::with_capacity(capacity)`) | **the buffer that OOM'd** |
| Serialize | `pack.rs` `to_bytes()` (byte image) → `.clone()` into `SectionBlob` → concat into one `file_buf` for the whole `.prl` → `Cursor` readback | ~3–4 further whole-payload copies |
| `--sh-analyze` (opt-in) | `pipeline.rs` clones the entire post-drop dense sections | doubles dense residency in that mode |

**What is actually co-resident, and across how many bakes.** Within one bake the peak
is the dense buffer plus the compaction buffer it is rewritten into (≈2×); the
exact-zero drop rebuild is an *earlier* transient, freed before compaction allocates,
not a third simultaneous copy. Across the compile, `pipeline.rs` runs three delta
bakes (in run order: indirect id27, animated-direct id45, direct id41), assembles all
three into `PostBakeDeltaSections`, and holds every dense payload from its bake through
the single
shared compaction — so the real peak scales with the **sum** of the three dense
payloads, and `--sh-analyze` clones all three at once (≈3× the sum). The gate's budget
must bound the cumulative dense across the three bakes, not one bake in isolation
(Task 1). The gate is scoped to the three delta bakes (owner-locked); the base id34/id35
whole-volume dense buffers that `pipeline.rs` clones and holds co-resident between the
delta bakes (`sh_analyze_base_indirect` / `sh_analyze_base_direct`) are *not* counted by
it, so the conservative copy-chain factor must leave headroom for them — a peak dominated
by a base clone rather than a delta payload is outside this gate's reach.

`write_prl` (`level-format/src/lib.rs`) is generic over `W: Write`, but it writes the
**whole section table — every section's offset and length — before any payload byte**,
and today the compiler pre-serializes every section into eager `*_bytes` locals and
feeds an in-memory `file_buf` accumulator rather than the output file. So a bare writer
redirect removes the whole-`.prl` accumulator but still holds every section's bytes at
once: streaming to one-section residency additionally needs a payload-free length query
per section (so the table can be written without the payloads resident) and per-section
serialization moved into the write loop. See Task 2.

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
    Cap --> Serialize: to_bytes → clone → file_buf → readback
    Serialize --> [*]: fs::write
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
