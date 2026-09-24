# SH Probe Streaming — Per-Cluster Residency

> Slice 3 shipped in PR #516. Parent: `context/plans/in-progress/sh-probe-streaming/`.
> GPU frame-time, seam, and growth-copy reads remain `not-yet-evaluable`; see
> `execution.md` and `measurements/sh-probe-streaming/cluster-residency.md`.

## Goal

Keep only the SH payload needed by visible and near-visible cell clusters in a
renderer-owned GPU pool. Load and prepare cluster payloads off the frame path,
install one generation-matched batch before SH compose, and evict cold clusters
without lighting holes, boundary darkening, or duplicate light accumulation.

## Scope

**In:** a synchronous no-eviction proof gate; one-file cluster payloads for ids
27/34/35/41/45; visible-cell planning; two-hop prefetch; time-based hysteresis;
bounded async reads and decode;
renderer-owned fixed-budget pools with controlled growth for non-evictable working
sets; atomic install/compose/sample promotion; LRU eviction; baked-owner pinning;
ambient-floor miss fallback; reload cancellation; diagnostics and manual performance
evidence.

**Out:** authored streaming hints or seam presentation (Slice 4); geometry, BVH,
lightmap, SDF, fog, acoustic, or texture streaming; networked residency state;
billboard direct-scatter streaming (ids 47/48, retained whole-resident here);
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
optional `ClusterShPayloads` id 50, container-entry v1, internal epoch 1. Id 50 is an
independently readable cluster-major copy of ids 27/34/35/41/45, emitted only when
id 49 and base id 34 are present. Existing
ids 27/34/35/41/45/47/48 remain byte-identical and preserve legacy whole-load behavior.
This duplication is deliberate: current v11/v4 BC6H bytes are encoded as global atlas
block rows, so a 6x6 logical tile shares 4x4 compression blocks with adjacent tiles and
cannot be gathered into an arbitrary residency slot without decoding/re-encoding or
retaining the whole source atlas. The sparse companions are likewise monolithic codecs.

Branch selection is exact. The loader validates id 50 structurally before consulting the
developer/test mode. Id 50 without id 49 rejects. Malformed id 50 rejects. Id 50 whose
streamed-resource inventory (the present 27/34/35/41/45 subset), source versions,
cluster counts, or index do not match id 49 rejects.
That subset must contain id 34 and its records must exactly match present
container sections; id 50 without id 34 rejects as `ClusterShPayloadsSourceMismatch`
before mode selection.
`POSTRETRO_SH_STREAMING=off` bypasses streaming only after a valid id 49 + id 50 pair has
passed that validation. Unknown id 50 remains skippable to older loaders, preserving one
PRL container-v4 artifact. Id 50 is built after finalized section selection, uncached,
from borrowed final outputs. The encoder receives the finalized presence inventory plus
the borrowed pre-BC6H packed id 34/id 35 sources; encoded global BC6H bytes alone cannot
create independent 8x8 cells. It changes no bake result or existing cache key/epoch.

| Sections present | Id-50 validation result | `POSTRETRO_SH_STREAMING` | Runtime path |
|---|---|---|---|
| No 49, no 50 | n/a | any | legacy whole-load |
| 49 only | n/a | any | legacy whole-load |
| 50 only | invalid pair | any, including `off` | reject `ClusterShPayloads*` |
| 49 + 50, no id 34 | invalid source inventory | any, including `off` | reject `ClusterShPayloadsSourceMismatch` |
| 49 + malformed/mismatched 50 | invalid id 50 | any, including `off` | reject `ClusterShPayloads*` |
| 49 + valid 50 | valid | `off` | legacy whole-load |
| 49 + valid 50 | valid | unset | bounded async streaming |
| 49 + valid 50 | valid | `sync-proof` | synchronous no-eviction proof |
| 49 + valid 50 | valid | `async` | bounded async streaming |

Runtime has explicit developer/test controls: `POSTRETRO_SH_STREAMING=off`
forces legacy whole-load, `sync-proof` selects the thin synchronous/no-eviction path,
and `async` selects the complete policy. A valid id-50 PRL defaults to `async`; a PRL
without id 50 ignores the variable and stays legacy. The variable is a developer/test
gate, not a player option or stable content surface, and cannot suppress id-50 structural,
version, inventory, or index validation failures.

The loader does not retain whole bodies for streamed ids 27/34/35/41/45 in streaming
mode. Ids 47/48 remain whole-resident with their current global-probe addressing,
compose path, and sampler binding. It opens the PRL
once, validates the container plus ids 49/50 metadata, and stores that same `Arc<File>` in
an immutable `ShStreamManifest` with a bounded chunk index, cluster graph, content tag,
and global animation/selection metadata. The path is retained only for diagnostics; jobs
never reopen it. Chunk jobs use positional `FileExt` reads (`read_at` on Unix,
`seek_read` on Windows), so concurrent workers do not share or mutate a seek cursor. The
old whole-section bodies for streamed ids remain populated only in legacy mode.
`LevelWorld` exposes an explicit legacy-vs-streaming SH storage enum plus accessors
for shared global metadata,
so startup, renderer geometry, and capture code do not branch on raw section presence.
`level_world_to_geometry`, `LevelGeometry`, and capture preparation must carry streaming
manifests without requiring whole bodies for streamed ids in stream mode.

Streaming stale-work identity is deliberately independent of co-op/network content
identity. Each loaded streaming session receives a nonzero, monotonically increasing,
checked `u64` residency generation; it never resets or wraps, and exhaustion is a fatal
load error. Its BLAKE3 content tag covers a fixed domain separator, the exact container-v4
identity fields (magic, version, section count, and every ordered section-table
`section_id/offset/size/version` tuple), the exact id-49 body, and the validated id-50
header/source/index bytes. The index's per-chunk hashes bind every payload chunk. A
completion may install only when its generation and content tag match the live session,
its cluster remains in the current target set, and the freshly read bytes match that
cluster's index hash. Cooperative cancellation limits wasted work; this tuple is the
authoritative stale-result gate.

### Tile and sparse payload mechanics

Each streamed base tile keeps the v11/v4 logical 6x6 texels and border semantics but is
stored alone in an 8x8 physical cell. Edge texels dilate through the two padding
rows/columns before encoding. Production BC6H chunks encode each isolated cell to four
4x4 blocks; uncompressed-debug chunks retain exact `Rgba16Float` bits in the same 8x8
cell. The sample-side `ShGridInfo` reuses its existing `_pad2` word for physical
tile stride; the compose-side 64-byte `GridDims` prefix stays byte-identical and
its dynamic tail carries that stride. Logical tile dimension/border and all sample
taps stay unchanged.
Task 9 renames/populates `_pad2` in the shared 96-byte CPU grid packer and all
five WGSL `ShGridInfo` mirrors (forward, billboard, fog, skinned mesh,
kinematic brush), uses
physical stride in `sh_sample.wgsl::probe_slot_location`, and uses the dynamic
tail stride in all three compose `slot_tile_origin` helpers. Assert legacy=6,
streamed=8, dummy=1 without changing the 96-byte ABI.
Whole-load mode uses stride 6. Stream mode uses stride 8. No binding or per-fragment
read is added.

