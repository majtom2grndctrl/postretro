# SH Probe Streaming

> **Status (2026-09-23):** Slices 1–3 shipped in PR #516 (merged as
> `8ce91e682`); their specs are under `context/plans/done/`. Slice 4 authored
> hints have not been drafted or implemented, so this parent epic remains in
> progress. Adapter-backed frame-time, seam, and GPU growth-copy evidence is
> still `not-yet-evaluable`, not a completed performance claim.
> **Slice 4 scope clarification:** authored
> seams cut cluster boundaries and prefer far-side warm-up while a doorway
> hides the load. They do not delay door/portal/gameplay state. A cold opening
> keeps the Slice 3 ambient-floor fallback. This is a best-effort load-hide
> point, not a guaranteed door gate.

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
  no per-fragment locate-read, fixed 8-corner stencil, ≤ 32 taps in the straddle
  fallback). SH streaming layers over its id 34 v11 / id 35
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
      id 35 direct, ids 27/41/45 deltas, ids 47/48 billboard direct-scatter) at level install, on a booted level.
- [ ] The offscreen capture path reports steady-state frame time (and per-pass GPU time
      where `TIMESTAMP_QUERY` exists) for a named map on a named adapter, not just a PNG.
- [ ] **Premise finding (measure-and-record):** on the GTX 1660, frame time for a
      production-quality large map at two resident-SH footprints (a fine bake vs a
      coarse-spacing bake of the same map, both `--release` cold bakes — the ship footprint,
      not a warm approximation) is recorded, establishing whether — and by how
      much — resident-SH footprint moves frame time. If the delta is negligible, the epic
      stops here and the finding says so.

### Slice 2 — Cell clustering + PRL cluster directory (compiler-only)
- [ ] Clustering groups adjacent cells into units within an authored/parametrized
      byte/primitive budget; every runtime `cell_id` maps to exactly one cluster; cluster
      ids are stable across two `--no-cache` bakes of the same map (byte-identical
      directory).
- [ ] The cluster directory addresses each SH section per cluster as grid-relative
      cell/probe-index ranges (not byte offsets), so it is independent of the id 34 v11 /
      id 35 v4 byte layout and can be emitted before that branch lands; it round-trips
      through the loader, and a directory naming a cell/probe range outside the section's
      grid rejects with a named error. Resolving those index ranges to installable byte
      slices against the resident v11/v4 layout is Slice 3 work.
- [ ] For every affinity-cell delta that straddles a cluster boundary, the directory
      carries the owned/halo flag naming the one owning cluster (compile-time partition
      decision, grid-relative, independent of v11/v4 bytes). The assignment is deterministic
      across two `--no-cache` bakes (same byte-identical-directory gate). Slice 3 only reads
      this flag; it never re-derives the compile-time ownership partition at runtime.
- [ ] With the directory present but residency disabled, a booted level is visually
      identical to the pre-directory bake (the directory is inert until Slice 3), and the
      global SH bake view is unchanged (bake determinism gate still green).
- [ ] Clustering runs at a defined compiler stage without breaking the warm/cold cache
      contract; cold `--no-cache` remains the ship source of truth.

### Slice 3 — SH-probe per-cluster residency (the load/evict slice)
- [ ] Only SH clusters in the resident set are uploaded; crossing into a new area loads
      the entering cluster's SH and evicts a departed cluster's under a budget, verified
      by the resident-VRAM report falling below the whole-level baseline on a large map.
- [ ] Residency is driven by the per-frame visible-cell set plus a wider prefetch/hysteresis
      set; oscillating across a doorway (a cluster boundary alternating on adjacent frames)
      does not thrash load/evict (Orderings row 1). The hysteresis window is time-based and
      framerate-independent, matching the fog-volume reachability precedent (`rendering_pipeline.md §7.5`),
      not a frame or tick count, so thrash resistance is identical across refresh rates.
