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

**Prior commitments.** `warm-cache--delta-sh-and-graph-bakes` (landed) built
the shared per-entry helper the three delta bakes route through and commits to
byte-identical output; this work builds on that helper.
`lighting-scale--lightmap-bake-incremental-flush` (draft) shares the peak-RAM /
allocation-lifecycle pattern and byte-identity contract with this spec; the two are
siblings under the epic, neither prior to the other. Its "separate spec" out-of-scope note defers the
*runtime* GPU storage-buffer/binding footprint — which this compile-time host-RAM
spec also excludes (see Out of scope) — not this bake's host memory; this spec's
mandate is the observed compile-time OOM in `research.md`. The
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
- *Out-of-core / spill the dense buffer.* Back the dense payload with an
  mmap/on-disk allocation so the whole-section coarsening fix-point runs over
  spilled pages and the bake *completes* — the completing posture the sibling
  lightmap track takes. Rejected as the interim: coarsening's neighbor-coupled
  smoothing is random-access across the whole section, so a spilled buffer thrashes
  rather than streams, and it yields no per-light measurement. Refuse-and-measure
  survives cheaply and produces the substrate forward-predicted adaptive density
  consumes; spill stays available to that later work if it ever wants completion
  without the density model.
- *Gate inside each bake (per-bake cumulative check).* Thread a running total through
  the three bake bodies and refuse in-bake. Rejected: it makes the three bakes fallible
  (rippling through their wrappers and tests), leaves earlier admitted bakes' dense
  resident when a later bake trips the budget, and gives no single point that sees all
  three projections. The plan-phase gate is less code and is the global decision point a
  future density pass needs.

## Acceptance criteria

- [ ] Compiling `content/dev/maps/stress-warren-hallway-inspection.map` no longer
  OOMs: because it is over budget it exits non-zero with a diagnostic naming the
  estimated peak bytes and the top dominating lights, **before** any multi-gigabyte
  allocation.
- [ ] **Gate admits (permit side):** a map whose estimated peak SH-delta host
  memory is within budget compiles to completion, producing the same section it
  produces today.
- [ ] **Gate refuses (refuse side):** a map whose cumulative estimate exceeds budget
  is refused in the plan phase, before any delta dense bake runs; no delta dense
  payload is allocated, and the process exits non-zero with the per-light diagnostic.
- [ ] The estimate accounts for the copy-chain **and for all three delta bakes**, not
  a single dense buffer: it is the cumulative dense payload summed across the three
  delta bakes — which `pipeline.rs` assembles and holds co-resident from their bakes
  through the shared compaction — times a documented factor that upper-bounds peak
  co-residency: the dense buffers plus the compaction buffer they are rewritten into
  (≈2×), plus one cumulative-delta-sized share reserved for the co-resident base id34/id35
  originals and clones (≈3× normally), plus the persistent `--sh-analyze` clone of all
  three when that mode runs with coarsening enabled (≈4×). The exact-zero drop rebuild is
  a separate earlier equal-size transient the same factor bounds. So an admitted map does
  not OOM one copy — or one sibling bake — past the gate on delta payload. A base-dominated
  map remains outside this delta gate's reach; the reserved share prevents a
  delta-dominated admission from consuming the whole host-RAM budget (see `research.md`).
- [ ] The shipped default budget admits every current dev map (`campaign-test`,
  `gate-heavily-lit`, `kinematic-platform`) and rejects
  `stress-warren-hallway-inspection`; `research.md` records the four maps' measured
  cumulative-dense projections and the numeric gap between the heaviest admitted dev
  map and the warren that the default sits within, so the default is a measured
  separating value, not an arbitrary pick. The budget is CLI-configurable, mirroring
  the existing payload-cap flag.
