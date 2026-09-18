# SH Probe Streaming

> **Epic spec.** Realizes the `large-map-spatial-residency` seed, SH-first. It sets
> direction, staging, cross-boundary constraints, and per-slice acceptance for the
> whole epic. Each slice below is drafted into its own brief/spec before it runs;
> the SH-residency slice's exact wire layering waits for `adaptive-probe-spacing`
> (id 34 v11 / id 35 v4) to land. Read at `c269dd9`.
> **Supersedes for the SH track:** `context/plans/large-map-spatial-residency.md`
> and `context/research/spatial-streaming.md` (research basis; kept).

## Goal

Keep only the SH irradiance data near — or about to be near — the camera resident,
loading and evicting the rest as the player moves, so a large production-quality map
runs with frame-time headroom on a 6 GB GTX 1660 and map scale is bounded by disk
rather than a whole-level VRAM upload. One authored level stays one logical PRL.
This is the first streamed resource on a residency substrate later baked subsystems
subscribe to.

## Scope

### In scope

- A frame-time + resident-VRAM measurement harness that isolates whether shrinking
  resident SH footprint moves frame time on the target hardware. Falsifies the
  epic's driving premise before the streaming machinery is built.
- A compiler-only cell-clustering pass and a deterministic PRL cluster directory:
  stable cluster ids, cluster bounds/cell membership, per-cluster addressing for the
  SH sections. Proven without any runtime eviction.
- SH-probe per-cluster residency: async off-frame-path load + decode, renderer-owned
  generation-matched atomic install and eviction under a budget, driven by the
  existing per-frame visible-cell signal plus a wider prefetch/hysteresis set.
  Composes over the `adaptive-probe-spacing` v11 stored geometry; preserves the
  bandwidth-neutral sampler (no new binding, no per-fragment locate-read).
- Cross-cluster SH ownership/halo rules that preserve the no-double-count lighting
  invariant and keep L0/L1/L2 reconstruction self-consistent at cluster boundaries.
- A defined miss/seam policy: a portal that would expose an un-resident cluster never
  shows a lighting hole.
- Additive authored streaming hints (streaming seams, priority/budget, always-resident)
  as brush entities modeled on the existing `*_region` / `*_volume` pattern, layered
  over algorithm-default clustering.

### Out of scope

- Streaming geometry/BVH, lightmap layers, SDF, reflection probes, fog, acoustics.
  The substrate is built to generalize (named as a one-way-door consideration), but no
  second resource streams in this epic. Geometry/BVH already gets working-set relief
  from portal + visible-cell culling; SH does not, which is why SH leads.
- Texture/material residency — keys on material + mip, not region; different eviction
  policy. Never folded into the spatial substrate.
- Multiple logical maps or gameplay-visible sectors. Residency is local resource state.
- Divergent gameplay/collision/visibility content across co-op peers. Peers may hold
  different resident subsets, but logical level/content identity agrees at admission.