- [ ] A render frame samples only the installed-and-composed SH set, never a
      planner-targeted-but-uncomposed cluster: a partial or generation-mismatched install is
      never sampled, and a base folded into the indirect atlas whose direct (id 35/41/45) or
      billboard-scatter (id 47/48) companion has not recomposed is not yet counted resident
      (Invariant: atomic install; Orderings rows 7, 8). Installs drain at exactly one point
      per frame, before the SH compose pass (`rendering_pipeline.md §7.1 step 5`), and the
      install marks all three compose passes dirty for the installed cluster's slots. The
      residency state is latched there into one per-frame snapshot with two reads: compose
      reads the installed set — it composes every installed cluster's dirty slots and writes
      the miss placeholder for the rest — while forward, billboard, and fog read the
      installed-and-composed subset, which excludes a cluster installed at this frame's drain
      (it composes this frame but is not sampled until next frame). A completion landing after
      the drain point defers to the next frame, so no cluster flips miss→resident between
      compose and the forward read.
- [ ] A portal exposing a not-yet-resident cluster shows the defined fallback, never a
      lighting hole or uninitialized atlas (Orderings row 4; miss policy).
- [ ] A light whose influence spans clusters is not summed twice on any receiver, and
      L0/L1/L2 reconstruction is self-consistent across a cluster boundary (Invariant:
      no-double-count; boundary continuity).
- [ ] Evicting the cluster that owns a straddling affinity cell, while a co-covering
      neighbor stays resident, does not darken the neighbor's boundary: the owned cell is
      eviction-pinned while any resident cluster reconstructs against it — runtime ownership
      never transfers (the per-entry owner is compile-time-fixed; Slice 3 only reads it), so
      continuity rests on pinning the baked owner, or on the baked co-covering fallback order
      named in the Wire format (eviction-stable ownership). A halo copy is reconstruct-only and never substitutes for
      the lost accumulation.
- [ ] The SH sampler gains no binding and no per-fragment locate-read; forward fragment
      texture inventory and compose BGL budgets are unchanged. The no-binding half is a
      runnable regression guard on the pipeline budget tests
      (`forward_pipeline_sampled_texture_request_matches_bgl_definitions`); the
      no-locate-read half is a shader-review/grep gate, not a runnable test. Composes over
      id 34 v11 / id 35 v4.
- [ ] I/O and decode run off the Input → Game → Audio → Render → Present path; the event
      loop never blocks on a cluster load (a slow/synthetic-latency load degrades to the
      miss fallback, not a frame stall).
- [ ] **Perf finding (manual, resource-bound):** GTX 1660, production-quality large map,
      `--release` cold bake (the ship footprint), whole-load baseline vs SH-streamed — frame time and resident VRAM by section
      recorded; MacBook cross-check recorded; seam/pop hunt at boundaries recorded as a
      read. Full stress-warren attempted; recorded not-yet-evaluable if the box cannot
      bake/run it. Peak concurrent in-flight decode/host-buffer bytes during a teleport /
      fast-travel that overruns the prefetch horizon is recorded and stays a bounded fraction
      of the whole-map SH payload, never the whole set (`development_guide.md §1.4` — count
      coexisting representations).

### Slice 4 — Authored streaming hints
- [ ] A streaming-seam brush entity marks a portal/door as a best-effort
      load-hide point by cutting the cluster boundary and preferring far-side
      warm-up; opening cold retains the ambient-floor miss fallback. An
      always-resident brush entity pins a cluster resident; a priority/budget hint biases
      retention. Zero hints still yields the algorithm-default baseline unchanged.
- [ ] Hints are additive over clustering: no hints and zero-priority no-op
      hints preserve the algorithm-default partition and residency behavior.
      A seam coincident with an existing cluster boundary preserves partition
      and SH payload bytes but intentionally advances far-side warm-up.
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
atomic install and evicts departed clusters under a budget. The eviction victim is drawn
first from clusters outside the current target set (departed), by LRU; a prefetch cluster
still in the target set is evicted only under continued pressure once no departed cluster
remains, and never the same tick it installs merely for a zero sample-recency key; a
visible cluster is never evicted (Orderings rows 3, 3a). Cross-cluster light ownership
and a halo rule keep no-double-count and boundary reconstruction self-consistent. A miss
policy defines what a portal exposing a not-yet-resident cluster shows. The sampler stays
bandwidth-neutral — no binding, no locate-read. This slice is itself drafted thin-first
(a hard-gated, synchronous, no-eviction path that proves the atomic install and the
visible-cell drive) before async load and budget eviction fan out.

