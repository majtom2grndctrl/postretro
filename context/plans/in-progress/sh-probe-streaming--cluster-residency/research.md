# SH Probe Streaming Cluster Residency — Grounding

Read at `6d5054d36`. This file records current source anchors and the lifecycle used to
derive the Slice 3 contract. It is research, not durable architecture.

## Lifecycle

```mermaid
sequenceDiagram
    participant Frame as App render preparation
    participant Plan as ShResidencyController
    participant Worker as bounded load worker
    participant Drain as renderer residency drain
    participant Compose as three SH compose paths
    participant Sample as forward/billboard/fog/SDF

    Frame->>Plan: read VisibleCells and monotonic render time
    Plan->>Plan: map cells to clusters; close two-hop prefetch and owner pins
    Plan->>Worker: enqueue target cluster with content tag + residency generation
    Worker->>Worker: positional-read one id-50 chunk; hash; decode owned CPU payload
    Worker-->>Plan: ready completion retaining permit + tag + generation + cluster id
    Frame->>Plan: take at most two ready completions, visible-first
    Plan->>Drain: target snapshot + ready batch + eviction requests
    Drain->>Drain: reject stale/non-target; invalidate evictions; install compose data
    Drain->>Compose: installed set + dirty flattened affinity ranges
    Compose->>Compose: direct, billboard, indirect writes for every installed family
    Compose-->>Drain: queue-ordered composition belongs to frame N
    Note over Drain,Sample: frame N samples only prior installed-and-composed set
    Frame->>Drain: frame N+1 drain
    Drain->>Sample: publish depth-moment words for frame-N composed clusters
```

Every arrow has a current or planned read site. Current visibility is produced in
`crates/postretro/src/main.rs` before `Renderer::render_frame_indirect`. Current SH
composition is encoded inside `Renderer::record_scene_passes`: direct and billboard
compose run before mesh planning; indirect compose runs in `record_pre_scene_compute`;
every forward consumer runs later in the same command buffer. The plan adds the sole
drain before those calls.

## Current source anchors

| Contract | Current source |
|---|---|
| Cell clusters and owned/halo ranges | `level-format/src/cluster_directory.rs` |
| Loaded inert directory | `LevelWorld.cluster_directory` in `level-loader/src/prl.rs` |
| Whole-file load | `load_prl_with_section_limits` in `level-loader/src/prl_loader.rs` (`std::fs::read` today) |
| v11 metadata and atlas bytes | `OctahedralShVolumeSection` in `level-format/src/sh_volume.rs` |
| v4 direct stored atlas | `DirectShVolumeSection` in `level-format/src/direct_sh_volume.rs` |
| Canonical stored-node prefix | `stored_node_prefix_sum` in `level-format/src/sh_reconstruct.rs` |
| Load-derived slot words | `build_probe_indirection_words` in `renderer/src/render/sh_indirection.rs` |
| Depth moment + sample indirection carrier | `pack_probe_depth_moments` in `renderer/src/render/sh_volume.rs` |
| Whole SH GPU install | `Renderer::install_level_geometry` in `renderer/src/render/renderer_resources.rs` |
| World-to-renderer adapter | `level_world_to_geometry` in `renderer/src/render/renderer_geometry.rs` |
| Visible cells | `determine_visible_cells` call in `crates/postretro/src/main.rs` |
| Shared frame recorder | `Renderer::record_scene_passes` in `renderer/src/render/renderer_render_frame.rs` |
| Direct/billboard compose | `renderer_render_frame.rs` before mesh/shadow work |
| Indirect compose | `record_pre_scene_compute` in `renderer/src/render/renderer_shadow_passes.rs` |
| SH allocation report | `renderer/src/render/sh_residency.rs` |
| Capture setup | `crates/postretro/src/capture/prepared.rs` |
| Mesh frame-system shadow selection | `crates/postretro/src/scripting/frame_systems/mesh_render.rs` reads `LevelWorld.entity_shadow_lights` |

Relevant exact anchors for the Slice 3 storage seam:

- Finalized SH publication and source availability: `level-compiler/src/pipeline.rs`
  lines 1051-1057, 1524-1536, and 2000-2008; `level-compiler/src/pack/finalized_sections.rs`
  lines 9-18.
- Legacy whole-section loader shape: `level-loader/src/prl.rs` lines 602-634.
- Container-v4 identity fields: `level-format/src/container.rs` (`MAGIC`,
  `CURRENT_VERSION`, `Header`, and ordered `SectionEntry` id/offset/size/version fields).
- Current whole-file ownership and cursor model: `level-loader/src/prl_loader.rs`
  `load_prl_with_section_limits` reads a path into one `Vec<u8>` and constructs a cursor;
  there is no retained file handle or positional-read seam yet.