- Replacing or altering `adaptive-probe-spacing`. It is the complementary within-chunk
  lever ("coarsening reduces data inside a resident chunk; residency selects which
  chunks are resident") and stays intact.
- Changing the SH bake's global view. The dense bake stays whole-volume; clustering and
  residency are post-bake partitioning and runtime selection.

## Direction

**Problem.** The runtime holds the entire baked SH irradiance volume resident and
uploaded for the whole level (whole-file `std::fs::read`, per-section `from_bytes`,
one-shot `install_level_geometry`), so a large map's SH atlas competes for VRAM and
texture-cache bandwidth regardless of where the camera is. The SH sampler's per-fragment
cost is a fixed 8-corner stencil independent of probe count, so the cost that a large
resident atlas imposes is footprint and memory-bandwidth/cache pressure — not compute.
Coarsening the whole grid (shipped global spacing; `adaptive-probe-spacing`) trades that
footprint for lost detail everywhere; streaming trades it for locality, keeping full
density near the camera.

**Prior commitments.**
- Epic 9 (roadmap-q1-archive) explicitly deferred probe streaming and shipped the
  gating instruments: the "memory-budget checkpoint + coarse open-area spacing" and a
  probe format "kept chunk-friendly so a later brick split needs no interpolant rewrite
  (deferred-streaming insurance)." This epic consumes that insurance.
- `large-map-spatial-residency` seed + `spatial-streaming.md` research fix the
  substrate: **clustered cells riding the existing visible-cell signal**, one substrate
  all baked subsystems later subscribe to — **not** a regional BVH (that framing was a
  culling structure; `perf-per-region-bvh` archived). This epic adopts that ruling.
- `adaptive-probe-spacing` (ready, branch pushed) owns the stored-density contract over
  the octahedral atlas and the **bandwidth-neutral sampler** invariant (no new binding,
  no per-fragment locate-read, ≤ 8 taps). SH streaming layers over its id 34 v11 / id 35
  v4 wire and must not turn the sampler into an indirection/locate-read path. Divergence
  from that constraint would silently regress the very frame time this epic targets.
- Author region brush entities already exist (`lightmap_scale_region`,
  `sh_protect_volume`, `fog_volume`). The authoring-hints slice extends that established
  pattern rather than inventing a partition surface. (Corrects the research's
  "net-new / greenfield" premise, now stale.)
- Renderer owns all GPU (index §2). I/O, decode, and CPU prep stay off the
  Input → Game → Audio → Render → Present path (development_guide §4.2).

**Placement.** Three axes, each pinned deliberately. *Load-time-vs-runtime:* clustering
and the cluster directory are compile-time (baked-over-computed); residency selection is
runtime. *Compiler-vs-renderer ownership:* the compiler emits the directory; the renderer
owns GPU upload, atomic install, and eviction; a between layer owns off-frame I/O/decode.
*Algorithm-default-vs-authored:* clustering is algorithmic by default with additive
authored hints — the seed's ruling, reused because a mandatory hand-partition is fragile
and zero-authoring must still yield a sane baseline.

**Alternatives rejected.**
- *Geometry/BVH first.* The seed's other candidate and the bigger raw-VRAM win, but
  geometry already gets runtime relief from portal + visible-cell culling, and its
  residency semantics are simpler — so it is neither the acute pain nor the case that
  proves the hard seams. SH is the resident-whole, least-mitigated pain and the owner's
  motivating case; proving the loader/install/evict machinery on SH's harder seams
  (cross-cluster light ownership, boundary self-consistency, bandwidth-neutral sampler)
  de-risks generalization more than proving it on the easy case would.
- *Coarsen harder instead of streaming.* Global spacing and `adaptive-probe-spacing`
  shrink footprint but cap detail everywhere and cannot lift the map-scale ceiling; they
  are kept as complementary levers, not substitutes.
- *Compress the SH representation harder* (fewer bands, a different codec, heavier
  quantization) — a footprint lever on the precision/encoding axis, distinct from
  coarsening's density axis and cheaper than load/evict machinery. Rejected as the epic's
  answer for the same reason as coarsening: it is a global constant-factor win, so resident
  footprint still scales with total probe count and hits the same map-scale ceiling.
  Streaming is the only lever that makes resident footprint scale-independent. Complementary,
  not a substitute; a codec change would compound with streaming, not replace it.
- *A separate regional-BVH / per-subsystem residency query.* Rejected by the research
  invariant: it would not unify subsystems and would duplicate the visibility signal the
  renderer already computes.
- *Sidecar files per cluster.* Rejected for the directory (see Wire format): a single
  logical PRL with a per-cluster range table keeps one authored level = one file and one
  content identity for co-op admission.

## Acceptance criteria

Rows are grouped by slice. Manual/perf rows are resource-bound: pin fixture, spacing,
machine class, cache mode, and cleanup per `testing_guide.md` §Resource bounds; GPU perf
is a manual read (no CI perf gate), never inferred from CPU time.

### Slice 1 — Measurement harness (premise falsifier)
- [ ] A per-section footprint report lists every emitted PRL section's disk bytes and
      sums to the file's total payload, on a gate fixture (extends today's aggregate-only
      pack log).
