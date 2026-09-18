# SH Probe Streaming — Per-Cluster Residency

> Slice 3 executable draft. Parent: `context/plans/in-progress/sh-probe-streaming/`.
> Slice 1 and Slice 2 GPU reads remain `not-yet-evaluable`; that does not block
> this slice. All new names below are proposed design.

## Goal

Keep only the SH payload needed by visible and near-visible cell clusters in a
renderer-owned GPU pool. Load and prepare cluster payloads off the frame path,
install one generation-matched batch before SH compose, and evict cold clusters
without lighting holes, boundary darkening, or duplicate light accumulation.

## Scope

**In:** a synchronous no-eviction proof gate; one-file cluster payloads; visible-cell
planning; two-hop prefetch; time-based hysteresis; bounded async reads and decode;
renderer-owned fixed-budget pools with controlled growth for non-evictable working
sets; atomic install/compose/sample promotion; LRU eviction; baked-owner pinning;
ambient-floor miss fallback; reload cancellation; diagnostics and manual performance
evidence.

**Out:** authored streaming hints or seam presentation (Slice 4); geometry, BVH,
lightmap, SDF, fog, acoustic, or texture streaming; networked residency state;
changing cell clustering; changing SH bake rays/coarsening; changing ids 34/35 or
any existing section version/cache epoch; a new sampler binding; a per-fragment
residency lookup; `unsafe`; a player-facing budget option.

## Shared context

Every task reads `context/lib/index.md`, `development_guide.md`, `testing_guide.md`,
`build_pipeline.md`, `rendering_pipeline.md`, this whole brief, and `research.md`.
Also read the parent epic, both earlier child briefs, and the Slice 1/2 records under
`measurements/sh-probe-streaming/`. Use current source over parent assumptions.

This slice crosses persistent wire, loader, renderer, and frame-order contracts. All
implementation tasks require **xhigh** reasoning. Task handoffs include the full AC,
wire, state, failure, and invariants tables. Keep public re-exports during mechanical
splits. Do not change existing section epochs, section-internal versions, container
entry versions, binding numbers, or cache-stage epochs.

## Decisions

### Runtime mode and compatibility

Section 49 remains id 49 v1 with the shipped 64-primitive / 32-cell defaults. Add
optional `ClusterShPayloads` id 50, container version 1, internal epoch 1. Id 50 is an
independently readable cluster-major copy of the installable SH working set. Existing
ids 27/34/35/41/45/47/48 remain byte-identical and preserve legacy whole-load behavior.
This duplication is deliberate: current v11/v4 BC6H bytes are encoded as global atlas
block rows, so a 6x6 logical tile shares 4x4 compression blocks with adjacent tiles and
cannot be gathered into an arbitrary residency slot without decoding/re-encoding or
retaining the whole source atlas. The sparse companions are likewise monolithic codecs.

When valid ids 49 and 50 are both present, runtime selects streaming. Missing either
section selects the existing whole-load path. Id 50 without id 49, mismatched cluster
counts/resource inventory/source versions, or a malformed index rejects at level load.
Unknown id 50 remains skippable to older loaders, preserving one PRL and one content
identity. Id 50 is built after finalized section selection, uncached, from borrowed
final outputs. It changes no bake result or existing cache key/epoch.

Runtime mode is explicit while evidence is collected: `POSTRETRO_SH_STREAMING=off`
forces legacy whole-load, `sync-proof` selects the thin synchronous/no-eviction path,
and `async` selects the complete policy. A valid id-50 PRL defaults to `async`; a PRL
without id 50 ignores the variable and stays legacy. The variable is a developer/test
gate, not a player option or stable content surface.

The loader does not retain whole SH-family bodies in streaming mode. It reads the
container, ids 49/50 metadata, and small global descriptor tables, then stores an
immutable `ShStreamManifest` with the PRL path, file identity, bounded chunk index,
cluster graph, and global animation/selection metadata. The old `LevelWorld` SH section
fields remain populated only in legacy mode. Accessors expose shared global metadata so
startup and scripting do not branch on storage mode.

### Tile and sparse payload mechanics

Each streamed base tile keeps the v11/v4 logical 6x6 texels and border semantics but is
stored alone in an 8x8 physical cell. Edge texels dilate through the two padding
rows/columns before encoding. Production BC6H chunks encode each isolated cell to four
4x4 blocks; uncompressed-debug chunks retain exact `Rgba16Float` bits in the same 8x8
cell. The renderer's existing grid uniform gains a physical
tile-stride value; logical tile dimension/border and all sample taps stay unchanged.
Whole-load mode uses stride 6. Stream mode uses stride 8. No binding or per-fragment
read is added.