- [ ] The refusal diagnostic reports the per-light CSR-entry contribution (the
  measurement forward-predicted adaptive density consumes), computed from the CSR
  without baking; on a cumulative refusal it reports each contributing bake's dense bytes
  and per-light rows labeled by bake, so the dominant bake and its lights are named even
  when that bake is not the one that tripped the budget.
- [ ] For a fixture that emits an SH-delta section and passes the gate (an admitted map
  with an animated light for id27/id45, or a promoted static light for id41), the
  compiled `.prl` is byte-identical before and after this change (cold and warm
  cache). SHA-256 recorded in `research.md`.
- [ ] The serialize stage holds at most one *large* section's serialized payload beyond
  the owned section structs (small sections, whose combined bytes are far below one large
  payload, may be serialized eagerly to populate the section table) — the whole-`.prl`
  in-memory accumulator, the eager large-payload `*_bytes` locals, and the per-section
  byte-image clone are gone; readback re-reads the written file and validates each
  section's container framing. Verified structurally (no whole-`.prl` `Vec<u8>`
  accumulator, no eager *large*-section byte image materialized before its own streamed
  write, no `to_bytes` call made only to measure a large section's length, no payload
  clone on the append path) and by the byte-identity criterion above.
- [ ] A normal (non-verbose) build gains no new per-item log spam: the gate emits
  one info summary line (estimated peak, budget, decision) with the per-light
  histogram behind `-v`; the serialize footprint is one info line.

## Tasks

### Task 1: Pre-bake working-set gate + measurement (thin slice)