The compiler resolves id-49 dense ranges through the canonical v11 stored-node prefix
sum. One cluster chunk contains every unique stored node its dense closure references,
with id-34 and present id-35 tiles in identical local-slot order. Per-probe patches carry
global dense index, depth moments, and a local-slot indirection word. The renderer
resolves the canonical node owner's allocated pool slot for compose indirection;
the sampled depth-moment texture remains invalid until promotion after compose.
The wire word uses the existing valid/level/scale bits, while its slot bits are
the node's base-tile rank in this chunk's sorted, tile-expanded unique stored-node
closure. Each node occupies its canonical contiguous `stored_tile_count` tiles:
L0 contributes one per valid probe, L1 contributes eight corner tiles addressed
as base plus corner, and L2 contributes one mean tile; scaled nodes reuse their
origin node's base. Decode derives the
global stored-node prefix id from the dense index and retained id-34 metadata,
validates the wire level/scale/local rank, then resolves the canonical node
owner's tile-expanded base rank. Install rewrites only slot bits to that owner's
live pool base plus base rank. Malformed ranks/levels/scales reject rather than
alias another cluster's slot. Test scaled-L1 round-trip and base-plus-corner
addressing.

Sparse ids 27/41/45 are sliced by final affinity-cell rows. A chunk stores owned rows
and the halo rows needed by its directory coverage. Only the baked owner row may enter
compose CSR; halo rows establish an owner dependency and provide an independently
loadable copy, never another accumulation.
For overlapping dense closures, derive ownership without another wire field. Id 49's
dense ranges and id 34's retained validity/level/scale records determine the canonical
stored-node prefix and each cluster's sorted unique stored-node closure. The lowest
cluster id containing a stored node owns its pool slot; the lowest cluster id containing
a global dense probe owns that probe's depth-moment/indirection patch. A patch owner
maps its local stored-node id to the canonical node owner's slot using the node's
global prefix id and its rank in that owner's sorted closure. These owners may differ.
Every chunk still carries the complete independently decodable local closure, but a
non-owner's duplicate tiles and patches never allocate a second live slot, overwrite
shared words, or add light again. Index `stored_tile_count` and `decoded_bytes` count
the complete chunk; `requested_resident_bytes` and logical occupancy charge only
canonical-writer tiles/rows. Pin the transitive node/patch writer closure as well as
sparse-row owners before a dependent cluster can become sampleable. The lower-id
canonical-writer edges are acyclic; validate that every referenced writer and local
rank resolve from id 49 and retained id-34 metadata.
Ids 27/41/45 keep `valid_probe_masks` and `cell_levels` as always-resident metadata
parallel to their global affinity rows; row payloads reference those metadata rows and do
not duplicate masks or levels. Ids 47/48 retain their existing whole-resident
representation and contribute no id-50 source records or chunks. Global
animation descriptors, samples, descriptor-index maps, light-selection maps,
grid metadata, valid masks, cell levels, and CSR offset tables stay in an
always-resident metadata floor. The metadata-only id-34 projection retains grid
origin/cell size/dimensions, atlas format/geometry, every probe's validity,
depth moments, density level and node scale, animation descriptors, and
`slot_for_map_light`, but not `compact_atlas`. Ids 27/41/45 retain affinity
factor/dimensions, tile geometry, `valid_probe_masks`, `cell_levels`,
`affinity_offsets`, `affinity_lights`, and section-specific descriptor/selection
maps, but not `delta_subblocks`; id 35 retains its geometry/format header
without its atlas. Read exact codec-defined metadata ranges positionally. Run
the same id-34+47 and id-45+48 cross-validations as legacy using these
projections and whole-resident 47/48 bodies. Missing CSR rows encode equal
start/end offsets.
Chunk-local payloads carry no wgpu types.
The 47/48 whole-resident allocation is a separately named fixed GPU charge, not
misreported as small metadata or logical streamed occupancy.

### Residency policy

The planner maps the current `VisibleCells` through id 49. `Culled` uses those clusters;
`DrawAll` makes every cluster visible. Prefetch is the unique two-hop cluster set over
runtime portal adjacency. Visible clusters are required immediately. Prefetch-only
clusters remain targets while covered. Any cluster that leaves both sets remains a
hysteresis target for 2.0 seconds of monotonic render time. This duration is independent
of refresh rate.

Default requested SH GPU floor is 256 MiB on native desktop adapters, including macOS.
That GPU floor charges fixed GPU metadata, whole-resident ids 47/48, and active
physical pool capacity; logical resident occupancy is a sub-ledger of the active
pools, not another charge. CPU encoded,
decoding, and ready bytes are separate checked phase ledgers and never contribute to the
GPU floor. An absent family gets no pool. The renderer sizes each present family pool to
at least its largest single-cluster requirement, then distributes the remaining floor in
proportion to finalized whole-level streamed-family bytes. If fixed GPU metadata,
whole-resident ids 47/48, and the sum of present-family largest minima exceeds
256 MiB, the effective floor is that larger checked
sum. If every present family reaches its whole-level capacity first, the exact
physical effective floor may be below the 256 MiB request; the controller must
accept that renderer-reported figure when it covers all mandatory charges.
The effective floor is reported. Pools use deterministic first-fit free ranges with
coalescing. They never compact live slots.
Ids 34 and optional 35 form one dense slot-pool group. Allocate/free the same
slot ranges in every present member, with identical 8x8 width/height/layer
geometry, one growth/retirement transaction, and GPU-to-GPU retained-range
copies. A static-only sample path may alias its base view instead of allocating
a composed target. None of these active resources is fixed metadata.

| Dense group member | Presence | Format and role |
|---|---|---|
| Indirect base | id 34 present (mandatory for id 50) | id-34 source format; isolated-cell upload |
| Indirect composed/sample | when indirect compose allocates output | RGBA16F; same slots |
| Direct base | id 35 present | id-35 source format; isolated-cell upload |
| Direct composed/sample | when direct/animated-direct compose allocates output | RGBA16F; same slots |

Publish the shared probe word only when every present dense resource and sparse
compose family has completed. Charge each actually allocated member separately;
never let independent family allocation assign a different slot to id 35.

Pool layout is deterministic from finalized metadata and adapter limits. For the
coupled dense group, a slot costs one isolated 8x8 cell in every present member
(BC6H 64 bytes or RGBA16F 512 bytes per member); divide its proportional floor
share by that checked combined slot cost, round up, and take the maximum with
its largest single-cluster canonical-writer slot requirement. Let `S` be that
minimum slot count and `M = floor(max_texture_dimension_2d / 8)`. Fix
`tiles_per_row = min(M, ceil_sqrt(S))`, `rows_per_layer = min(M,
ceil_div(S, tiles_per_row))`, and `tiles_per_layer = tiles_per_row * rows_per_layer`;
initial layers are `ceil_div(S, tiles_per_layer)`. Width and height are these
fixed row dimensions times 8; growth doubles active layer count (or increases
to the exact non-evictable minimum if larger) without changing either dimension.
Reject zero/overflow/adapter-limit results before allocation. Sparse-family
entry/tile capacities similarly round their proportional byte shares up to
their element alignment and largest owned-row requirement, subject to adapter
buffer limits; grow by appending offset-stable capacity. Charge actual rounded
physical capacity and report unused capacity separately from logical occupancy.

