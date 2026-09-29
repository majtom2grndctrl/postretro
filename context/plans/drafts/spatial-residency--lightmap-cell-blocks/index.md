# spatial-residency--lightmap-cell-blocks

Brief · resumable · build-to-learn · Epic: large-map-spatial-residency stage 5 · reads: `context/lib/rendering_pipeline.md` §Cluster SH residency, §7.1, §7.8 · `context/lib/build_pipeline.md` §PRL section IDs · `context/lib/experimental_spikes.md` · read at 87fdf8583

## Problem
Owner goal: PostRetro runs smoothly on laptops with other apps open. Low tier gives lightmap-shaped data 256 MiB, and interior maps show no pop-in. The static lightmap (id 22) and shadowmask (id 42) load whole. On `stress-warren-hallway-inspection` they cost about 1 GiB, and no packing of whole layers fits Low (seed §Pre-planning measurements). This experiment streams both as per-cell blocks, sized by a baked mandatory set: what is visible from anywhere within a movement lead of the camera cell. Done means the hallway map runs with ids 22 and 42 streamed. The runtime reports resident and mandatory bytes, pool layers, misses and repacks against the hypotheses in Acceptance. The findings note then says whether cell blocks suffice for Low or which lever to pull next.

## Decisions
- **Mandatory set is visible set plus lead, plus pins** (owner, 2026-09-28). A camera cell's mandatory set is every cell within portal-path lead L, plus each such cell's visible set, plus the cells of pinned clusters. The engine has no draw distance, so no distance bound alone stops pop-in.
  - **Visible set.** The runtime portal walk, sampled from eye points over the shipped portals, then dilated by one portal hop (PVS to PHS). Sampling undercounts by construction; dilation catches the edge cells it misses, and visible misses measure what it still misses. A Quake-style anti-penumbra PVS bake is rejected for its bake time (owner).
  - **Pins and priority.** Lightmaps read the id-49 cluster hints: a cluster flagged by a `stream_resident_volume` is pinned, and a `stream_priority_region` ranks prefetch. The hints are decoded once, in the shared streaming layer, for every resource. SH's owner closure stays SH-only, because it follows probe ownership, which lightmaps don't have.
- **The baked set is a streaming-owned cell relation.** It maps a camera cell to (cell, lead), where lead is the smallest L that makes the cell mandatory. The runtime chooses L up to the baked maximum. The set is keyed by cell and never names a lightmap block, so a later streamed resource maps the same cells to its own units. It is not an id-46 axis. `context/research/cell-visibility-substrate.md` keeps id 46 conservative (§Invariant hierarchy) and keeps streaming lookahead consumer-side (§Bake-side contract), and this set is sampled and carries a streaming lead.
- **Residency unit is the cell block.** Each cell's charts pack into one BC-aligned block. Whole layers overshoot Low, and fixed tiles need chart splitting. Cluster closure fits the hallway map but spends the margin larger maps need (research §2). This diverges from `plans/done/sh-probe-streaming` Slice 2, which makes the cluster directory the substrate every later resource keys on. Id 49 stays unchanged and still supplies the pins.
- **Ids 22 and 42 are one unit.** A lightmap block and its shadowmask block install and become sampleable in the same drain, generation-matched, or not at all (seed §Constraints).
- **Block identity reaches the vertex stage.** The static-layer field of the per-vertex lightmap attribute becomes block id + 1, with 0 meaning no lightmap. Lightmap UVs become block-local. The vertex stage resolves each block's pool layer, offset and residency from a vertex-only table. Fragment bindings don't change, because the forward fragment stage has none free.
- **Animated lighting follows the block frame.** Animated weight-map keys (id 25) and the animated block table rebase from static-atlas texels to block-local texels.
- **One read issuer, one budget owner.** A single ordered read issuer and a single drain budget are shared by SH and lightmap blocks.
  - One owner decides how much work a drain carries across both resources: bounds are policy, owned once (`plans/done/sh-streaming--warm-set-and-io-contract` §Resolutions). Today that owner is the SH controller; the seed's resource-neutral planner will replace it.
  - Mandatory reads of both resources go before optional reads of either, each tier in file-offset order. Two issuers would split that ordering on SATA, and a second cap at a boundary would repeat the retired-cap bug.
  - Blocks upload in their stored texel formats with no decode step; SH decode stays SH-only. Extracting the shared layer must not change SH behaviour.