### Slice 4: Authored streaming hints
Additive brush-entity hints over the default clustering: a streaming-seam marker
(best-effort load-hide point), always-resident pin, and priority/budget bias, following the existing
`*_region` / `*_volume` FGD pattern and its compile-time validation. Default (no hints)
behavior is unchanged; hints constrain partition cuts and bias the planner.

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
| Streaming seam | seam marker at a portal/door | n/a (compiler-derived residency behavior) | brush entity, e.g. `streaming_seam_volume` (suffix pattern of `fog_volume`) | n/a |
| Always-resident | cluster pin | n/a | brush entity, e.g. `stream_resident_volume` | n/a |
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
  addressing into the SH sections as grid-relative cell/probe-index ranges (not byte
  offsets), resolved to installable byte slices against the resident v11/v4 layout at
  Slice 3; and, for each cross-cluster straddling affinity-cell delta, a per-entry owned/halo flag
  marking the single cluster that accumulates it, so compose accumulates the owned copy once
  and treats every other cluster's copy as reconstruct-only halo (atomic-install granule).
  Empty-list and single-cluster (whole-map = one cluster)
  cases encode explicitly. Mirrors the existing section-table discipline (per-section
  offset+size already exists in the container). One file, not sidecars — one authored
  level stays one logical PRL and one co-op content identity.
- **id 34 (v11) / id 35 (v4).** Owned by `adaptive-probe-spacing`; **not re-versioned by
  this epic**. Residency addresses ranges *within* these sections through the directory;
  it does not change the probe-record stride, node-scale semantics, prefix-sum slot
  derivation, or the stored set. A cluster's SH payload is a contiguous, independently
  installable slice of the same v11/v4 bytes. If v11/v4 cannot be sliced per cluster
  without reordering, that reordering is a compiler-side emit-order decision recorded in
  the Slice 3 brief — it must not alter what the sampler reads per probe.
- **Cross-section constraint at load.** The directory's per-cluster SH ranges must be
  consistent with every grid-matched companion section: the delta sections (ids 27/41/45)
  and the billboard direct-scatter sections (id 47, dense in x-fastest id-34 probe order;
  id 48, on id-45's CSR affinity layout). A resident cluster carries the companion
  entries its bricks need, or the miss/halo policy supplies them; any change to id 34's
  probe emit order mirrors into id 47 and id 48, or their reads misalign. Load rejects a
  directory that references a section version it was not built for.
- **Atomic-install granule.** A cluster's install unit is its base slice (id 34/35) plus
  every delta entry its bricks reference, the billboard direct-scatter companion entries
  (ids 47/48) for the cluster's probes where the level carries them, plus the neighbor-halo entries inside the
  reconstruction stencil, installed under one generation. A cluster is not counted composed
  until its halo is resident; base-without-halo is never fed to compose — it would leave a
  discontinuous or double-counted boundary. Because the SH compose passes' dispatch conditions (`rendering_pipeline.md §7.1 step 5`) fire on level-load, animated activity, and mask change but not on a mid-level install, a cluster install marks all three passes dirty for that frame — indirect (id 34 base + id 27), direct (id 35 + ids 41/45), and billboard-scatter (ids 47/48) each recompose the installed cluster's slots. A cluster counts composed only after every atlas it feeds has recomposed: a base folded into the indirect atlas while its id 47/48 remain uncomposed is not composed. A delta affinity cell (4×4×4 base probes)
  straddling a cluster boundary is owned by exactly one cluster in the directory and
  accumulated once in compose; the other cluster carries it as halo. Compose keys accumulation on the directory's per-entry owned/halo flag — it accumulates only the owned copy and reads halo copies for reconstruction only, or two resident clusters that both carry a straddling cell (owner + halo) double-count it. Ownership is eviction-stable and never transferred at runtime: the baked owner is eviction-pinned while any resident cluster reconstructs against it — otherwise evicting the owner (a departed cluster) would drop the cell from accumulation and darken a still-resident neighbor's boundary, even though the neighbor's halo copy survives for reconstruction. Promoting a halo copy to accumulate would be a runtime ownership recompute, which the owned/halo keying forbids; so if pinning departed owners costs too much budget, baking a co-covering fallback order into the directory (a Slice 2 wire change) is the owner-decided alternative — still read, never recomputed. Halo entries are
  duplicated into each cluster that reconstructs against them, not shared, so evicting a
  neighbor never removes a resident cluster's boundary data.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A frame samples only a generation-matched SH set | Slice 3 (atomic install) | Threatened by partial upload, an evict racing an install, a mid-frame residency swap, and by a cluster the planner has targeted but whose install has not yet reached the SH compose pass (`rendering_pipeline.md §7.1 step 5`) | Slice 3 AC (atomic install; no partial sample). "Resident" for sampling means installed-and-composed, not planner-targeted. A completed install lands before the compose pass that consumes it, or the cluster stays on the miss policy that frame — a targeted-but-uncomposed cluster is never sampled as uninitialized or stale probe data. Installs drain at exactly one point per frame, before the SH compose pass (`rendering_pipeline.md §7.1 step 5`); residency state is latched there into one per-frame snapshot with two reads — compose reads the installed set (composing every installed cluster's dirty slots, miss placeholder for the rest) while forward, billboard, and fog read the installed-and-composed subset (the `LightTermMask` snapshot precedent, `rendering_pipeline.md §4`), which excludes a cluster installed at this frame's drain until it has composed — so a completion that lands after that point defers to the next frame rather than flipping a cluster from miss to resident between compose and the forward read |