For ids 27/41/45, keep the per-affinity validity masks, cell levels, animation and
selection maps, and row ownership map resident. At the existing `affinity_offsets`
binding, use two `u32` words `(entry_start, entry_end)` per affinity row instead of
the legacy prefix array; the legacy path builds the equivalent pairs from its prefix
at install. These are global per-family entry-pool indices shared by the existing
`affinity_lights` buffer and the per-entry compaction-offset tail. The three
streamed compose shaders read `offsets[cell * 2]` and `offsets[cell * 2 + 1]`;
billboard compose keeps its unchanged prefix CSR. A missing or halo-only row
has `(0,0)`. Owned rows allocate first-fit
ranges in per-family entry metadata and f16 tile buffers; the existing light-index
and compaction-offset bindings point at those pools. Patch a row's pair only after
its entries and tile offsets are uploaded, and reset it to `(0,0)` before either
range is reused. Chunk kind-3 `first_entry` and `first_tile_f16` values are
block-local entry and f16-half offsets, never GPU addresses. Validate ordered,
non-overlapping local ranges; install translates each owned row to its entry-pool
range and each entry to a tile-pool f16-half offset, then packs f16 halves into
the existing u32 storage carrier. Test odd-half offsets, cross-row reuse, and
eviction/reinstall in all three families. Non-owner halo copies consume decoded
CPU bytes while ready but no GPU entry/tile capacity or logical occupancy after
install. Recompose invalidated
rows so no stale offset is reachable. This changes compose-side buffer contents and
indexing, not binding numbers/counts or fragment sampling.

Eviction first considers departed, non-hysteresis clusters, then prefetch-only clusters.
Within a tier use `(last_visible_time, last_target_time, cluster_id)` ascending. Never
evict visible clusters, a cluster installed during the current drain, or an owner needed
by any installed/sampleable halo cluster. Owner-pin closure makes owner clusters targets
before queueing. Install drain sorts and installs owners before dependent halo clusters.
A halo cluster cannot be installed, promoted, or sampled until every baked owner for its
halo ranges is installed, composed, and sampleable. Missing owners defer the halo instead
of transferring ownership or composing halo rows. A pressure-evicted prefetch cluster is
suppressed until the two-hop horizon changes or pressure clears. If non-evictable demand
exceeds a pool, grow that family geometrically at the drain boundary. Growth allocates a
larger active generation, copies retained ranges GPU-to-GPU without relocation-visible
gaps, and retires the old generation only after submitted work can no longer reference
it. Atlas width, height, tiles-per-row, and tiles-per-layer stay fixed for a session;
atlas growth appends array layers, so every retained slot maps to the same texels before
and after copying. Buffer growth preserves flat byte offsets. If adapter limits prevent
append-only growth, the install fails with a named capacity error rather than silently
changing slot geometry. A family may have at most one active and one retiring
generation; no second growth begins until retirement completes. The non-evictable
bytes above the effective floor are
reported as overshoot, while active-plus-retiring capacity is reported separately as the
temporary replacement peak. Growth is exceptional; it preserves correctness instead of
dropping visible lighting, edge-logs one onset, and never requires retained decoded
chunks, file I/O, or decode on the render thread.

Async owns exactly four permits. A permit spans encoded read, decode, `Ready`, and final
install or drop; a ready completion retains its permit, so queued ready work cannot admit
unbounded replacement jobs. Encoded, decoding, and ready phase bytes use checked `u64`
charges with independent current/high-water rows. Payload ownership moves between phases:
no full-payload clones exist, and successful install retains no full decoded host copy.
The renderer installs at most two clusters per frame. Ready installs sort visible first,
then prefetch, then hysteresis, with cluster id as the tie-break. A completion must match
the live generation, content tag, current target membership, and its per-chunk hash.
Same-level retreat and cross-level stale completions drop, release all CPU phase bytes,
and return the permit without charging logical occupancy.

### Atomic frame lifecycle and miss behavior

The renderer drains residency once, at the start of `record_scene_passes`, before direct,
billboard, and indirect SH compose. Drain first invalidates evicted probes in the sampled
depth-moment texture, then installs ready base/delta data and compose-side
indirection into pool slots. The indirect and direct streaming compose families
dispatch by affinity-cell-row ranges because current compose entry points are
affinity-brick based.
Dirty rows derive from the installed cluster's dense/stored-node closure and sparse CSR
rows. Base-only and static-only copy-through still coalesces to affected affinity rows;
it never dispatches arbitrary dense indices directly. Indirect compose covers ids 34 and
27. Direct compose covers ids 35, 41, and 45. Existing billboard compose continues
to cover whole-resident ids 47/48 without streaming dirty-range changes.
Compose uses the installed set.
World forward, billboards, fog, skinned meshes, and kinematic brushes sample the prior
installed-and-composed atlases. SDF consumes only sample-side depth moments.

A newly installed cluster stays `InstalledUncomposed` for that frame. Its sample-side
depth-moment indirection remains zero while all applicable compose passes write its slots.
At the next frame's drain, queue ordering makes the prior compose complete before its
sample-side words are published, and the cluster becomes `Sampleable`. A completion that
arrives after the drain waits. Eviction invalidates sample words before slot reuse.

The all-zero indirection word is the general miss placeholder. Existing SH sampling then
drops those corners and renormalizes survivors; if none survive it returns the ambient
floor. Whole-resident billboard scatter follows the same SH-validity weights, so a
missing SH cluster does not expose scatter there. Slice 4 owns authored seam gates;
this slice supplies the conservative placeholder everywhere.

Compose dispatches only dirty ranges. Reuse each pass's existing grid uniform binding
number/count, but its bind-group-layout entry changes from `has_dynamic_offset=false` to
`true`; current BGL entries are false. Indirect, direct, and animated-direct
dynamic-offset records start with the existing 64-byte `GridDims` layout produced
by `build_compose_grid_bytes`:
`grid_dimensions[3]`, `tile_dimension`, `atlas_dimensions[2]`, `tile_border`,
`delta_probe_f16_stride`, `affinity_dims[3]`, `atlas_tiles_per_row`,
`tiles_per_layer`, `atlas_layer_count`, and the two compact-atlas tail words. Append
`u32 physical_tile_stride`, `u32 range_start`, `u32 range_count`, and `u32 padding`
for an 80-byte record. `physical_tile_stride` is 6 on the legacy path and 8 in
streaming mode. Record stride is
`align_up(80, adapter.limits.min_uniform_buffer_offset_alignment)`. Every
dynamic offset and range is checked to fit `u32`, the record slice must stay within the
uniform buffer, and each corresponding BGL entry sets `min_binding_size = 80`.
Billboard scatter compose keeps its existing 32-byte `ScatterGrid` at binding 2,
its existing non-dynamic BGL, and its existing dispatch behavior. Each
distinct existing `GridDims` bind-group layout receives its matching dynamic record
without changing binding number or bind-group entry count. Dispatch one-dimensional
workgroups over `range_count`. `range_start/count` are flattened x-fastest
affinity-brick indices (id-49 `AffinityCell`), with one workgroup per index;
WGSL converts `range_start + workgroup_id.x` back to affinity xyz before the
existing brick logic. This prevents compose cost from scaling with the whole map when only a few clusters
change. Add layout/budget tests covering descriptor dynamic flags, record size/stride,
offsets, min binding size, adapter-limit bounds, and binding number/count preservation.