- **Miss policy is stricter than SH's.**
  - Mandatory and visible blocks are never refused; the pool grows past its cap to hold them. The owner's no-pop-in rule rules out SH's ambient-floor tolerance.
  - Entries with lead between L and the baked maximum form a prefetch band. Band blocks are requested as prefetch: the cap bounds them and priority regions rank them. A block that leaves the mandatory set but stays in the band remains resident, so a block re-entering the mandatory set is usually already there. No timer is involved. The band trades pool size and repacks for fewer demand reads (research §1). Blocks outside both are freed at the next drain.
  - A transient miss drops static direct light and static specular for that block and keeps SH indirect.
  - Every non-portal visibility path (step-limit overflow, solid or exterior camera cell, no portals, empty world) demands only the camera cell's baked set. A solid or exterior camera cell has none, so those frames keep current residency and demand nothing new. A level without portal data has no meaningful baked set and runs in all-resident mode.
- **Level install makes the spawn cell's mandatory set resident before the first frame.** This extends the SH rule "doors never wait on residency". Gameplay never waits on a block.
- **An all-resident mode loads every block into the pool at install.** It is the parity baseline and the fallback when streaming is unavailable, mirroring SH's `off` mode.
- **Budget is a dev-tools slider, not a player setting.** The sliders are pool cap and lead L (`experimental_spikes.md` §Tuning levers). The player-facing tier is a later spec. Promotion amends `resource_management.md` so that baked lighting residency is in scope: the §9 texture-streaming non-goal, the §8.2 lifecycle table, and the paragraph on GPU-only baked payloads.
- **Renderer owns the pool, allocator, upload, growth, repack and retirement.**
  - Repack compacts in place: blocks move between layers of the same pool texture through one reserved spare layer. The table rewrite lands in the same submission as the copies. A repack never holds a second pool, since a doubled pool would overshoot the laptop budget.
  - Only growth allocates a new generation. At most one is retiring at a time, released on submitted-work-done. A mandatory block that waits on a retirement is a transient miss, and it is counted.
- **Hot paths** (`development_guide.md` §1.4, performance is part of the task):
  - The forward vertex stage does one table fetch per vertex. The fragment stage adds one offset per lightmap lookup and no new binding. This is bounded GPU work, measured as a forward-pass delta where timing exists.
  - Demand from the baked set is recomputed only when the camera cell or L changes. Visible-block demand reuses per-frame buffers. Neither allocates in steady state.
  - Block-table writes happen only on install, eviction or rebuild, never every frame.
  - Uploads stay within the shared drain budget.
- **Non-goals:**
  - Half-resolution lightmaps: owner-dropped. Full resolution fits Low on the evidence map.
  - Streaming the animated atlas or its weight maps. They add a fixed load outside the cell-block pool (research §Other pins), and the findings note reports it against Low.
  - Bake noise that survives repacking. A format change forces a full rebake anyway.
  - Sparse shadowmask placement: texels no light reaches are still stored. Deferred because cell blocks fit Low at every measured lead (research §1).
  - Splitting a cell whose block exceeds a pool layer. The build rejects it, and the findings note records the margin (research §1).