| No receiver sums one physical light twice | bake (existing); Slice 3 cross-cluster ownership/halo | Threatened where a light's influence/delta spans clusters and both resident | Slice 3 AC (cross-cluster no-double-count) |
| SH reconstruction is continuous across a cluster boundary | Slice 3 halo rule + eviction-stable ownership | Threatened where adjacent clusters differ in resident state or L0/L1/L2 level, or the owner of a straddling cell is evicted while a covering neighbor stays resident | Slice 3 AC (boundary self-consistency); Slice 3 AC (eviction-stable ownership); seam hunt |
| Sampler stays bandwidth-neutral (no binding, no per-fragment locate-read, fixed 8-corner stencil — ≤ 32 taps in the straddle fallback) | `adaptive-probe-spacing`; preserved by Slice 3 | Threatened by any residency indirection added to the per-fragment path | Slice 3 AC (no binding / inventory + budget guard) |
| One residency substrate: clustered cells on the visible-cell signal | Slice 2 (directory), Slice 3 (planner) | Threatened by a bespoke per-subsystem spatial query | Slice 2/3 ACs; design review |
| I/O, decode, CPU prep stay off the frame path | Slice 3 (off-frame worker) | Threatened by a synchronous load on the event loop | Slice 3 AC (no frame stall under load latency) |
| One authored level = one logical PRL / one co-op content identity | Slice 2 (single-file directory) | Threatened by sidecars or per-cluster files | Wire format; co-op admission unchanged |
| A portal never exposes a lighting hole | Slice 3 miss policy | Threatened by a visible portal into a not-yet-resident cluster | Slice 3 AC (miss fallback) |

## Orderings