## Acceptance criteria

- [ ] **AC1 — Preserved contracts:** existing ids 27/34/35/41/45/47/48, id 49 v1,
      container v4, binding numbers/counts, shader sample stencil, and cache epochs remain
      unchanged. Legacy PRLs without id 50 load and render through the prior whole path.
      Invalid id 50 rejects before mode selection, including when the developer/test mode
      is `off`.
- [ ] **AC2 — Payload wire:** id 50 round-trips deterministically; its source-version and
      inventory cross-check rejects drift; each cluster chunk is independently seekable,
      hash-checked, bounded before allocation, and contains the exact v11/v4 node closure,
      deterministic 8x8 local atlas layout and owned/halo sparse
      rows named by id 49. Header validation, decoded-byte formulas, per-family logical
      resident bytes, requested-resident-byte aggregation, and all three streamed sparse families
      are covered by tests.
- [ ] **AC3 — Thin proof:** a hard-gated synchronous no-eviction mode targets clusters
      from real visible cells, installs zero/one/many clusters generation-safely, composes
      all streamed families, and exposes each cluster only on the next frame. The
      developer/test `off` escape preserves legacy output.
- [ ] **AC4 — Miss behavior:** a visible but unavailable cluster samples valid resident
      neighbors and otherwise ambient floor. No uninitialized/stale atlas slot or lighting
      hole appears in CPU reference tests or manual seam inspection.
- [ ] **AC5 — Async bounds:** production streaming performs positional file read,
      checksum, decode, and CPU preparation off the Input → Game → Audio → Render →
      Present path. Synthetic 250 ms latency does not stall frame submission. Exactly
      four permits span read through ready/install/drop, ready work retains its permit,
      at most two installs occur per frame, and visible work cannot starve behind prefetch.
- [ ] **AC6 — Atomic generations:** nonzero checked residency generations never reset or
      wrap. Same-level retreat completions and cross-level stale completions both drop
      unless generation, content tag, target, and chunk hash all match. Reload/unload
      cancels or drains workers, clears targets, invalidates sampled words, releases GPU
      pools, and never installs prior-level bytes.
- [ ] **AC7 — Policy:** visible plus two-hop prefetch drive targets; 2.0-second hysteresis
      prevents adjacent-frame doorway thrash at 30/60/144 Hz. Departed-first LRU eviction,
      persistent prefetch suppression, and just-installed protection follow the pinned key.
- [ ] **AC8 — Ownership and continuity:** every resident halo cluster retains its baked
      owner. Only canonical dense writers patch shared words, and only owned sparse
      rows accumulate. Owner install/promotion precedes every
      dependent halo, missing owners defer halo install, owner eviction cannot darken a
      resident boundary, and L0/L1/L2 reconstruction stays self-consistent across cluster
      seams.
- [ ] **AC9 — Budget:** the default/effective GPU floor charges only fixed GPU metadata
      plus whole-resident 47/48 capacity and active physical pool capacity; logical
      occupancy is a sub-ledger. Reports separate
      active and retiring capacity, replacement peak, non-evictable overshoot, and checked
      encoded/decoding/ready current and high-water bytes without double-count. A
      nondegenerate large fixture whose whole-load requested SH bytes exceed the
      effective floor remains below whole-load requested SH allocation; growth
      is append-preserving GPU-to-GPU, permits at most one retiring generation per family,
      and logs once per overshoot onset.
- [ ] **AC10 — Sampler and compose:** forward sampled-texture/BGL budget tests stay green;
      grep/review finds no fragment-stage residency locate read. Dirty-range compose uses
      existing binding numbers/counts, dynamic-offset grid records, and affinity-row
      dispatch for indirect and direct streaming families; billboard scatter retains
      its unchanged whole-resident compose path. Streaming compose does not dispatch over
      unrelated whole-map dense or affinity cells, nor arbitrary dense indices.
- [ ] **AC11 — Determinism and resource lifetime:** two cold worker-count variants emit
      identical id 49/id 50 and unchanged legacy section bodies. Teleport stress proves
      permit count and every CPU phase high-water stay bounded, payload ownership moves
      without full clones, installed clusters retain no full host payload, and cleanup
      removes generated PRLs, reports, caches, and crate-scoped build artifacts when disk
      is low.
- [ ] **AC12 — Manual finding:** record whole-load versus streamed requested SH bytes and
      frame-time windows. Prefer the GTX 1660 Super with the largest map that boots; also
      record a mid-sized map on this MacBook Pro. Failed adapters/full-map runs are
      `not-yet-evaluable`, not negative evidence or a hard rollout gate. Record seam/pop
      inspection separately and tune the engineering floor when evidence exists.

## Boundary inventory

No JS, TS, Luau, FGD, network, or player-option boundary is added.

| Meaning | Proposed Rust owner | Wire / runtime |
|---|---|---|
| Stream payload | `level-format::cluster_sh_payloads::ClusterShPayloadsSection` | PRL id 50; container-entry v1; epoch 1 |
| Manifest | `level-loader::sh_stream::ShStreamManifest` | metadata/index + validated `Arc<File>`; diagnostic path + streaming content tag |
| Prepared chunk | `level-loader::sh_stream::PreparedShCluster` | plain CPU bytes; no wgpu |
| Drain handoff | `level-loader::sh_stream::{ShDrainBatch, ShDrainOutcome}` | generation/tag, target reset/deltas, evictions, owned ready chunks, accepted/dropped ids, owned deferred chunks; no wgpu |
| Planner | `postretro::sh_streaming::ShResidencyController` | session-local; never networked |
| Renderer state | `renderer::ShResidencyState` | nonzero monotonic residency generation + target/install/sample sets and GPU pools |
| Budget | internal desktop default | 256 MiB native desktop requested SH GPU floor; no CLI/KVP in this slice |
| Timing | monotonic render seconds | prefetch depth 2; hysteresis 2.0 s |
| Caps | internal constants | 4 lifecycle permits; 2 installs/frame; active + at most 1 retiring pool generation/family |
| Mode gate | `POSTRETRO_SH_STREAMING` | `off`, `sync-proof`, `async`; validation precedes mode; valid id 49 + id 50 defaults to `async` |

## Wire format

All id-50 scalar fields are unsigned little-endian integers. No `f32` or `f64` scalars
occur. `f16` payload values are little-endian IEEE binary16 encoded as `u16`. Lengths,
offsets, `decoded_bytes`, `requested_resident_bytes`, all cluster counts, and all block
lengths are checked in `u64` against id 49/source metadata and the fixed record/format
formulas before allocation, then converted to `usize` only after file bounds pass. No
padding, implicit alignment, platform layout, sentinel offset, unnamed policy maximum, or
trailing bytes. The section is header + source table + fixed cluster index + payload blob.

The streaming content tag is BLAKE3 over, in order: the ASCII domain bytes
`postretro.sh-stream.v1\0`; PRL magic; little-endian container version and section count;
every ordered container entry encoded as little-endian `u32 section_id`, `u64 offset`,
`u64 size`, `u16 version`; the exact id-49 body; and the exact validated id-50 header,
source table, and cluster index bytes. It does not hash the diagnostic path or reuse the
co-op/network content identity. The index's 32-byte chunk hashes transitively bind the
payload blob, and every completion rechecks its selected chunk before decode/install.