Gate the three delta bakes' combined host RAM in a **plan phase** that runs before any
delta dense payload is materialized, then let the bakes run in an **execute phase**. The
plan phase builds each delta bake's CSR up front in `pipeline.rs` — `decompose_affinity*`
+ `build_csr`, producing `affinity_lights` and its offsets/masks (cheap light-index
arrays, not the dense payload) — hoisting the `EntityShadowLights` selection the direct
(id41) CSR needs ahead of the dense bakes (that selection is a BVH-eligibility CPU pass
reading no baked SH payload, so it moves up cleanly — but reconstruct its run condition
as `!static_baked_lights.is_empty()`, equivalent to today's
`direct_sh_volume_section.is_some()` gate, which is unavailable at the plan-phase site
because the base-direct id35 section has not baked yet). No delta bake's CSR depends on
another delta bake's output, so all three CSRs are constructible before any dense bake.
From each CSR the projected dense bytes are `entry_count × subblock_f16_len × 2`, where
`entry_count = affinity_lights.len()` and `subblock_f16_len = PROBES_PER_CELL ×
delta_probe_f16_stride(TILE_DIMENSION)` — uniform across the three bakes (the per-bake
stride constants are aliases, 18,432 bytes/entry), so only `entry_count` varies. Estimate **peak** as the cumulative dense bytes summed across the three delta bakes
(which `pipeline.rs` assembles and holds co-resident from their bakes through the
shared compaction) times a documented copy-chain factor: the dense buffers plus the
compaction buffer they are rewritten into (≈2×), one cumulative-delta-sized share of
headroom for the co-resident base id34/id35 originals and clones (≈3× normally), plus
the persistent `--sh-analyze` clone of all three when that mode runs with coarsening
enabled (≈4×); the exact-zero drop rebuild is a separate earlier equal-size transient
the same factor bounds (see `research.md`). A single gate check in `pipeline.rs` compares the cumulative projection ×
factor against the budget — weighing the whole delta demand at once, not one bake in
isolation (a per-bake check in isolation would admit three bakes at 40% budget each and
OOM at 120% downstream). On refusal the compile returns an error before any delta dense
bake runs, so no delta dense payload is ever allocated; only the cheap plan-phase CSRs
are resident. The projection+gate is a function taking the three CSRs (their
`affinity_lights` arrays and entry counts), the sub-block f16 length, and a byte budget;
it returns an error carrying a
diagnostic that reflects the *cumulative* demand: the cumulative estimated peak, each
contributing bake's dense bytes (so the dominant bake is named), and each bake's top-N
light contributors by CSR-entry count (tally
`affinity_lights` occurrences — computable pre-bake from the CSR alone; label the rows
by bake, since the light-index spaces differ — id27/id45 index `affinity_lights`, id41
indexes its `selected` list; the direct bake may enrich its rows with `static_index`
from `selected` where cheap, matching today's post-bake `DirectDeltaBakeStats` labels). This per-light tally is the measurement
AC "refusal diagnostic reports the per-light contribution" requires and the input
forward-predicted adaptive density consumes. The single upfront decision over all three projections is exactly the seam where a future
"coarsen instead of refuse" (adaptive density) attaches — it replaces the refuse branch
with a global density-budget decision, no call-site reshape needed then. Because the gate
lives in `pipeline.rs` before the dense bakes, not inside a bake body, the gate itself
forces no change to the three bakes' signatures or `warm-cache--delta-sh-and-graph-bakes`'s landed helper:
refusal is a clean early return from the already-fallible compile, not an `ensure!`
threaded through each bake's `Option` return. In the execute phase the three dense bakes
run as today; whether each rebuilds its CSR or reuses the plan-phase one is an
implementation call — thread the plan-phase CSR into the bake if the `decompose_affinity`
/ `build_csr` pass measures costly, otherwise the pure-recompute keeps the bake bodies
byte-identical. On refusal the compile fails non-zero (mirror `enforce_payload_cap`'s
`anyhow::ensure` style) after one info summary line, with the histogram gated behind
`args.verbose` at the `pipeline.rs` call site (matching how `lightmap_bake` diagnostics
are gated). Plumb the budget as a new field carried
with `DeltaSectionConfig` (or a sibling compiler-config struct) and a CLI flag,
mirroring the `max_payload_bytes` plumbing at its `main.rs` parse site and its
`pipeline.rs` threading; the default is pinned by the admit-dev-maps /
reject-warren AC — set from those maps' measured cumulative-dense projections, and
CLI-overridable per invocation. The copy-chain factor is a deliberately conservative
upper bound (it may reject a delta-dominated map that would just fit, and never admits
one that OOMs on delta payload — but the base id34/id35 clones `pipeline.rs` holds
between the delta bakes are uncounted, see `research.md`, so one full factor is reserved
as headroom for them); if measurement shows it too coarse, the budget knob is the interim
lever and tightening the factor is a follow-up. This is the thin slice: CLI → plan-phase
gate → refusal diagnostic, crossing every seam, and it alone prevents the observed OOM.

### Task 2: Serialize-streaming the section write

Restructure the pack write path so it does not hold multiple whole-payload copies
of a section at serialization. Today it builds each section's byte image, clones it
into a `SectionBlob`, concatenates every section into one in-memory `file_buf`
holding the entire `.prl`, writes that once, then re-reads it through a `Cursor` for
readback validation. Replace this with a write that bounds resident serialized bytes to one section. Two
facts block a bare writer swap: `write_prl` takes `&[SectionBlob]` and emits the
whole section table — every section's offset and length — before any payload, and
`pack.rs` pre-serializes every section into eager `*_bytes` locals before assembling
the list, so redirecting the writer from the in-memory `file_buf` to the output file
alone still holds every section's bytes at once. Reaching one-large-section-resident
peak needs, together: the section table (`write_prl` writes every offset and length
before any payload) filled from lengths known without the large payloads resident — a
delta section's length follows from its finalized CSR/coarsening tables, and each other
section whose payload can grow with content — the split is by *size*, not a fixed type
roster: every `pack.rs` `*_bytes` local that scales with map content (SH/delta, lightmap,
BVH, geometry, the shadowmask atlas, animated-light weight maps, the SDF atlas, and any
other content-scaled section — enumerate against the `*_bytes` locals) exposes a
payload-free byte-length query beside its `to_bytes`, kept bit-exact with
`to_bytes().len()` (the section-table length field is a consumer of that query, never a
reason to call `to_bytes` merely to measure). Only the genuinely fixed-size / bounded
sections may be serialized eagerly to populate the table — their combined residency is
far below one large payload and does not move the peak. Then per-section serialization of the large
payloads moves into the write loop — serialize one large section, write it through the
`W: Write` path, drop it, next — which retires the eager large-payload `*_bytes` locals
that today coexist before the write. Guard the length contract at write time: assert that
each section's streamed byte count equals the length its table entry declared, so a
`byte_len`-vs-`to_bytes().len()` drift aborts the build deterministically at the offending
section rather than shipping a container whose interior offsets are silently wrong.
Readback then re-reads the written file and checks each section's container framing — its
table offset and length resolve to a decodable region and the declared length matches the
bytes present — not by comparing against resident payload copies (the removed clones no
longer provide them); the write-time assertion, not framing readback, is what catches a
`byte_len` drift, and whole-payload byte fidelity is the byte-identity fixture's job
(AC "byte-identical").
Also drop the per-section byte-image `.clone()` on the append path (capture the byte
length for the size log without cloning the payload), and emit the serialize footprint
as one info line. The on-disk bytes must be byte-for-byte
identical — lifetime-only. Watch file size: `pack.rs` is ~3097 lines (mostly tests)
and `delta_sections.rs` ~2125; if the streamed writer needs a `write_to<W: Write>`
on the section type, add it in the level-format section module rather than growing
`pack.rs`, and split `delta_sections.rs` along its existing view/test seams before
extending it if the change lands there. Satisfies the serialize-footprint criterion
and preserves byte-identity. Largely independent of Task 1: Task 1 adds a budget field
to `DeltaSectionConfig` (`delta_sections.rs`) and Task 2 may split that file, so Task
1's field-add (Phase 1) lands first; otherwise the two touch distinct files (pack /
level-format vs. the bake call sites and config).

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; crosses CLI → plan-phase gate →
diagnostic, prevents the OOM, and falsifies the "gate before materialize" boundary
before any fan-out.
**Phase 2 (sequential):** Task 2 — serialize-streaming. Independent of Task 1 in
files; sequenced after only to keep the byte-identity fixture (established by Task
1's admitted-map path) as the shared regression check.

**Cross-plan dependency:** `warm-cache--delta-sh-and-graph-bakes` — landed
(`done/`); its shared helper is the seam Task 1's gate sits ahead of. No remaining
ordering constraint.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Emitted `.prl` bytes byte-identical for any map the gate admits (lifetime + pre-bake-refusal only; no payload value/order/format change) | Task 1, Task 2 | Streaming that reorders/truncates section bytes; a gate that alters the bake for an admitted map | AC "byte-identical", AC "serialize holds at most one large section" |
| The gate fires in the plan phase, before any delta dense materialization; no delta dense payload is allocated on refusal | Task 1 | Computing the estimate after a dense bake, or inside the helper's `collect` | AC "gate refuses", AC "no OOM" (gate placement — plan phase, ahead of the execute-phase bakes — verified by review; warren's before-multi-GB-alloc exit is the observable proxy) |
| The gate estimate upper-bounds the delta-dominated peak (cumulative dense across the three bakes × copy-chain factor), so an admitted map does not OOM downstream on delta payload (base id34/id35 copies are uncounted; one full factor is reserved as headroom) | Task 1 | A per-bake estimate that ignores the sibling bakes' co-resident dense, an exact delta-only factor with no base headroom, or a single-buffer factor that ignores the compaction / `--sh-analyze` copies | AC "estimate accounts for the copy-chain" |