- [ ] A resident-VRAM report attributes bytes to at least the SH sections (id 34 base,
      id 35 direct, ids 27/41/45 deltas) at level install, on a booted level.
- [ ] The offscreen capture path reports steady-state frame time (and per-pass GPU time
      where `TIMESTAMP_QUERY` exists) for a named map on a named adapter, not just a PNG.
- [ ] **Premise finding (measure-and-record):** on the GTX 1660, frame time for a
      production-quality large map at two resident-SH footprints (a fine bake vs a
      coarse-spacing bake of the same map) is recorded, establishing whether — and by how
      much — resident-SH footprint moves frame time. If the delta is negligible, the epic
      stops here and the finding says so.

### Slice 2 — Cell clustering + PRL cluster directory (compiler-only)
- [ ] Clustering groups adjacent cells into units within an authored/parametrized
      byte/primitive budget; every runtime `cell_id` maps to exactly one cluster; cluster
      ids are stable across two `--no-cache` bakes of the same map (byte-identical
      directory).
- [ ] The cluster directory addresses each SH section per cluster (offset+length or
      per-cluster payload) and round-trips through the loader; a directory naming a
      cluster/section range outside the section rejects with a named error.
- [ ] With the directory present but residency disabled, a booted level is visually
      identical to the pre-directory bake (the directory is inert until Slice 3), and the
      global SH bake view is unchanged (bake determinism gate still green).
- [ ] Clustering runs at a defined compiler stage without breaking the warm/cold cache
      contract; cold `--no-cache` remains the ship source of truth.

### Slice 3 — SH-probe per-cluster residency (the load/evict slice)
- [ ] Only SH clusters in the resident set are uploaded; crossing into a new area loads
      the entering cluster's SH and evicts a departed cluster's under a budget, verified
      by the resident-VRAM report falling below the whole-level baseline on a large map.
- [ ] Residency is driven by the visible-cell set plus a wider prefetch/hysteresis set;
      oscillating across a doorway (a cluster boundary alternating on adjacent ticks) does
      not thrash load/evict (Orderings row 1).
- [ ] A render frame samples only a generation-matched SH set: a partial or
      generation-mismatched install is never sampled (Invariant: atomic install).
- [ ] A portal exposing a not-yet-resident cluster shows the defined fallback, never a
      lighting hole or uninitialized atlas (Orderings row 4; miss policy).
- [ ] A light whose influence spans clusters is not summed twice on any receiver, and
      L0/L1/L2 reconstruction is self-consistent across a cluster boundary (Invariant:
      no-double-count; boundary continuity).
- [ ] The SH sampler gains no binding and no per-fragment locate-read; forward fragment
      texture inventory and compose BGL budgets are unchanged (regression guard on the
      pipeline budget tests). Composes over id 34 v11 / id 35 v4.
- [ ] I/O and decode run off the Input → Game → Audio → Render → Present path; the event
      loop never blocks on a cluster load (a slow/synthetic-latency load degrades to the
      miss fallback, not a frame stall).
- [ ] **Perf finding (manual, resource-bound):** GTX 1660, production-quality large map,
      whole-load baseline vs SH-streamed — frame time and resident VRAM by section
      recorded; MacBook cross-check recorded; seam/pop hunt at boundaries recorded as a
      read. Full stress-warren attempted; recorded not-yet-evaluable if the box cannot
      bake/run it.

### Slice 4 — Authored streaming hints
- [ ] A streaming-seam brush entity marks a portal/door as a load-hide point; an
      always-resident brush entity pins a cluster resident; a priority/budget hint biases
      retention. Zero hints still yields the algorithm-default baseline unchanged.