**Header (72 bytes):** `u32 epoch=1`, `cluster_count`, `source_count`, `flags=0`,
`grid_dimensions[3]`, `affinity_dimensions[3]`, `tile_dimension=6`, `tile_border=1`,
`physical_tile_stride=8`, `reserved=0`, `u64 payload_bytes`, `u64 reserved=0`.
Validate the complete header and source table before allocating any payload buffer.

**Source record (16 bytes):** `u32 section_id`, `u32 internal_version`, `u32 kind`,
`u32 reserved=0`. Records are unique and sorted by section id. Kinds are 0 dense-base
atlas (34/35) and 1 sparse-affinity (27/41/45). Source records must exactly match
the present streamed-resource subset of id 49; ids 47/48 never appear in id 50.
Known internal versions are 27 v6, 34 v11, 35 v4, 41 v4, and 45 v4. They are copied
from the current source codec and cross-checked at load, not inferred from id 50.

**Cluster index record (80 bytes):** `u64 payload_offset` relative to payload blob,
`u64 payload_len`, `u64 decoded_bytes`, `u64 requested_resident_bytes`,
`u32 stored_tile_count`, `u32 dense_patch_count`, `u32 affinity_patch_count`,
`u32 flags=0`, then `blake3[32]` over the exact chunk bytes. Records are implicit cluster
id order and must consume non-overlapping, ascending payload ranges exactly. Empty
clusters have zero blocks, zero counts, zero length, the BLAKE3 empty hash, and no payload
bytes. `decoded_bytes` equals the checked sum of every decoded block body using the
formulas below. Per-family logical resident bytes are checked separately, then summed into
`requested_resident_bytes`; fixed metadata is not included in this per-cluster field.

**Chunk header (16 bytes):** `u32 chunk_version=1`, `u32 cluster_id`, `u32 block_count`,
`u32 reserved=0`. **Block record (32 bytes):** `u32 section_id`, `u32 block_kind`,
`u32 element_count`, `u32 flags`, `u64 offset` relative to chunk start, `u64 length`.
Block records are sorted by `(section_id, block_kind)`, unique, non-overlapping, and point
after the complete block table. All block flags and reserved fields are zero;
unknown kinds, wrong source/kind pairs, duplicates, gaps, and trailing bytes reject.
For a nonempty valid dense closure, exactly one id-34/kind-0 probe-patch block and
one id-34/kind-1 atlas block are required, plus one id-35/kind-1 atlas block if
id 35 is present; id 35 never duplicates the shared patch block. Each present
sparse source has exactly one kind-3 block when its id-49 owned/halo row coverage
is nonempty and no block otherwise. No other blocks are legal. Empty clusters have
the zero-length index form and no chunk header. Required block kinds:

| Kind | Body |
|---|---|
| 0 probe patches | `element_count × 16`: global dense index, existing valid/level/scale word with chunk-local sorted-closure slot rank, mean-distance f16, mean-square f16, reserved u32 |
| 1 isolated atlas | u32 format (1 BC6H, 0 RGBA16F), u32 local slot count, u32 width, u32 height, u32 layers, then tagged layer-major bytes; dimensions derive from local slots |
| 3 sparse rows for ids 27/41/45 | u32 row count, u32 entry count, u32 tile-f16 count, u32 reserved; row records `4xu32 = 16B` as `(global affinity index, first entry, entry count, role)`; entry records `4xu32 = 16B` as `(light/descriptor index, first tile-f16, tile-f16 count, reserved=0)`; then raw f16 tiles |

The kind-3 entry's first word is the source section's `affinity_lights` value,
not a universal descriptor id: id 27 indexes its animation-descriptor map,
id 41 indexes `EntityShadowLights` selection order, and id 45 indexes its own
animation-descriptor map. Preserve each namespace exactly and test all three.

Role is 1 owned or 2 halo, matching id 49. Base/direct atlas blocks share identical
local slot counts and geometry. Their deterministic local layout is exactly
`irradiance_atlas_array_layout([local_slot_count, 1, 1], 8, MAX_SH_ATLAS_DIMENSION)`;
Task 5 moves/exports the compiler-private `MAX_SH_ATLAS_DIMENSION` value into
level-format as the single format-owned cap and Task 6 makes the compiler import it;
no numeric value changes.
the block's width, height, layers, tiles-per-layer, and tiles-per-row must equal the
derived layout. Isolated atlas payload length is
`layers * (width / 4) * (height / 4) * 16` for BC6H with 4-aligned dimensions, and
`layers * width * height * 8` for RGBA16F. Pack each logical 6x6 tile at the origin of
its 8x8 physical cell, dilate the right and bottom two texels before encoding, store
cells row-major inside each layer, and store bytes layer-major. Probe patch bytes are
`dense_patch_count * 16`. Sparse
27/41/45 row bytes are `16 + row_count * 16 + entry_count * 16 + tile_f16_count * 2`,
with tile counts derived from always-resident `valid_probe_masks` and `cell_levels`.
Per-family logical resident bytes charge only canonical-writer id-34/id-35 tiles
and owned id-27/41/45 sparse f16 tile and pooled entry bytes; duplicate dense
tiles, duplicate patches, and halo rows are decoded but consume no installed GPU
capacity. Derive this from id 49 plus retained id-34 metadata, not a chunk claim.
`requested_resident_bytes` is the checked sum for the chunk's present
families. Dense indices are strictly ascending. Sparse rows are strictly ascending and
entries retain their source CSR order. Blocks are present only when the cluster has
cluster-local elements for that family. Counts, payload lengths, node closure, owner, and
halo status validate against ids 49 and source metadata.

## Runtime states and failure behavior

| State | Sample-side word | Budget | Exit |
|---|---|---|---|
| `Absent` | invalid | none | target queues load |
| `Queued` | invalid | one permit + current encoded/decoding phase bytes | completion or cancellation |
| `Ready` | invalid | same permit + ready bytes | drain installs or drops |
| `InstalledUncomposed` | invalid | logical occupancy within active GPU capacity; no full host payload | all applicable compose dispatches encoded |
| `Sampleable` | valid | logical occupancy within active GPU capacity; no full host payload | eviction or level teardown |
| `Failed` | invalid | none | horizon change retries once; repeated same hash stays failed |

Failure identity is `(residency_generation, content_tag, cluster_id, chunk_hash)`.
Keep one retry credit per failure identity. A change in the visible/two-hop horizon
revision after the failed cluster has left and re-entered the target set spends that
credit once; repeated failure of the same identity stays `Failed` for the session.
A new generation or content tag creates a new identity and resets the credit.
Target removal alone does not retry, and simple adjacent-frame updates that leave
the horizon unchanged do not retry.