Residency introduces mutable per-frame state, prefetch timers, and load completions.
Pinned scenarios the Slice 3 brief's tests cite:

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| 1 | Doorway oscillation | Camera crosses a cluster boundary back and forth on adjacent frames | Hysteresis holds both clusters resident within budget; no load/evict thrash |
| 2 | Prefetch vs latency | Cluster enters the prefetch horizon, then becomes visible before its load completes | Miss fallback until the generation-matched install lands; no stall, no hole |
| 3 | Eviction under pressure | Resident set exceeds budget while a new cluster must load | Eviction draws first from clusters outside the target set (departed), coldest-first by LRU; then, under continued pressure, from prefetch clusters inside the target set, coldest-first by the same LRU/priority key as the departed tier; a visible cluster is never evicted. An evicted still-targeted prefetch cluster is suppressed from re-targeting while budget pressure persists and its prefetch coverage is unchanged, so the planner does not re-add it from the horizon next frame merely because the camera has not moved — without that persistence a stationary camera churns the prefetch set (re-target → reload → re-evict) indefinitely. Suppression clears when pressure relents or the horizon changes, so a genuine re-approach still reloads it. A departed owner eviction-pinned for a resident neighbor's accumulation (runtime ownership never transfers; Slice 3 eviction AC) is not an eligible victim — eviction skips it like a visible cluster, even though it sits in the departed tier. Eviction terminates when residency is under budget or only clusters it cannot evict remain — visible, always-resident, and eviction-pinned owners — the last two are the row 3a overshoot, not an infinite loop. A just-installed prefetch cluster is not evicted the tick it lands merely for a zero sample-recency key. (Priority bias and always-resident pins are Slice 4 hints — their retention is verified there, not in Slice 3.) |
| 3a | Working set exceeds budget | Visible + always-resident clusters alone exceed the residency floor (whole-map = one cluster, or one cluster larger than the floor) | The floor is a relief target, not a hard cap: residency exceeds it rather than evict a visible or always-resident cluster, and the overshoot is logged once per onset (edge-triggered when residency first exceeds the floor, and again only after it drops back under and re-exceeds), never once per frame: a persistent degenerate working set overshoots every frame, and a per-frame log would spam the render hot path (`development_guide.md §6.1`). The Slice 2 clustering byte budget is held ≤ the residency floor minus always-resident overhead, so no single non-degenerate cluster forces this case; a crowded visible set of many in-budget clusters can still overshoot on a non-degenerate map, held under this same overshoot-not-evict policy. Eviction-pinned departed owners (runtime ownership never transfers; Slice 3 eviction AC) are a further over-floor source whenever a resident neighbor still accumulates against them — held under the same policy, unless the owner bakes a co-covering fallback order to shed the pin (Wire format granule) |
| 4 | Portal into un-resident cluster | A portal becomes visible exposing a cluster not yet resident | Authored seams prefer far-side warm-up before opening; a cold opening and every other miss use the conservative ambient-floor placeholder, never uninitialized atlas |
| 5 | Level reload / teardown | Level unload or reload while loads are in flight | In-flight loads are cancelled or drop at install; residency state clears. The generation counter is monotonic across level loads (never resets), or completions carry the level content identity, so a level-A completion can never alias a level-B generation and install into it |
| 5a | Stale completion, same level | A cluster's load completes after the player retreated and the planner dropped it from the target set | The completion is dropped at install and budget is not charged. Two independent checks gate install, not one: a target-set-membership check rejects a same-level retreat completion whose cluster the planner dropped (the level-monotonic generation counter does not change on a same-level target-set edit, so it cannot catch this), while a generation/content-identity check rejects a cross-level completion — per-level cluster ids collide, so a level-A cluster-5 completion draining during level-B would pass a membership test whenever level-B targets its own cluster 5, and only the identity match keeps A's bytes out of B's slot. A completion draining alongside both must pass both |
| 6 | Co-op divergent residency | Two peers hold different resident cluster subsets | Both render correct local lighting; logical content identity agrees at admission; residency never crosses the wire as gameplay state |
| 7 | Target vs installed-and-composed | Planner marks cluster X resident the frame X becomes visible; X's install has not been folded into the composed atlas (`rendering_pipeline.md §7.1 step 5`) | X shows the miss fallback that frame — "resident" for sampling means installed-and-composed, not planner-targeted; X is never sampled as uninitialized or stale probe slots |
| 8 | Batch of N installs one frame | N completions drain before a frame's install pass, N in {0, 1, many} | Each installs atomically under its own generation; the composed atlas is coherent at every prefix; uninstalled clusters stay on the miss policy; N=0 is a no-op |
| 9 | Camera crossing exceeds prefetch horizon | Teleport or fast-travel: per-frame displacement exceeds the prefetch depth, so a whole region becomes visible cold | Bounded mass miss-fallback for those frames, never a frame stall (I/O stays off the frame path); the region composes as its installs land. "Bounded" is pinned by two caps: a maximum concurrent in-flight load/decode count, so the transient decode buffers of a mass cold-in never coexist as the whole-map SH set this epic exists to keep un-resident (`development_guide.md §1.4` — count coexisting representations), and a per-frame install/upload cap, so a mass completion installs across frames rather than in one GPU upload burst. Under that cap, ready completions install visible-cluster-first and prefetch clusters yield — the install-side mirror of row 3's eviction priority — so a visible cluster's miss-fallback duration is bounded by the visible backlog over the cap, never starved behind off-screen prefetch installs under sustained pressure. The render-side miss cost is already bounded by the visible-cell set |
| 10 | Empty cluster list at runtime | Directory encodes zero clusters (degenerate bake) while the map carries SH geometry | Defined and never silently dark: load rejects with a named error, or the whole map falls back to the ambient-floor miss placeholder |