The compiler resolves id-49 dense ranges through the canonical v11 stored-node prefix
sum. One cluster chunk contains every unique stored node its dense closure references,
with id-34 and present id-35 tiles in identical local-slot order. Per-probe patches carry
global dense index, depth moments, and a local-slot indirection word. The renderer adds
the allocated pool base slot when patching compose indirection; the sampled depth-moment
texture remains invalid until promotion after compose.

Sparse ids 27/41/45/48 are sliced by final affinity-cell rows. A chunk stores owned rows
and the halo rows needed by its directory coverage. Only the baked owner row may enter
compose CSR; halo rows establish an owner dependency and provide an independently
loadable copy, never another accumulation. Id 47 stores dense global-probe patches.
Global animation descriptors, samples, descriptor-index maps, light-selection maps,
grid metadata, and CSR offset tables stay in a small always-resident metadata floor.
Missing CSR rows encode equal start/end offsets. Chunk-local payloads carry no wgpu
types.

### Residency policy

The planner maps the current `VisibleCells` through id 49. `Culled` uses those clusters;
`DrawAll` makes every cluster visible. Prefetch is the unique two-hop cluster set over
runtime portal adjacency. Visible clusters are required immediately. Prefetch-only
clusters remain targets while covered. Any cluster that leaves both sets remains a
hysteresis target for 2.0 seconds of monotonic render time. This duration is independent
of refresh rate.

Default requested-payload floor is 256 MiB on all desktop adapters. Charge chunk decoded
bytes plus renderer pool bytes and the fixed metadata floor; use checked `u64` sums.
The renderer sizes each family pool to at least its largest single-cluster requirement,
then distributes the remaining floor in proportion to finalized whole-level family
bytes. The effective floor is the resulting sum and is reported. Pools use deterministic
first-fit free ranges with coalescing. They never compact live slots.

Eviction first considers departed, non-hysteresis clusters, then prefetch-only clusters.
Within a tier use `(last_visible_time, last_target_time, cluster_id)` ascending. Never
evict visible clusters, a cluster installed during the current drain, or an owner needed
by any installed/sampleable halo cluster. A pressure-evicted prefetch cluster is
suppressed until the two-hop horizon changes or pressure clears. If non-evictable demand
exceeds a pool, grow that pool geometrically at the drain boundary, re-upload retained
decoded chunks under the same generation, and edge-log one overshoot onset. Pool growth
is exceptional; it preserves correctness instead of dropping visible lighting. It never
runs file I/O or decode on the render thread.

Async limits are four in-flight read/decode jobs and two cluster installs per frame.
Ready installs sort visible first, then prefetch, then hysteresis, with cluster id as the
tie-break. A ready completion must match both the level content identity/generation and
current target membership. Same-level retreat and cross-level stale completions drop
without budget charge.

### Atomic frame lifecycle and miss behavior

The renderer drains residency once, at the start of `record_scene_passes`, before direct,
billboard, and indirect SH compose. Drain first invalidates evicted probes in the sampled
depth-moment texture, then installs ready base/delta/scatter data and compose-side
indirection into pool slots. All three compose paths receive the dirty affinity ranges.
Compose uses the installed set. Forward, billboard, fog, and SDF reads use the prior
installed-and-composed set.

A newly installed cluster stays `InstalledUncomposed` for that frame. Its sample-side
depth-moment indirection remains zero while all applicable compose passes write its slots.
At the next frame's drain, queue ordering makes the prior compose complete before its
sample-side words are published, and the cluster becomes `Sampleable`. A completion that
arrives after the drain waits. Eviction invalidates sample words before slot reuse.

The all-zero indirection word is the general miss placeholder. Existing SH sampling then
drops those corners and renormalizes survivors; if none survive it returns the ambient
floor. Id-47 patches for a missing cluster remain zero, and billboard scatter follows the
same SH-validity weights. Slice 4 owns authored seam gates; this slice supplies the
conservative placeholder everywhere.

Compose dispatches only dirty id-49 affinity ranges. Reuse each pass's existing grid
uniform binding as a dynamic-offset array of `(range_start, range_count)` records and
dispatch one-dimensional workgroups. WGSL converts the flattened affinity index back to
xyz. This changes no binding number/count and prevents compose cost from scaling with the
whole map when only a few clusters change.

## Acceptance criteria