- [ ] Hints are additive over clustering: a bake with no hints and a bake whose hints
      match the algorithm's defaults produce the same residency behavior.
- [ ] Hints follow the established `*_region` / `*_volume` FGD pattern (Boundary
      inventory) and validate at compile time with named errors for out-of-range values.

## Tasks

Slices are milestone-sized; each is drafted into its own brief/spec before it runs. The
paragraphs below are direction contracts for that drafting, not task-agent contracts.

### Slice 1: Measurement harness
Build the instruments that let every later slice prove its claim and that falsify the
epic's premise now. Add a whole-level per-section disk-footprint dump (today only
aggregate + largest-payload is logged at pack time). Add resident-VRAM-by-section
attribution at level install (today only scattered per-atlas startup estimates exist).
Wire steady-state frame-time and, where `TIMESTAMP_QUERY` is present, per-pass GPU-time
capture into the offscreen capture path (today it produces a PNG). Reuse `--sh-analyze`,
`POSTRETRO_GPU_TIMING`, and `Renderer::new_offscreen` / `capture_frame_indirect`. Then
run the premise finding: same map baked fine vs coarse-spacing, frame time on the 1660,
recorded. This slice ships value independent of the rest (footprint/VRAM observability)
and is the go/no-go read for the epic.

### Slice 2: Cell clustering + PRL cluster directory
A compiler-only pass that groups adjacent cells into balanced residency units under a
byte/primitive budget and emits a new deterministic cluster-directory PRL section (see
Wire format). Nothing streams: the directory is inert, the runtime still loads whole. The
pass keeps the flat global bake view SH baking needs and does not perturb existing baked
output (bake determinism gate green). Own the clustering seed/partition rule, the
cluster-id stability contract, and the per-cluster → SH-section addressing. The partition
rule must be **resource-agnostic** (keyed on cell adjacency + a byte/primitive budget), or
explicitly justified as generalizable — not tuned to SH's byte distribution alone. This is
the shared residency substrate every later resource keys on (Generalization door); a
partition that quietly becomes SH-optimal-only weakens the generalization that justifies
leading with SH. It must be right before eviction rides it.

### Slice 3: SH-probe per-cluster residency
The first real load/evict, drafted after id 34 v11 / id 35 v4 land so it composes over
real bytes. A residency planner turns the per-frame visible-cell set plus a
prefetch/hysteresis horizon into a target resident cluster set. An off-frame-path worker
loads and decodes entering clusters' SH; the renderer performs a generation-matched
atomic install and evicts departed clusters under a budget. Cross-cluster light ownership
and a halo rule keep no-double-count and boundary reconstruction self-consistent. A miss
policy defines what a portal exposing a not-yet-resident cluster shows. The sampler stays
bandwidth-neutral — no binding, no locate-read. This slice is itself drafted thin-first
(a hard-gated, synchronous, no-eviction path that proves the atomic install and the
visible-cell drive) before async load and budget eviction fan out.

### Slice 4: Authored streaming hints
Additive brush-entity hints over the default clustering: a streaming-seam marker
(load-hide point), always-resident pin, and priority/budget bias, following the existing
`*_region` / `*_volume` FGD pattern and its compile-time validation. Default (no hints)
behavior is unchanged; hints only bias the planner and clustering seeds.

## Sequencing

**Phase 1 (sequential):** Slice 1 — measurement harness. Thin path that falsifies the
epic's driving premise (does resident-SH footprint move frame time on the 1660) before
any streaming machinery is built. If the premise fails, the epic stops.

**Phase 2 (sequential):** Slice 2 — cell clustering + cluster directory. Compiler-only,
inert; the substrate Slice 3 keys on. Depends on nothing runtime; can start once Slice 1
confirms the premise. Blocks Slice 3.