## Pinned scenarios

Concrete orderings the ACs imply but do not spell out. Each is a test; each names
the task that delivers it and the AC or invariant it reinforces.

| ID | Scenario | Ordering | Expected outcome | Kind | Task (covered?) | Reinforces |
|---|---|---|---|---|---|---|
| P1 | Three delta sections, each ≈40% of budget | Plan phase builds all three CSRs → cumulative projection ≈120% of budget × factor, checked once | Gate refuses in the plan phase before any delta dense bake; no delta dense allocated | refuse | Task 1 — covered (single upfront gate over all three projections) | Invariant "admitted map does not OOM downstream"; AC "gate admits" |
| P2 | `--sh-analyze` on a map near budget | Three deltas baked; `pipeline.rs` clones all three at once after baking, held through compaction | Budget bounds ≈4× the cumulative dense (≈2× delta residency, one share of base-copy headroom, plus the simultaneous analysis clone); refuse if exceeded | refuse | Task 1 — covered (factor applied to the cumulative sum, ≈4× under `--sh-analyze`) | AC "estimate accounts for the copy-chain" |
| P3 | All lights culled → `affinity_lights` empty (N=0) | Plan phase builds the CSR → empty `affinity_lights` → projected dense 0 | Estimate 0, admits; histogram empty; no divide-by-zero; section produced/omitted exactly as today | admit | Task 1 — pinned here (paragraph does not special-case N=0) | AC "gate admits" |
| P4 | Near-budget map, warm cache hit | Plan-phase projection is CSR-derived and computed before any dense bake; warm vs cold changes only the execute-phase bake path | Gate decision identical warm vs cold (admit→admit, refuse→refuse); byte-identical output on the admit side | invariant | Task 1 — pinned here (cache-independence otherwise unstated) | AC "byte-identical (cold and warm cache)" |
| P5 | Streamed write of a large delta section | `write_prl` writes the full section table (all offsets + lengths) before the first payload byte | Each large section's length comes from a payload-free length query (bit-exact with `to_bytes().len()`); no large section's `to_bytes` image is materialized before its own streamed write; at most one large section's byte image resident (small sections may be serialized eagerly to fill the table) | invariant | Task 2 — covered (payload-free length query for large sections; small eager) | AC "holds at most one large section's serialized payload" |
| P6 | Streamed vs accumulator write of the same sections | Table then payloads emitted in the same section order as today | Byte-for-byte identical `.prl`: section order, offsets, and lengths unchanged | invariant | Task 2 — partial ("lifetime-only"); section-order stability pinned here | AC "byte-identical"; Invariant "byte-identical" |
| P7 | Readback validation after streamed write | File fully written and flushed → reopen the file → validate each section | Readback re-reads the on-disk file (not an in-memory whole-`.prl`) and validates container framing; a framing mismatch aborts non-zero. A `byte_len` drift is caught earlier by the write-time length assertion (P9), not by framing readback | invariant | Task 2 — covered (re-read the written file; framing check); flush-before-read pinned here | AC "readback validation reads the written file" |
| P8 | A single delta bake's projection alone exceeds budget (the warren case) | Plan phase builds all three CSRs; one bake's projection × factor already exceeds budget | Gate refuses in the plan phase before any delta dense bake | refuse | Task 1 — pinned here (the dominant single bake trips the same upfront gate; distinct diagnostic attribution from P1) | AC "no OOM"; Invariant "gate fires before any delta dense materialization" |
| P9 | A large section's `byte_len` disagrees with its `to_bytes().len()` | Section table written from `byte_len`; the section's payload then streamed | Write-time assertion (streamed byte count == declared length) aborts the build non-zero at the offending section, before a container with wrong interior offsets is written | refuse | Task 2 — covered (write-time length assertion) | AC "holds at most one large section's serialized payload"; AC "byte-identical" |

## Open questions

None. Budget-value and copy-chain-factor calibration is decided in Task 1: the
default is measured to satisfy the admit-dev-maps / reject-warren AC and is
CLI-overridable, and the factor is a conservative upper bound tunable via the budget
knob.