- [ ] **AC1 — Preserved contracts:** existing ids 27/34/35/41/45/47/48, id 49 v1,
      container v4, binding numbers/counts, shader sample stencil, and cache epochs remain
      unchanged. Legacy PRLs without id 50 load and render through the prior whole path.
- [ ] **AC2 — Payload wire:** id 50 round-trips deterministically; its source-version and
      inventory cross-check rejects drift; each cluster chunk is independently seekable,
      hash-checked, bounded before allocation, and contains the exact v11/v4 node closure,
      dense scatter patches, and owned/halo sparse rows named by id 49.
- [ ] **AC3 — Thin proof:** a hard-gated synchronous no-eviction mode targets clusters
      from real visible cells, installs zero/one/many clusters generation-safely, composes
      all applicable families, and exposes each cluster only on the next frame. Disabling
      the gate preserves legacy output.
- [ ] **AC4 — Miss behavior:** a visible but unavailable cluster samples valid resident
      neighbors and otherwise ambient floor. No uninitialized/stale atlas slot or lighting
      hole appears in CPU reference tests or manual seam inspection.
- [ ] **AC5 — Async bounds:** production streaming performs file read, checksum, decode,
      and CPU preparation off the Input → Game → Audio → Render → Present path. Synthetic
      250 ms latency does not stall frame submission. At most four jobs and two installs
      per frame exist; ready visible work cannot starve behind prefetch.
- [ ] **AC6 — Atomic generations:** same-level retreat completions and cross-level stale
      completions both drop. Reload/unload cancels or drains workers, clears targets,
      invalidates sampled words, releases GPU pools, and never installs prior-level bytes.
- [ ] **AC7 — Policy:** visible plus two-hop prefetch drive targets; 2.0-second hysteresis
      prevents adjacent-frame doorway thrash at 30/60/144 Hz. Departed-first LRU eviction,
      persistent prefetch suppression, and just-installed protection follow the pinned key.
- [ ] **AC8 — Ownership and continuity:** every resident halo cluster retains its baked
      owner. Only owned sparse rows accumulate. Owner eviction cannot darken a resident
      boundary, and L0/L1/L2 reconstruction stays self-consistent across cluster seams.
- [ ] **AC9 — Budget:** default/effective floor, fixed metadata, pool capacity, logical
      residency, in-flight bytes, decoded-ready bytes, and overshoot are reported without
      double-count. A nondegenerate large fixture remains below whole-load requested SH
      allocation; non-evictable overshoot grows safely and logs once per onset.
- [ ] **AC10 — Sampler and compose:** forward sampled-texture/BGL budget tests stay green;
      grep/review finds no fragment-stage residency locate read. Dirty-range compose uses
      existing bindings and does not dispatch over unrelated whole-map affinity cells.
- [ ] **AC11 — Determinism and resource lifetime:** two cold worker-count variants emit
      identical id 49/id 50 and unchanged legacy section bodies. Teleport stress proves
      peak jobs + decoded chunks + retained CPU payloads stay bounded and cleanup removes
      generated PRLs, reports, caches, and crate-scoped build artifacts when disk is low.
- [ ] **AC12 — Manual finding:** record whole-load versus streamed requested SH bytes and
      frame-time windows. Prefer the GTX 1660 Super with the largest map that boots; also
      record a mid-sized map on this MacBook Pro. Failed adapters/full-map runs are
      `not-yet-evaluable`, not negative evidence. Record seam/pop inspection separately.

## Boundary inventory

No JS, TS, Luau, FGD, network, or player-option boundary is added.

| Meaning | Proposed Rust owner | Wire / runtime |
|---|---|---|
| Stream payload | `level-format::cluster_sh_payloads::ClusterShPayloadsSection` | PRL id 50; container v1; epoch 1 |
| Manifest | `level-loader::sh_stream::ShStreamManifest` | metadata/index only; immutable path + content identity |
| Prepared chunk | `level-loader::sh_stream::PreparedShCluster` | plain CPU bytes; no wgpu |
| Planner | `postretro::sh_streaming::ShResidencyController` | session-local; never networked |
| Renderer state | `renderer::ShResidencyState` | generation + target/install/sample sets and GPU pools |
| Budget | internal desktop default | 256 MiB requested-payload floor; no CLI/KVP in this slice |
| Timing | monotonic render seconds | prefetch depth 2; hysteresis 2.0 s |
| Caps | internal constants | 4 in-flight jobs; 2 installs/frame |
| Mode gate | `POSTRETRO_SH_STREAMING` | `off`, `sync-proof`, `async`; valid id 50 defaults to `async` |