| Failure | Behavior |
|---|---|
| Structural id-50/version/inventory/index error | reject level with named `ClusterShPayloads*` error |
| Chunk range/size exceeds index or checked wire/source formulas | reject before allocation |
| Runtime positional-read/hash/decode failure | warn once per cluster/hash, keep miss fallback, mark failed |
| Completion not targeted | drop before renderer; release phase bytes and permit; no logical occupancy charge |
| Generation/content-tag mismatch | drop before renderer even if cluster id is targeted; release phase bytes and permit |
| Residency generation exhaustion | reject the new streaming session; never reset or wrap |
| Chunk hash mismatch after positional read | named hash failure; release phase bytes and permit; never install |
| Pool fragmentation with enough total free bytes | evict eligible ranges, coalesce, retry; grow only when remaining demand is non-evictable |
| Growth while prior generation is retiring | defer the second growth/install until retirement completes; never allocate a third generation |
| Halo completion without every baked owner installed, composed, and sampleable | defer halo install/promotion; keep ownership with baked owner |
| Device loss/submit failure | existing renderer fatal path; never promote sampleability |
| DrawAll or non-evictable set over floor | grow, edge-log overshoot, preserve correctness |
| Id 49 only, or neither id 49 nor id 50 | legacy whole-load path; no partial streaming |
| Id 50 without id 49 | reject level with named `ClusterShPayloads*` error |

Named codec failures are `ClusterShPayloadsVersionMismatch`,
`ClusterShPayloadsInvalidData`, `ClusterShPayloadsSourceMismatch`,
`ClusterShPayloadsRangeOutOfBounds`, `ClusterShPayloadsSizeOverflow`, and
`ClusterShPayloadsHashMismatch`. Loader/worker errors wrap these names without replacing
the existing errors for source ids 27/34/35/41/45/47/48.

## Invariants

| Invariant | Established by | Threatened at | Verified by |
|---|---|---|---|
| Frame samples only generation/tag/target/hash-matched, composed data | Tasks 7–11 | positional read, completion drain, slot reuse, reload | AC3, AC4, AC6 |
| Physical light accumulates once | Tasks 6, 9, 12 | owner/halo duplication | AC2, AC8 |
| Boundary reconstruction keeps baked owner | Tasks 8, 9, 12 | owner eviction, halo install ordering | AC7, AC8 |
| Dirty compose dispatches by affinity rows only | Tasks 2, 9, 13 | dense-index dispatch shortcut, base-only copy-through | AC10 |
| Id-50 mode cannot mask invalid wire | Tasks 5, 7, 10 | developer/test `off` escape | AC1, AC2, AC3 |
| Sampler keeps bindings and fixed taps | Tasks 2, 9 | pool indirection | AC1, AC10 |
| I/O/decode stays off frame path | Task 11 | sync fallback leaking to production | AC5 |
| GPU floor, logical occupancy, and CPU phases are separate checked ledgers | Tasks 8, 9, 11, 12 | ready backlog, growth, retirement | AC5, AC9, AC11 |
| Growth preserves installed bytes with active + one retiring generation | Tasks 9, 12 | DrawAll, fragmentation, repeated growth | AC4, AC9 |
| One PRL; streaming stale tag stays separate from co-op identity | Tasks 5–8 | sidecar or identity reuse temptation | AC1, AC2, AC6 |
| Existing bake/cache output is unchanged | Tasks 4–7 | payload extraction | AC1, AC11 |

## Tasks

### Task 1: Split loader ownership before streaming

Behavior-preserving split of `prl.rs` and `prl_loader.rs`: move LevelWorld lighting data
and accessors into a focused module; move container inventory/section-read helpers out of
the production assembly function. Preserve feature gates, constructors, exports, optional
fallback order, and every current caller. Inventory all direct `LevelWorld` SH/global
selection consumers that must migrate later, explicitly including startup install,
`level_world_to_geometry`, `LevelGeometry`, capture preparation, and
`crates/postretro/src/scripting/frame_systems/mesh_render.rs` `entity_shadow_lights`.
No id-50 behavior.

### Task 2: Split renderer SH resources before pools

Behavior-preserving extraction from `sh_volume.rs`, `sh_compose.rs`,
`direct_sh_compose.rs`, `animated_direct_sh_compose.rs`, `renderer_render_frame.rs`, and
`renderer_shadow_passes.rs`: isolate atlas upload/allocation,
compose carrier construction, dispatch planning, and pre-scene SH orchestration. Preserve
all wgpu descriptors, BGL entries, dispatch conditions, diagnostics, and public APIs. No
residency behavior.
Inspect the install/call interfaces in `renderer_resources.rs`,
`renderer_types.rs`, `direct_sh_resources.rs`, and
`billboard_direct_scatter_compose.rs`; keep them stable in this split and defer
their residency-specific changes to Task 9.

### Task 3: Split application frame and capture seams

Extract visible-cell-to-render preparation from `main.rs` and repeated capture setup from
`capture/driver.rs` and `capture/prepared.rs` into focused modules. Preserve
Input → Game → Audio → Render → Present,
visibility results, capture output, and renderer call order. Keep the later
legacy-vs-streaming SH storage seam visible in capture preparation. No planner behavior.
Inventory both windowed/headless `main.rs` render sites, the shared
`renderer_render_frame.rs` entry point, and `renderer_capture.rs`; preserve
argument order and capture-preparation behavior at all of them.

### Task 4: Split compiler final-lighting publication

Extract final SH inventory, post-bake metadata stages, and pack descriptor construction
from oversized `pipeline.rs`, `pack.rs`, `pack/finalized_sections.rs`, and
`pack_output.rs`. Preserve stage order,
borrowed finalized-section presence, section order, one-payload-at-a-time serialization,
and cold/warm output. Introduce `FinalizedShPackSources<'a>` carrying borrowed
pre-BC6H `OctahedralShVolumeSection` and optional `DirectShVolumeSection` in
RGBA16F packed stored-slot form, alongside `FinalizedShEmissionView`; keep
these owned packed values alive through emission without cloning full atlases.
Generalize `PlannedSection` with a checked byte length and one-shot writer to
the staged PRL so a future id-50 spool can stream without a full `Vec<u8>`;
legacy sections retain their current one-shot byte encoders through a wrapper.
Do not change existing output or emit id 50 in this task.

### Task 5: Add id-50 codec and shared validator

Implement the exact wire above in level-format, register id 50, and add overflow,
ordering, range, checksum, source-version, resource-inventory, owner/halo, node-closure,
hostile-header-before-allocation, per-family byte-formula, sparse-family,
and empty/single-cluster tests. Expose metadata/index parsing separately from chunk decode
so loader startup never reads the payload blob.
The source table is exactly the present 27/34/35/41/45 subset with required
id 34; 47/48 never appear. Move/export the existing numeric
`MAX_SH_ATLAS_DIMENSION` cap from compiler-private `sh_bake.rs` into level-format,
and assert compiler/codec isolated-layout agreement without changing the cap.

### Task 6: Bake deterministic cluster chunks

Consume Task 4's `FinalizedShPackSources` borrowed packed indirect and optional
packed direct, plus its finalized presence inventory and id 49. Resolve dense ranges through
the v11 stored-node prefix, independently encode 8x8 physical cells from borrowed
pre-BC6H id 34/id 35 sources in the source irradiance format, slice sparse rows, retain
halo copies for ids 27/41/45, and append id 50 after id 49. Do not encode
47/48 in id 50; their legacy bodies remain byte-identical.
Spool one chunk at a time to a session-owned temporary file so hashes/indexes are known
before the section header is written; stream that spool into the staged PRL and remove it
on success or failure. Never retain the whole id-50 payload in RAM. Preserve every old
body/version/epoch. Add worker-count determinism using dedicated one-worker and
four-worker Rayon pools over the same fixture, plus legacy-body identity tests.