## Acceptance
### Automated — honesty gates
- [ ] Every chart lies inside its cell's block, and no two charts overlap. Block edges are multiples of 4 × the direction texel scale, and no block exceeds a pool layer. A level whose block count would overflow the vertex id rejects the build, and a level one below that limit builds.
- [ ] The baked residency set equals the dry run's mandatory set, dilation included, for every camera cell at every lead up to the baked maximum.
- [ ] A cell in a cluster flagged pinned is mandatory from every camera cell. A cell in an unflagged cluster is mandatory only through lead or visibility. Across the same band blocks, a priority region reorders lightmap prefetch.
- [ ] On the same PRL, streaming with a pool large enough to hold every block renders pixel-identical to all-resident mode, on both evidence maps (headless capture). Capture reads the view's mandatory and visible blocks synchronously before the frame, as it preloads SH.
- [ ] Every lightmapped vertex's block id and block-local UV, resolved through the block table, address the same chart texel its static-atlas UV addressed before the rebase.
- [ ] Every animated face's block-local key addresses the same chart texels its static-atlas key did before the rebase. Load rejects an animated block that falls outside its cell block.
- [ ] A block forced missing renders with static direct and static specular absent and SH indirect present. The same capture with the block resident renders it lit. The missing-block pixels match the same capture with the static-direct and static-specular light terms masked off.
- [ ] With the shadowmask read held back, neither half of the pair becomes sampleable. Releasing the read makes both sampleable in the same drain.
- [ ] Pool cap below the mandatory set: the pool grows, and every mandatory and visible block installs. Pool cap above the mandatory set: band blocks beyond the cap are refused, and a block outside the band is freed at the next drain.
- [ ] A repack allocates no second pool texture, and afterwards every resident block samples its own texels.
- [ ] Before the first rendered frame after level install, every block in the spawn cell's mandatory set is resident.
- [ ] In streaming mode, bytes read from ids 22 and 42 equal the requested block ranges plus the index. Neither whole payload is ever held in memory. Bytes are counted at the positional reader under both the loader and the issuer, in the game and in capture.
- [ ] Completions from a previous level generation are discarded after a reload. Level unload releases the pool, the retained file handle, the workers and any retiring pool.
- [ ] A frame on each non-portal visibility path requests no block outside the camera cell's baked set. From a solid or exterior camera cell, it requests nothing new. An empty-world frame performs no residency-set lookup and requests nothing. A level without portal data loads all-resident.
- [ ] Steady-state frames with an unchanged camera cell write no block-table bytes and make no heap allocation in lightmap residency. A camera-cell change writes only the entries that changed. Proven by unchanged capacity of every per-frame residency buffer across the steady frames, and by a block-table write counter.
- [ ] A level with no lightmap blocks boots, and a level with exactly one block streams it.
- [ ] Load rejects each of these:
  - an older lightmap or shadowmask section version;
  - a mismatch between the id-42 block count and id 22;
  - a residency entry naming a cell past the cell count;
  - residency CSR offsets that decrease or pass entry_count;
  - a vertex block id past the table;
  - a block blob range outside its section;
  - a block larger than a pool layer;
  - a nonzero reserved field.
- [ ] With lightmap demand idle, SH streaming issues the same request order and install budget as before extraction (regression guard). Measured against a request and budget trace recorded from the pre-extraction controller on the same synthetic schedule. The existing SH streaming tests pass unchanged.
- [ ] With SH and block demand both pending, one issuer thread performs every read, mandatory before optional across both resources, each tier in ascending file offset.
- [ ] Each ordering in research.md §Pin rows (P1–P12) produces its expected outcome, in tests over the residency controller and a fault-injected issuer that holds reads, as the SH worker tests do.
- [ ] Headless BGL tests pin the forward pipeline: FRAGMENT bindings are unchanged, and the only VERTEX addition is the block table.
### Automated — measured findings (reported, not gated)
- [ ] Hallway and campaign-test walks, logged and in capture JSON:
  - per-resource resident and mandatory bytes;
  - peak pool layers, repack count, and the transient peak bytes during growth;
  - install time per drain and bytes read;
  - visible misses, counted two ways: drawn but outside the baked set, and drawn but not yet resident.

  Hypotheses: research §1, re-measured by the dry run against this brief's mandatory set. Expect no visible misses after spawn.
- [ ] Per-frame CPU time of lightmap residency in the `[CpuTiming]` line, on both walks.
- [ ] PRL size delta and time-to-first-frame delta on both maps, against the whole-resident build.
- [ ] Dry-run cost of dilation: worst and p95 mandatory bytes with and without the one-hop dilation. Visible misses at runtime are the check on whether dilation suffices.
### Manual
- [ ] Owner walk on the hallway and campaign-test at the default lead, with the pool-cap and lead sliders live: no pop-in, no seams at block edges, animated lights correct.
- [ ] Windows walk with `POSTRETRO_GPU_TIMING=1`: forward-pass cost of the vertex table lookup (hand off; this Mac has no timestamp queries).
- [ ] Findings note: measured values against the hypotheses, the visual read, and a recommendation (cell blocks suffice; allocator or repack change; or a lever such as half-res or a smaller lead for Low).