## Wire format

All id-50 integers are unsigned little-endian. Floating-point values do not occur.
Lengths and offsets are checked in `u64`, converted to `usize` only after file and policy
bounds pass. No padding, implicit alignment, platform layout, sentinel offset, or trailing
bytes. The section is header + source table + fixed cluster index + payload blob.

**Header (72 bytes):** `u32 epoch=1`, `cluster_count`, `source_count`, `flags=0`,
`grid_dimensions[3]`, `affinity_dimensions[3]`, `tile_dimension=6`, `tile_border=1`,
`physical_tile_stride=8`, `reserved=0`, `u64 payload_bytes`, `u64 reserved=0`.

**Source record (16 bytes):** `u32 section_id`, `u32 internal_version`, `u32 kind`,
`u32 reserved=0`. Records are unique and sorted by section id. Kinds are 0 dense-base
atlas (34/35), 1 sparse-affinity (27/41/45), 2 dense-scatter (47), 3 sparse-scatter
(48). Present finalized resources must match id 49 exactly. Known internal versions are
27 v6, 34 v11, 35 v4, 41 v4, 45 v4, 47 v1, and 48 v1. They are copied from
the current source codec and cross-checked at load, not inferred from id 50.

**Cluster index record (80 bytes):** `u64 payload_offset` relative to payload blob,
`u64 payload_len`, `u64 decoded_bytes`, `u64 requested_resident_bytes`,
`u32 stored_tile_count`, `u32 dense_patch_count`, `u32 affinity_patch_count`,
`u32 flags=0`, then `blake3[32]` over the exact chunk bytes. Records are implicit cluster
id order and must consume non-overlapping, ascending payload ranges exactly. Empty chunks
use zero counts, zero length, the BLAKE3 empty hash, and no payload bytes.

**Chunk header (16 bytes):** `u32 chunk_version=1`, `u32 cluster_id`, `u32 block_count`,
`u32 reserved=0`. **Block record (32 bytes):** `u32 section_id`, `u32 block_kind`,
`u32 element_count`, `u32 flags`, `u64 offset` relative to chunk start, `u64 length`.
Block records are sorted by `(section_id, block_kind)`, unique, non-overlapping, and point
after the complete block table. Required block kinds:

| Kind | Body |
|---|---|
| 0 probe patches | `element_count × 16`: global dense index, local indirection word, mean-distance f16, mean-square f16, reserved u32 |
| 1 isolated atlas | u32 format (1 BC6H, 0 RGBA16F), u32 local slot count, u32 width, u32 height, u32 layers, then tagged layer-major bytes; dimensions derive from 8x8 cells |
| 2 dense scatter patches | `element_count × 12`: global dense index + RGBA f16 |
| 3 sparse rows | u32 row count, u32 entry count, u32 tile-f16 count, u32 reserved; row records `(global affinity index, first entry, entry count, role)`; entry records `(light/descriptor index, level, first tile-f16, tile-f16 count)`; then raw f16 tiles |

Role is 1 owned or 2 halo, matching id 49. Base/direct atlas blocks share identical
local slot counts and geometry. Dense indices are strictly ascending. Sparse rows are
strictly ascending and entries retain their source CSR order. Counts, levels, payload
lengths, node closure, owner, and halo status validate against ids 49 and source metadata.

## Runtime states and failure behavior

| State | Sample-side word | Budget | Exit |
|---|---|---|---|
| `Absent` | invalid | none | target queues load |
| `Queued` | invalid | in-flight bytes | completion or cancellation |
| `Ready` | invalid | decoded-ready bytes | drain installs or drops |
| `InstalledUncomposed` | invalid | resident bytes | all applicable compose dispatches encoded |
| `Sampleable` | valid | resident bytes | eviction or level teardown |
| `Failed` | invalid | none | horizon change retries once; repeated same hash stays failed |

| Failure | Behavior |
|---|---|
| Structural id-50/version/inventory/index error | reject level with named `ClusterShPayloads*` error |
| Chunk range/size exceeds index or policy | reject before allocation |
| Runtime seek/read/hash/decode failure | warn once per cluster/hash, keep miss fallback, mark failed |
| Completion not targeted | drop before renderer; no budget charge |
| Generation/content identity mismatch | drop before renderer even if cluster id is targeted |
| Pool fragmentation with enough total free bytes | evict eligible ranges, coalesce, retry; grow only when remaining demand is non-evictable |
| Device loss/submit failure | existing renderer fatal path; never promote sampleability |
| DrawAll or non-evictable set over floor | grow, edge-log overshoot, preserve correctness |
| Missing id 49 or id 50 | legacy whole-load path; no partial streaming |