### Task 7: Load metadata without whole streamed SH bodies

Consume Tasks 1 and 5. Add bounded positional container reads and `ShStreamManifest`;
streaming mode decodes only the metadata-only projections and id-50 indexes for
streamed families,
while legacy mode remains byte-for-byte behavior-compatible. In streaming mode,
decode 47/48 as existing whole-resident companions; do not retain whole bodies for
27/34/35/41/45. Add the explicit legacy-vs-streaming SH storage enum,
content tag, cluster adjacency, and accessors for startup animation/selection metadata.
Migrate every direct `LevelWorld` SH/global-selection consumer to legacy/stream accessors:
startup, `level_world_to_geometry`, `LevelGeometry`, capture preparation, and
`crates/postretro/src/scripting/frame_systems/mesh_render.rs` `entity_shadow_lights`.
Make `LevelGeometry` carry an explicit legacy-body versus streaming-manifest
storage variant, so renderer installation never requires whole streamed bodies.
Add bounded container/table and section-metadata projection parsers in
level-format/level-loader, plus projection-compatible id-34+47/id-45+48
validation entry points. A fixture with a large atlas body must prove that the
streaming branch does not read that body during load.
Define `PreparedShCluster`, `ShDrainBatch`, and `ShDrainOutcome` in
`level-loader::sh_stream`, which both `postretro` and `renderer` already depend
on. The batch owns at most two ready chunks and carries the generation/content
tag, sorted/deduplicated target-add and target-remove deltas, and evictions.
On a new generation it carries one validated cluster-count-bounded target bitset
reset; renderer clears its old target set, applies that seed before any ready
admission, then retains the generation-tagged set across frames. No unchanged
whole target set is resent; each later delta is bounded by manifest cluster
count. The outcome names accepted/dropped ids and returns owned deferred chunks
so the app-side controller releases/retains permits and phase bytes exactly
once. No wgpu or app-controller type crosses this boundary. Tests cover legacy and streaming variants for each migrated boundary, including
id-34+47 and id-45+48 cross-validation without whole streamed bodies. Test
invalid-id-50 rejection before mode selection, including `off`. Add named
rejection and chunk read/decode APIs. Open and validate once, retain the exact `Arc<File>`
plus a diagnostic-only path, and implement cursor-independent positional `FileExt` reads
on Unix and Windows. Do not call the legacy whole-file `PrlContainer::open` in
streaming mode: parse the container table and all required non-streamed sections
through bounded positional reads on the retained `Arc<File>`, never constructing
a whole-PRL or whole streamed-section buffer. Run id-34+47 and id-45+48 pair
validation against the metadata projections. Hash the fixed domain, exact
container-v4 identity fields, exact id
49, and validated id-50 header/source/index; prove table/path replacement cannot redirect
a manifest and chunk hashes bind payload.

### Task 8: Build the pure residency planner and synchronous gate

Consume Task 7's exact `ShStreamManifest`, decoded chunk, metadata, and
`ShDrainBatch`/`ShDrainOutcome` contracts. Add
the state machine, cell→cluster map, two-hop horizon, 2.0-second hysteresis, target
membership, a process-monotonic nonzero checked `u64` residency generation, content-tag
matching, ready priority, owner-pin closure that makes owners targets, owner-before-halo
ready ordering, missing-owner deferral, suppression, LRU key, separate checked GPU
floor/logical-occupancy/CPU-phase charging from id 49/source metadata and fixed format
formulas, and four-permit/two-install caps as plain Rust. Generation exhaustion rejects
the next streaming session rather than resetting or wrapping. The hard-gated synchronous
source reads one chunk at target time and never evicts; unit tests prove visible drive,
owner-before-halo ordering, missing-owner deferral, and zero/one/many lifecycle
transitions before GPU work exists.
Use an injectable `GenerationClock` for pure overflow tests and a process-global
checked atomic implementation in production. Emit Task-7 `ShDrainBatch` and
consume `ShDrainOutcome`; the planner never exposes an app-owned type to renderer.

### Task 9: Add renderer pools and atomic compose lifecycle

Consume Tasks 2, 5, 7, and 8. Build renderer-owned atlas/buffer pools, sampled and compose
indirection mirrors, CSR offset patching, first-fit allocation, dirty-range
dynamic uniforms for streamed families, next-frame sample
promotion, miss invalidation, and requested-byte ledger rows. Dirty work is dispatched as
coalesced affinity-cell-row ranges for indirect ids 34 + 27 and direct ids
35 + 41 + 45, including base/static-only
copy-through. Derive affected rows from dense/stored-node closure for base-only sources;
never dispatch arbitrary dense indices directly. Keep every binding number/count and all
wgpu inside the renderer; same binding numbers use dynamic-offset descriptors and aligned
records with checked `u32` offsets/ranges. Keep the compose `GridDims` 64-byte
prefix fixed, put physical tile stride in its 80-byte dynamic tail, and reuse
`ShGridInfo._pad2` for sample-side stride without changing uniform byte size.
Billboard scatter retains its current whole-resident resources, 32-byte
`ScatterGrid`, and compose dispatch. Implement growth as append-preserving
GPU-to-GPU copy with one active and at most one retiring generation per family, deferring
further growth until retirement. Install moves decoded ownership into uploads and retains
no full host copy. Expose fixed metadata, active capacity, logical occupancy, retiring
capacity, and replacement peak separately.
The renderer accepts `ShDrainBatch` from level-loader at the one pre-compose
drain, rechecks generation/tag/target membership, and returns `ShDrainOutcome`
after install/defer/drop; it never imports the `postretro` controller.
Update the shared CPU grid packer, `sh_sample.wgsl`, and the forward, billboard,
fog, skinned-mesh, and kinematic-brush WGSL `ShGridInfo` mirrors for physical
stride. Expand legacy CSR prefixes into row pairs for id-27/id-41/id-45
compose, update all three shaders' pair indexing, and leave id-48 billboard
prefix CSR unchanged. Assert 96-byte sample ABI and 64-byte compose prefix.
Use one coupled id-34/id-35 slot allocator and growth transaction. Resolve every
probe patch through the canonical dense-probe writer and its stored-node writer's
slot; upload only canonical tiles. Represent each sparse affinity row with an
existing-binding `(start,end)` pair into pooled entry metadata/tile buffers;
install only owned rows and reset evicted rows before pool reuse. Test overlapping
owner/halo rows for ids 27/41/45, different dense patch/node owners, one-sided
id-35 presence, and growth/eviction without stale reachable addresses.

### Task 10: Connect visible cells and prove the thin path