## Path
- **Evidence and prototypes:** the dry run (`pvs_sampling.rs`, `visible_set.rs`, `cell_blocks.rs::pack_cell_block`, `block_allocator.rs::BlockPool`). Move the sampling and block packing into compiler stages; don't reimplement them. The ignored test stays the yardstick.
- **Seams:** research §3 (block-local UV blast radius) and §6 (shared-issuer extraction). The residency-set bake needs the portal world and locator the dry run's `inputs.rs` builds; the loader's section-to-world converters are `pub(crate)`.
- **Layout:** store blocks cluster-major so the issuer's coalescing works as it does for SH. Group 6 is free for the vertex table (research §Other pins). The compose pass should need no change, since it is layout-free (research §3).
- **Rebuild precedent:** SH dense growth (`sh_streaming/gpu/growth.rs`) copies GPU-side with `copy_texture_to_texture`, at origin zero, under the one-retiring rule. Moving blocks to new offsets is new. WebGPU allows a same-texture copy only between distinct subresources, which is why repack goes through the spare layer.
- **Hint decode:** today the pin and priority decode sits in SH's `PlannerTopology`, which is `pub(super)`. It moves to the shared layer.
- **Varyings:** the vertex stage emits the interpolated block-local texel, a flat pool layer and offset, and the flat animated id. Check the inter-stage variable limit before settling on this.
- **Settle chokepoint:** `drafts/sh-streaming--reveal-gate-and-warm-horizon` expects lightmap streaming to join its settle chokepoint. Coordinate with it before either lands.
- **Rival shape:** a fragment-stage page table with fixed tiles and bordered pages (classic virtual texturing). It is rejected because it needs a fragment binding and chart splitting.
- **First slice (the riskiest assumption):** block-major format, block-local UVs, the vertex table and the animated rebase, rendered in all-resident mode. It passes if it is pixel-identical to streaming with everything resident on both maps. Streaming and the shared issuer come next.
- **Split first:** `forward.wgsl`, `lightmap_bake.rs`, `lightmap_layer.rs`, `pipeline.rs`, `prl_loader.rs` and renderer `lighting/lightmap.rs` are all past 800 lines. Split each along the seams this brief touches, in its own commit, right before the task that extends it.

## Open questions
- Allocator: the measured shelf allocator, or a guillotine allocator that merges freed space. — **delegated**: start with shelf, and report repack counts.
- Baked maximum lead (lean: 32 m), default L (lean: 16 m), and default pool cap (lean: 15 layers at L = 16 m, where the brief set's worst cell needs 12 and repacks stay under 3% of steps; research §1). — **delegated**

## Wire format
Little-endian throughout, following the id-50 index layout (fixed header, then fixed-width records, then payload blobs). Offsets are from the section payload start. Every count is a `u32` in the header, and no list is length-prefixed per entry. Reserved fields are written as zero, and load rejects nonzero.

| Section | Layout |
|---|---|
| Id 22 Lightmap (version bump) | Header: version `u32`, block_count `u32`, direction_texel_scale `u32`, irradiance_format `u32` (the existing BC6H / Rgba16Float tag). Then `block_count` records: cell_id `u32`, width `u16`, height `u16`, irradiance_offset `u64`, irradiance_len `u32`, direction_offset `u64`, direction_len `u32`, reserved `u32`. Then blobs: one block's irradiance, then its Rg8 direction. Records and blobs are sorted by owning cluster, then cell id. Block id is the record index. Zero blocks is a valid header with no records. |
| Id 42 Shadowmask (version bump) | The existing channel table is unchanged. Then block_count `u32` (must equal id 22's), then `block_count` records in id-22 block order: group_a_offset `u64`, group_a_len `u32`, group_b_offset `u64`, group_b_len `u32`, reserved `u32`. Each group blob is BC5 at the block's width × height. The pool layer keeps today's side-by-side halves. |
| Id 17 Geometry (container version bump) | Vertex layout and stride are unchanged. The static-layer field becomes block id + 1, with 0 meaning no lightmap. The lightmap UV becomes a block-local unorm over the block extent. |
| Id 25 animated weight maps (version bump) | The animated block key becomes block id `u32` plus block-local x and y (`u16` each), replacing static layer and static x/y. |
| New: cell residency set (next free SectionId) | Header: version `u32`, camera_cell_count `u32`, max_lead `u32`, entry_count `u32`. Then `camera_cell_count + 1` `u32` entry offsets (CSR), indexed by camera cell id. Then `entry_count` records: cell_id `u32`, lead `u32`, in id 46's fixed-point units. Lead is 0 for the camera's own dilated visible set. Each camera cell's entries are sorted by lead, then cell id. A camera cell with no entries has an empty range. Pins are not baked here; they come from id 49. |

## Boundary inventory
No script, FGD or network surface. Residency is local resource state. Co-op admission uses the same content identity (seed §Constraints).