- World-to-renderer/capture whole-body assumptions: `renderer/src/render/renderer_geometry.rs`
  lines 81-94; `renderer/src/render/renderer_types.rs` lines 388-424;
  `crates/postretro/src/capture/prepared.rs` lines 83-89; startup install in
  `crates/postretro/src/startup/lifecycle.rs`; `LevelWorld.entity_shadow_lights` direct
  read in `crates/postretro/src/scripting/frame_systems/mesh_render.rs`.

## Actual v11/v4 gather mechanics

Id 34 v11 stores dense probe metadata first and one globally packed stored-tile atlas.
`stored_node_prefix_sum` derives slots in brick-major order. L0 stores every valid probe;
L1 stores eight node corners; L2 stores one node mean. Scale>0 member bricks resolve the
aligned origin node's slot. Renderer derives one word per dense probe and copies that word
into depth-moment B/A channels. Fragment sampling reads the word from the existing 3D
depth-moment texture, then samples the atlas slot. There is already no separate locate
buffer in the fragment path.

Id 35 v4 carries no metadata. Loader and renderer require its stored geometry to match id
34 exactly, so the same slot word addresses both base atlases. This is the key usable seam:
a streamed pool can relocate one shared local slot run and patch the existing word, without
changing sample taps or adding a binding.

The source-grounded local atlas precedent is `stored_layout` in
`level-compiler/src/sh_density.rs`: it calls
`irradiance_atlas_array_layout([tile_count, 1, 1], tile_dimension,
MAX_SH_ATLAS_DIMENSION)`. Slice 3 uses the same helper with
`[local_slot_count, 1, 1]` and physical tile dimension `8` for id-50 isolated cells.

The stale parent assumption that a cluster is a contiguous slice of existing v11/v4 bytes
is false for production BC6H. Logical tiles are 6x6 but BC6H encodes 4x4 blocks across the
whole atlas. Adjacent tiles can share physical compression blocks. Arbitrary cluster
gathers therefore are not independent byte ranges. The runtime also has no BC6 decoder.
An independently encoded cluster companion is required unless the whole source atlas stays
resident, which would defeat the goal. Id 50 supplies that companion while ids 34/35 remain
unchanged. Therefore the id-50 encoder must run while borrowed pre-BC6H packed id 34/id 35
sources and the finalized presence inventory are still available; encoded global BC6H
bytes alone are not enough to create independent 8x8 residency cells.

## Existing companion shapes

- Ids 27/41/45 are global affinity-cell CSR structures with flat entry/tile payloads.
  Directory ranges identify rows, not encoded byte spans.
- Id 47 is dense x-fastest `Rgba16Float` over the id-34 grid and currently uploads as a
  whole 3D texture.
- Id 48 mirrors id 45's CSR topology but expands every entry to 64 RGBA16F probe samples.
- Owner scope decision for this slice: ids 47/48 stay whole-resident. Billboard scatter
  samples id 47 at the global dense probe coordinate; the existing SH indirection word
  names a stored-node slot that L1/L2 share across multiple dense probes. It cannot
  uniquely address dense scatter without a separate scheme. A later asset-streaming
  project may redesign texture sampler assignments, so id 50 covers only ids
  27/34/35/41/45. Report the unchanged 47/48 allocation separately.
- Ids 27/41/45 carry `valid_probe_masks` and `cell_levels`; those are row-parallel
  metadata. Keeping them always resident avoids per-row duplication in id 50. Id 48 has
  no mask/level stream and instead stores dense 64-probe RGBA16F blocks per CSR entry.
- Direct, animated-direct, and billboard compose each own different buffers and dirty
  predicates. A cluster is not sampleable until all present families have composed.
- Animation descriptors/sample curves and descriptor maps are small shared global data;
  streaming them per cluster would duplicate mutable runtime state and break script writes.
  They remain always resident.

Current compose entry points are affinity-brick based. Indirect compose dispatches over
`buffers.affinity_dims`; direct and animated-direct compose use the same `GridDims`
shape and affinity dimensions; billboard compose follows the same per-affinity family
contract. Dirty streaming compose therefore derives affinity-row ranges even for
base-only dense sources. It never dispatches raw dense indices.

The current compose `GridDims` uniform is 64 bytes, packed by
`postretro_render_cpu::sh_compose::build_compose_grid_bytes`: three grid-dimension
words, tile dimension, two atlas-dimension words, tile border, delta-probe f16 stride,
three affinity-dimension words, atlas tiles per row, tiles per layer, atlas layer count,
and two compact-atlas tail words. Binding 18 is a non-dynamic uniform in the current
indirect, direct, and animated-direct compose BGLs. Slice 3 changes only the dynamic
offset flag and record shape at the same binding number.

## Miss and promotion seam

The current all-zero probe-indirection word is invalid. All SH samplers drop invalid
corners, renormalize survivors, and reach ambient floor when none survive. It is therefore
the existing conservative miss representation. Compose already reads separate storage
buffers containing the same derived words, while fragment consumers read words packed in
the depth-moment texture. Keeping the sample-side word invalid for one frame lets compose
write a newly installed cluster without exposing it early. Queue ordering then permits
promotion at the next drain with no GPU readback.