**Phase 3 (sequential):** Slice 3 — SH-probe residency. Consumes the cluster directory
(Slice 2) and the harness (Slice 1); drafted after id 34 v11 / id 35 v4 land. Internally
thin-slice-first: hard-gated synchronous no-eviction install proving the atomic,
generation-matched, visible-cell-driven path, then async load + budget eviction.

**Phase 4 (concurrent with late Phase 3):** Slice 4 — authored hints. Depends on the
clustering seeds (Slice 2) and the planner surface (Slice 3); the seam-marker's
load-hide semantics need the miss policy from Slice 3, so it lands after that policy is
fixed.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP | CLI |
|---|---|---|---|---|
| Cluster directory | new PRL section (next free id 49) | little-endian, version-first, named reject on mismatch | n/a | measurement/force flags beside existing `--sh-*` |
| Cluster id | stable per-cluster index, x-fastest cluster order | `u32` in directory; every `cell_id` maps to one | n/a | n/a |
| Streaming seam | seam marker at a portal/door | n/a (compiler-derived residency behavior) | brush entity, e.g. `stream_seam` (pattern of `fog_volume`) | n/a |
| Always-resident | cluster pin | n/a | brush entity, e.g. `stream_always_resident` | n/a |
| Priority / budget | retention bias | n/a | KVP on the seam/region entity (numeric, validated range) | n/a |
| Residency budget floor | runtime cap | n/a | n/a | player-option or build const (see Open questions) |

New FGD KVP casing/enums are pinned in the Slice 4 brief against `postretro.fgd`'s
existing `*_region` / `*_volume` entries; this row set fixes the vocabulary, not the
exact strings.

## Wire format

Adds one binary surface (the cluster directory) and layers residency over two existing
sections. Encodings are implementer-pinned at landing; this fixes constraints.

- **Cluster directory (new section, next free id 49).** Little-endian; section-internal
  version word first; named reject on version mismatch (loader and compiler share the
  validator). Contains: cluster count; per-cluster bounds and cell membership; per-cluster
  addressing into the SH sections (offset+length ranges within the single PRL, or
  per-cluster payload handles). Empty-list and single-cluster (whole-map = one cluster)
  cases encode explicitly. Mirrors the existing section-table discipline (per-section
  offset+size already exists in the container). One file, not sidecars — one authored
  level stays one logical PRL and one co-op content identity.
- **id 34 (v11) / id 35 (v4).** Owned by `adaptive-probe-spacing`; **not re-versioned by
  this epic**. Residency addresses ranges *within* these sections through the directory;
  it does not change the probe-record stride, node-scale semantics, prefix-sum slot
  derivation, or the stored set. A cluster's SH payload is a contiguous, independently
  installable slice of the same v11/v4 bytes. If v11/v4 cannot be sliced per cluster
  without reordering, that reordering is a compiler-side emit-order decision recorded in
  the Slice 2/3 brief — it must not alter what the sampler reads per probe.
- **Cross-section constraint at load.** The directory's per-cluster SH ranges must be
  consistent with the delta sections' (ids 27/41/45) grid-matched entries: a resident
  cluster carries the delta entries its bricks need, or the miss/halo policy supplies
  them. Load rejects a directory that references a section version it was not built for.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A frame samples only a generation-matched SH set | Slice 3 (atomic install) | Threatened by partial upload, an evict racing an install, a mid-frame residency swap | Slice 3 AC (atomic install; no partial sample) |