Named codec failures are `ClusterShPayloadsVersionMismatch`,
`ClusterShPayloadsInvalidData`, `ClusterShPayloadsSourceMismatch`,
`ClusterShPayloadsRangeOutOfBounds`, `ClusterShPayloadsSizeOverflow`, and
`ClusterShPayloadsHashMismatch`. Loader/worker errors wrap these names without replacing
the existing errors for source ids 27/34/35/41/45/47/48.

## Invariants

| Invariant | Established by | Threatened at | Verified by |
|---|---|---|---|
| Frame samples only generation-matched, composed data | Tasks 8–10 | completion drain, slot reuse, reload | AC3, AC4, AC6 |
| Physical light accumulates once | Tasks 6, 9, 12 | owner/halo duplication | AC2, AC8 |
| Boundary reconstruction keeps baked owner | Tasks 8, 12 | owner eviction | AC7, AC8 |
| Sampler keeps bindings and fixed taps | Tasks 2, 9 | pool indirection | AC1, AC10 |
| I/O/decode stays off frame path | Task 11 | sync fallback leaking to production | AC5 |
| Working set is bounded or explicit overshoot | Tasks 8, 9, 12 | teleport, DrawAll, fragmentation | AC9, AC11 |
| One PRL / one content identity | Tasks 5–7 | sidecar temptation | AC1, AC2, AC6 |
| Existing bake/cache output is unchanged | Tasks 4–7 | payload extraction | AC1, AC11 |

## Tasks

### Task 1: Split loader ownership before streaming

Behavior-preserving split of `prl.rs` and `prl_loader.rs`: move LevelWorld lighting data
and accessors into a focused module; move container inventory/section-read helpers out of
the production assembly function. Preserve feature gates, constructors, exports, optional
fallback order, and every current caller. No id-50 behavior.

### Task 2: Split renderer SH resources before pools

Behavior-preserving extraction from `sh_volume.rs`, `sh_compose.rs`,
`direct_sh_compose.rs`, and `renderer_render_frame.rs`: isolate atlas upload/allocation,
compose carrier construction, dispatch planning, and pre-scene SH orchestration. Preserve
all wgpu descriptors, BGL entries, dispatch conditions, diagnostics, and public APIs. No
residency behavior.

### Task 3: Split application frame and capture seams

Extract visible-cell-to-render preparation from `main.rs` and repeated capture setup from
`capture/driver.rs` into focused modules. Preserve Input → Game → Audio → Render → Present,
visibility results, capture output, and renderer call order. No planner behavior.

### Task 4: Split compiler final-lighting publication

Extract final SH inventory, post-bake metadata stages, and pack descriptor construction
from oversized `pipeline.rs`, `pack.rs`, and `pack_output.rs`. Preserve stage order,
borrowed finalized-section presence, section order, one-payload-at-a-time serialization,
and cold/warm output. Generalize the extracted writer seam to accept a bounded streaming
encoder while every existing section still uses its current one-shot byte encoder. No id
50 behavior.

### Task 5: Add id-50 codec and shared validator

Implement the exact wire above in level-format, register id 50, and add overflow,
ordering, range, checksum, source-version, resource-inventory, owner/halo, node-closure,
and empty/single-cluster tests. Expose metadata/index parsing separately from chunk decode
so loader startup never reads the payload blob.

### Task 6: Bake deterministic cluster chunks

Consume Task 4's borrowed finalized inventory plus id 49. Resolve dense ranges through
the v11 stored-node prefix, independently encode 8x8 physical cells in the source
irradiance format, slice sparse rows, retain halo copies, and append id 50 after id 49.
Spool one chunk at a time to a session-owned temporary file so hashes/indexes are known
before the section header is written; stream that spool into the staged PRL and remove it
on success or failure. Never retain the whole id-50 payload in RAM. Preserve every old
body/version/epoch. Add worker-count determinism and legacy-body identity tests.

### Task 7: Load metadata without whole SH bodies

Consume Tasks 1 and 5. Add seek-based container reads and `ShStreamManifest`; streaming
mode decodes only global metadata and id-50 indexes, while legacy mode remains byte-for-
byte behavior-compatible. Add content identity, cluster adjacency, accessors for startup
animation/selection metadata, named rejection, and chunk read/decode APIs.

### Task 8: Build the pure residency planner and synchronous gate