## Open questions

- **Residency covers most of the map at 1 m (finding, 2026-09-25).** The SH compose perf spike
  (`ready/perf-sh-compose-sampled-row-gating/spike-findings.md`) measured stress-warren-mini at
  1 m: 22 of 482 clusters installed, covering 91% of affinity bricks, and the owner closure of
  1–3 visible clusters spanning 18–19 of them. Likely cause (unverified): coarsened L1/L2 owner
  nodes with large patch spans. It limits residency relief and ruled out a cluster-grain compose
  gate. That brief doesn't address it; targeting and owner-closure width belong to this epic.
- **Premise magnitude (Slice 1, measurement-owned).** Whether the 1660 sluggishness is
  VRAM oversubscription (driver thrash) vs bandwidth-bound within 6 GB, and the size of
  the frame-time win from shrinking resident SH. Slice 1's finding decides whether Phase 2
  proceeds. Recommended: proceed only on a material, recorded frame-time delta.
- **Cluster byte/primitive budget (Slice 2).** The budget *number* is delegated to the
  Slice 1 distribution measurement (per-cluster footprint after a dry-run clustering pass).
  The partition *rule* is not delegated — it is fixed resource-agnostic (adjacency + budget)
  so the substrate generalizes; only the threshold is measured. The threshold is additionally
  held at or below the residency floor minus always-resident overhead (Orderings row 3a), so
  no single non-degenerate cluster can exceed the floor.
- **Residency budget floor — decided (owner-accepted).** The floor is a relief target, not
  a hard cap (Orderings row 3a): a working set that cannot fit — whole-map = one cluster,
  authored always-resident pins plus the visible set exceeding the floor, or eviction-pinned
  boundary owners — overshoots rather than evict a visible cluster or reject a structurally
  valid map. Sized to the 6 GB GTX 1660 desktop floor (matches the existing lighting
  perf-floor hardware). The floor *number* and whether the cap is a player option or a build
  constant remain Slice 3 decisions.
- **Miss policy default — clarified for Slice 4.** Authored seams are
  best-effort warm-up points, not gameplay or visual door gates. A cold seam
  and every other miss retain Slice 3's conservative ambient-floor SH
  placeholder; no door waits on async residency.
- **v11/v4 per-cluster byte-slice mechanics (Slice 3).** The cluster directory addresses
  SH data by grid-relative cell/probe index, so the directory itself is unaffected by the
  v11/v4 byte layout. What remains for Slice 3 is resolving those index ranges to
  installable byte slices against the resident layout: whether id 34 v11 / id 35 v4
  payloads gather per cluster as stored, or need a sampler-neutral compiler-side emit-order
  change. Resolved against the landed branch when Slice 3 is drafted.
- **Generalization door.** The substrate is built to let geometry/lightmap-layer/SDF/
  fog/acoustics subscribe later. Not built here; flagged so Slice 2's directory shape does
  not foreclose a second resource cheaply.