| No receiver sums one physical light twice | bake (existing); Slice 3 cross-cluster ownership/halo | Threatened where a light's influence/delta spans clusters and both resident | Slice 3 AC (cross-cluster no-double-count) |
| SH reconstruction is continuous across a cluster boundary | Slice 3 halo rule | Threatened where adjacent clusters differ in resident state or L0/L1/L2 level | Slice 3 AC (boundary self-consistency); seam hunt |
| Sampler stays bandwidth-neutral (no binding, no per-fragment locate-read, ≤ 8 taps) | `adaptive-probe-spacing`; preserved by Slice 3 | Threatened by any residency indirection added to the per-fragment path | Slice 3 AC (no binding / inventory + budget guard) |
| One residency substrate: clustered cells on the visible-cell signal | Slice 2 (directory), Slice 3 (planner) | Threatened by a bespoke per-subsystem spatial query | Slice 2/3 ACs; design review |
| I/O, decode, CPU prep stay off the frame path | Slice 3 (off-frame worker) | Threatened by a synchronous load on the event loop | Slice 3 AC (no frame stall under load latency) |
| One authored level = one logical PRL / one co-op content identity | Slice 2 (single-file directory) | Threatened by sidecars or per-cluster files | Wire format; co-op admission unchanged |
| A portal never exposes a lighting hole | Slice 3 miss policy | Threatened by a visible portal into a not-yet-resident cluster | Slice 3 AC (miss fallback) |

## Orderings

Residency introduces mutable per-frame state, prefetch timers, and load completions.
Pinned scenarios the Slice 3 brief's tests cite:

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| 1 | Doorway oscillation | Camera crosses a cluster boundary back and forth on adjacent ticks | Hysteresis holds both clusters resident within budget; no load/evict thrash |
| 2 | Prefetch vs latency | Cluster enters the prefetch horizon, then becomes visible before its load completes | Miss fallback until the generation-matched install lands; no stall, no hole |
| 3 | Eviction under pressure | Resident set exceeds budget while a new cluster must load | LRU/priority evicts a non-visible cluster; a visible or always-resident cluster is never evicted |
| 4 | Portal into un-resident cluster | A portal becomes visible exposing a cluster not yet resident | Defined miss fallback (seam gate at authored seams; conservative placeholder elsewhere), never uninitialized atlas |
| 5 | Level reload / teardown | Level unload or reload while loads are in flight | In-flight loads cancel or land harmlessly; residency state clears; no install into the next level |
| 6 | Co-op divergent residency | Two peers hold different resident cluster subsets | Both render correct local lighting; logical content identity agrees at admission; residency never crosses the wire as gameplay state |

## Open questions

- **Premise magnitude (Slice 1, measurement-owned).** Whether the 1660 sluggishness is
  VRAM oversubscription (driver thrash) vs bandwidth-bound within 6 GB, and the size of
  the frame-time win from shrinking resident SH. Slice 1's finding decides whether Phase 2
  proceeds. Recommended: proceed only on a material, recorded frame-time delta.
- **Cluster byte/primitive budget (Slice 2).** The budget *number* is delegated to the
  Slice 1 distribution measurement (per-cluster footprint after a dry-run clustering pass).
  The partition *rule* is not delegated — it is fixed resource-agnostic (adjacency + budget)
  so the substrate generalizes; only the threshold is measured.
- **Residency budget floor.** Recommend sizing to the 6 GB GTX 1660 as the desktop floor
  (matches the existing lighting perf-floor hardware); confirm with the owner. Whether the
  cap is a player option or a build constant is a Slice 3 decision.
- **Miss policy default.** Recommend a designed seam-gate at authored streaming seams
  (fits the theatrical set-piece ethos) plus a conservative placeholder (ambient-floor SH)
  elsewhere. Owner-confirmable; it is player-visible pacing, not purely technical.
- **v11/v4 per-cluster sliceability.** Whether id 34 v11 / id 35 v4 payloads slice per
  cluster without reordering, or need a compiler-side emit-order change (sampler-neutral).
  Resolved against the landed branch when Slice 3 is drafted.
- **Generalization door.** The substrate is built to let geometry/lightmap-layer/SDF/
  fog/acoustics subscribe later. Not built here; flagged so Slice 2's directory shape does
  not foreclose a second resource cheaply.