Add the state machine, cell→cluster map, two-hop horizon, 2.0-second hysteresis, target
membership, generation/content identity, ready priority, owner-pin closure, suppression,
LRU key, exact byte charging, and four/two caps as plain Rust. The hard-gated synchronous
source reads one chunk at target time and never evicts; unit tests prove visible drive and
zero/one/many lifecycle transitions before GPU work exists.

### Task 9: Add renderer pools and atomic compose lifecycle

Consume Tasks 2, 5, 7, and 8. Build renderer-owned atlas/buffer pools, sampled and compose
indirection mirrors, id-47 patches, CSR offset patching, first-fit allocation, dirty-range
dynamic uniforms, all-three-pass compose, next-frame sample promotion, miss invalidation,
and requested-byte ledger rows. Keep every binding number/count and all wgpu inside the
renderer.

### Task 10: Connect visible cells and prove the thin path

Consume Tasks 3, 8, and 9. Session owns the controller; after visibility it updates
targets and passes a bounded drain batch into renderer before scene recording. Capture
uses deterministic synchronous preload for its fixed visible set. Test install timing,
late completion deferral, partial-family refusal, and legacy-off parity.

### Task 11: Add bounded async read and decode

Replace only the production synchronous source with four named worker jobs. Jobs seek one
indexed chunk, verify BLAKE3, decode CPU data, and send generation/content-tagged results.
Bound queued/in-flight/ready bytes, prioritize visible requests, cancel cooperatively on
reload, and prove synthetic latency never blocks the render thread. Keep the synchronous
gate for deterministic tests.

### Task 12: Add eviction, growth, hysteresis, and diagnostics

Enable departed-first then prefetch LRU eviction, owner pins, pressure suppression,
two-second retention, two-install drain cap, exceptional pool growth, and edge-triggered
overshoot logging. Extend the existing SH residency report with fixed metadata, pool
capacity, logical occupancy, in-flight/ready bytes, target/state counts, misses, installs,
evictions, retries, and high-water marks.

### Task 13: Verify and record the residency finding

Run focused, workspace, cold determinism, reload/teleport, shader-budget, and leak/resource
checks. Record whole versus streamed reports and seam inspection under
`measurements/sh-probe-streaming/cluster-residency.md`. Prefer GTX 1660 Super large-map
release/cold evidence; also attempt a mid-sized MacBook map. Record unavailable GPU runs
as `not-yet-evaluable`. Remove generated PRLs/reports/PNGs/caches. After every phase check
free disk; below 10 GiB use only crate-scoped `cargo clean -p` for PostRetro crates.

## Sequencing

**Phase 1 (concurrent):** Tasks 1, 2, 3, 4 — behavior-preserving splits.

**Phase 2 (sequential):** Task 5 — pins the shared id-50 contract.

**Phase 3 (concurrent):** Tasks 6, 7, 8 — compiler, loader, and pure planner consume Task 5.

**Phase 4 (sequential):** Task 9 — consumes renderer split, manifest, codec, and planner.

**Phase 5 (sequential):** Task 10 — connects and proves the synchronous thin path.

**Phase 6 (sequential):** Task 11 — replaces production sync reads with bounded async work.

**Phase 7 (sequential):** Task 12 — enables budget eviction after async lifecycle is proven.

**Phase 8 (sequential):** Task 13 — integrated proof and measurement record.

## AC-to-proof map

| AC | Producing tasks | Primary proof |
|---|---|---|
| 1 | 1–7, 9 | legacy parity, registry/version/BGL tests |
| 2 | 5–7 | codec/property/cold determinism tests |
| 3 | 8–10 | synchronous state and frame-order tests |
| 4 | 9–10 | CPU sampler reference + seam read |
| 5 | 11 | latency and cap tests |
| 6 | 7–12 | reload/stale-completion tests |
| 7 | 8, 12 | deterministic-time planner tests |
| 8 | 6, 8, 9, 12 | owner/halo compose and eviction tests |
| 9 | 9, 12, 13 | allocation report and stress record |
| 10 | 2, 9, 13 | pipeline budgets, shader grep/review |
| 11 | 4–13 | cold hashes, teleport high-water, cleanup record |
| 12 | 13 | manual measurement record |

## Open questions

None block implementation. The 256 MiB default is an engineering starting floor, not a
claim that it is optimal. Task 13 records its result; changing it later is tuning, not a
wire or sampler change. The owner still decides whether the measured result justifies
shipping streaming enabled by default after this slice lands.