Consume Tasks 3, 8, and 9. Session owns the controller; after visibility it updates
targets and passes a bounded drain batch into renderer before scene recording. Capture
uses deterministic synchronous preload for its fixed visible set. Update both
windowed and headless `render_frame_indirect` call sites in `postretro/src/main.rs`,
the renderer's `renderer_render_frame.rs` and `renderer_capture.rs` entry points,
and `capture/prepared.rs`/`capture/driver.rs`; apply the returned `ShDrainOutcome` to
the session controller before the next frame. Test install timing,
late completion deferral, partial-family refusal, zero/one/many real-visible synchronous
installs, all streamed families, next-frame promotion, and legacy-off parity. This task is
the explicit gate before Task 11; do not begin bounded async until the synchronous thin
path passes.
At this intermediate feature-branch checkpoint, only explicit `sync-proof` and
`off` are runnable; unset/`async` return a named not-yet-implemented development
error rather than silently running synchronous I/O on the frame path. Task 11
replaces that temporary error with bounded async and establishes the final
default-async table above before the feature is shipped.
Record and commit `measurements/sh-probe-streaming/cluster-residency-sync-proof.md`
with zero/one/many visible-cluster fixtures,
all-streamed-family compose and next-frame sample assertions, and legacy-off parity
test commands/results before dispatching Task 11.
Name the `VisibleCells` fixtures, captured target ids, frame-N install and
frame-(N+1) promotion assertions, and exact command/results in that record.

### Task 11: Add bounded async read and decode

After Task 10 passes, replace only the production synchronous source with four named
worker jobs. Jobs use the manifest's retained file for positional reads, verify the
per-chunk BLAKE3, decode CPU data, and send generation/content-tagged results. Four
permits span encoded read through decode, `Ready`, and install/drop; ready items retain
their permits. Move payload ownership without full clones, track checked encoded,
decoding, and ready current/high-water bytes, prioritize visible requests, cancel
cooperatively on reload, and prove synthetic latency never blocks the render thread.
Generation + tag + current target + chunk hash remains authoritative even after cancel.
Keep the synchronous gate for deterministic tests.
Use a session-owned worker manager with bounded request/completion channels,
cooperative cancel, and teardown/join before releasing the retained file. Inject
a positional-reader/executor seam: production uses `FileExt`, while a delayed
test reader adds 250 ms per chunk. Assert render preparation/drain returns
without waiting on that reader, all permits return after reload, and no prior
generation completion can install.

### Task 12: Add eviction, growth, hysteresis, and diagnostics

Enable departed-first then prefetch LRU eviction, owner pins, pressure suppression,
two-second retention, two-install drain cap, serialized append-preserving pool growth, and
edge-triggered overshoot logging. Extend the existing SH residency report with fixed GPU
metadata, whole-resident 47/48 capacity, active and retiring pool capacity, logical
occupancy, non-evictable overshoot,
replacement peak, checked encoded/decoding/ready current and high-water bytes, permit and
target/state counts, misses, installs, evictions, and retries. Do not label temporary
replacement capacity as logical overshoot or count logical occupancy twice.
Extend the install-time `ShResidencyReport` with a live per-frame snapshot rather
than treating its original ledger as mutable history; update
`postretro/src/capture/report.rs` serialization for the new rows. Renderer-owned
submitted-work completion tracking registers a callback immediately after the
last-use submission. That `Send + 'static` callback only sends/marks a bounded
per-family completion ticket; it never mutates renderer pools or reports, since
it may run on a submit/poll thread. At the next pre-compose drain, the renderer
consumes the ticket, releases the retired generation, and removes its capacity
from the live snapshot. Tests distinguish
non-evictable logical overshoot from temporary replacement peak.

### Task 13: Verify and record the residency finding

Run focused, workspace, cold determinism, reload/teleport, shader-budget, and leak/resource
checks. Record whole versus streamed reports and seam inspection under
`measurements/sh-probe-streaming/cluster-residency.md`. Prefer GTX 1660 Super large-map
release/cold evidence; also attempt a mid-sized MacBook map. Record unavailable GPU runs
or GPU failures as `not-yet-evaluable`, never as proof or a hard gate. Tune the 256 MiB
engineering floor when comparable evidence is available; report whole-resident 47/48
bytes separately so partial savings are visible. Bounded async remains the
default for valid ids 49+50 regardless of adapter availability. Remove generated
PRLs/reports/PNGs/caches. After every phase check free disk; below 10 GiB use only
crate-scoped `cargo clean -p` for PostRetro crates.
Add a named pure allocation fixture whose asserted whole-load requested SH bytes
exceed the effective floor and whose limited visible set gives a strictly lower
streamed active request; keep actual GPU copy/seam confirmation as manual proof.
Update `context/lib/build_pipeline.md` section inventory and
`context/lib/rendering_pipeline.md` residency/frame contract after implementation.

## Sequencing

After each completed phase, run `df -h` on the workspace volume and record free
space. Below 10 GiB, run only explicit `cargo clean -p <PostRetro crate>` for
PostRetro crates with significant build churn before starting the next phase;
never run bare `cargo clean`. Commit completed task work before dispatching the
next worker so a usage-limit interruption cannot discard it.

**Phase 1 (sequential, committed checkpoints):** Tasks 1, 2, 3, 4 —
behavior-preserving splits. Task 1 may begin during draft review because it is
independent of id-50 and residency design; no feature task begins before promotion.

**Phase 2 (sequential):** Task 5 — pins the shared id-50 contract.

**Phase 3a (concurrent):** Tasks 6 and 7 — compiler and loader consume Task 5.

**Phase 3b (sequential):** Task 8 — planner consumes the committed Task 7 handoff.

**Phase 4 (sequential):** Task 9 — consumes renderer split, manifest, codec, and planner.

**Phase 5 (sequential gate):** Task 10 — connects and proves the synchronous thin path.
Task 11 may not start until this gate proves zero/one/many real-visible synchronous
installs, all streamed families, next-frame promotion, and legacy-off parity.

**Phase 6 (sequential):** Task 11 — replaces production sync reads with bounded async work.

**Phase 7 (sequential):** Task 12 — enables budget eviction after async lifecycle is proven.

**Phase 8 (sequential):** Task 13 — integrated proof and measurement record.

## AC-to-proof map

| AC | Producing tasks | Primary proof |
|---|---|---|
| 1 | 1–7, 9, 13 | frozen legacy-section byte/hash and cache-epoch assertions, registry/version/BGL tests; shader stencil/binding review and grep gate |
| 2 | 5–7 | codec/property/cold determinism tests |
| 3 | 8–10 | synchronous state and frame-order tests |
| 4 | 9–10 | CPU sampler reference: one resident neighbor and all corners missing; seam read |
| 5 | 11 | injected 250 ms reader, non-blocking render-preparation assertion, permit/cap tests |
| 6 | 7–12 | injectable generation-clock exhaustion and worker teardown/reload/stale-completion tests |
| 7 | 8, 12 | deterministic-time planner tests |
| 8 | 6, 8, 9, 12 | owner/halo compose and eviction tests |
| 9 | 9, 12, 13 | pure allocation fixture with whole-load > floor precondition; report/stress tests; manual GPU copy proof |
| 10 | 2, 9, 13 | pipeline budgets, shader grep/review |
| 11 | 4–13 | one-/four-worker cold hashes, teleport high-water tests; no-full-clone code review and manual cleanup record |
| 12 | 13 | manual measurement record |

## Open questions

None block implementation. The 256 MiB default is an engineering starting floor, not a
claim that it is optimal. Task 13 records its result; changing it later is tuning, not a
wire or sampler change. Valid ids 49+50 use bounded async by default; `off` remains a
developer/test compatibility escape and `sync-proof` remains a deterministic proof mode.
Neither gate is player-facing, and unavailable adapter evidence does not defer rollout.