## Stale identity and file ownership

Reopening a pathname per job is not a stable file identity: an editor or build can replace
the directory entry while an old level session is still alive. File metadata alone is also
not a sufficient content identity across supported filesystems. The manifest therefore
retains the exact `Arc<File>` opened and validated at level load. Its stored path exists
only to make diagnostics useful. Unix `FileExt::read_at` and Windows
`FileExt::seek_read` provide cursor-independent positional reads against that handle; this
avoids a mutexed shared seek cursor and permits the four workers to read independently.

Stale-result rejection has two independent axes. A process-monotonic nonzero checked
`u64` residency generation distinguishes load sessions; it never resets or wraps. A
BLAKE3 streaming content tag distinguishes validated bytes. Its domain-separated input is
the exact container-v4 magic/version/count and ordered section-table tuples, exact id 49,
and validated id-50 header/source/index. Per-chunk hashes in that index bind the omitted
payload blob. This tag is intentionally not the co-op/network content identity: streaming
lifetime policy must not silently redefine multiplayer compatibility. Cooperative worker
cancellation saves work, but generation + tag + current target + freshly verified chunk
hash is the authoritative install predicate.

## GPU and CPU lifetime accounting

The portable renderer report measures requested allocations, not opaque driver VRAM. The
256 MiB engineering floor therefore contains fixed GPU metadata and active physical pool
capacity only. Logical occupied ranges are a sub-ledger of that capacity. Encoded,
decoding, and ready host bytes are separate checked phase ledgers with independent high
waters; adding them to the GPU floor would mix unlike resources and double-count resident
data. Four lifecycle permits cover read through install/drop, including time spent ready,
so a two-install frame cap cannot create an unbounded ready queue. Moving one owned
payload between phases avoids full clones, and installation retains no full decoded host
copy.

Pool growth must not depend on that discarded CPU representation. The selected design
allocates a geometrically larger active resource and copies live ranges GPU-to-GPU at the
same offsets, preserving installed indirection. The former active resource becomes the
single retiring generation until queue completion proves it unreferenced. Each family may
hold at most active plus one retiring generation, and another growth waits. This makes two
different costs explicit: bytes required above the effective floor are non-evictable
overshoot; simultaneous active-plus-retiring capacity is a temporary replacement peak.
Calling both simply "overshoot" would hide the actual allocation high water.

## Oversized-file flags

The implementation would otherwise extend `level-loader/src/prl.rs` (~7,300 lines),
`prl_loader.rs` (~3,970), `renderer/render/sh_volume.rs` (~2,020),
`direct_sh_compose.rs` (~1,340), `sh_compose.rs` (~950),
`renderer_render_frame.rs` (~1,050), `crates/postretro/src/main.rs` (~14,000),
`crates/postretro/src/capture/driver.rs` (~1,660), `level-compiler/src/pipeline.rs`
(~2,890), `pack.rs` (~2,790), and `pack_output.rs` (~1,110). Phase 1 splits each touched
responsibility before feature work.

## Measurement interpretation

Slice 1 recorded only disk footprint because this host exposed no Metal adapter. Slice 2
proved deterministic directory construction and CPU-side inert loading, but its same-
adapter output comparison was also unavailable. Neither result says streaming lacks value.

Slice 3 manual evidence therefore has a bounded fallback order:

1. GTX 1660 Super: release/cold whole-load versus streamed on the largest map that boots.
2. This MacBook Pro: same comparison on a mid-sized map, using the existing window-title
   frame-time windows and renderer requested-byte report.
3. If adapter or map execution fails, retain exact commands/diagnostics and mark the GPU
   portion `not-yet-evaluable`. Do not infer runtime benefit from disk bytes.

The report distinguishes physical pool capacity from logical occupied bytes. The pool is
the requested GPU allocation and is the relevant whole-load comparison. Logical occupancy
explains policy behavior but is not mislabeled as driver VRAM. Opaque driver padding remains
outside the portable report, matching the existing SH residency ledger.

GPU failures or unavailable adapters remain `not-yet-evaluable`; they are never proof for
or against the streaming result. They also are not a hard rollout gate: valid ids 49+50
default to bounded async streaming. `POSTRETRO_SH_STREAMING=off` remains a developer/test
compatibility escape and `sync-proof` remains deterministic proof machinery; neither is a
player-facing mode. Task 13 measures and tunes the engineering floor when the relevant
adapter is available.

## Resource and disk lifecycle

Cold compiler products live under a session-owned `/private/tmp` directory or hidden names
under the source mod's maps directory when runtime content-root derivation requires it.
Retain only small JSON/Markdown evidence. Delete generated PRLs, scenes, PNGs, chunk dumps,
scratch caches, and temporary worktrees after proof.

After every implementation phase run `df -h` for the workspace volume. Below 10 GiB free,
clean only PostRetro crate artifacts with explicit `cargo clean -p <package>` calls. Never
run bare `cargo clean`.
