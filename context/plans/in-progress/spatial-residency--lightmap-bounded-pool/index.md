# spatial-residency--lightmap-bounded-pool

Brief · resumable · reads: `context/lib/index.md` §1.1 · `context/lib/rendering_pipeline.md` §4 (Lightmap cell-block residency) · `context/lib/build_pipeline.md` §Compiler pipeline, §PRL section IDs · `context/lib/development_guide.md` §1.4 · `context/lib/testing_guide.md` §Resource bounds · `context/plans/large-map-spatial-residency.md` · read at 6a813bdcd · evidence: `research.md`

## Problem
The owner's memory spike found the lightmap pool on `stress-warren-hallway-inspection` ending at 532 MiB and peaking at 1,050 MiB in a walk, against a 2 GB target for the whole level. With `done/spatial-residency--sh-bounded-pool` landed, this pool is what keeps the hallway over target: 2,170 MiB steady and 2,574 MiB at peak on Metal, the peak being lightmap growth. Its causes compound. The default lightmap density moved to 0.025 m/texel, 2.56x the texels of 0.04. Nearly half of each pool layer holds all-zero shadowmask planes, because a mask plane shares its block's placement whether or not it holds data. Repack cannot move a block to a higher layer, so fragmentation forces growth long before the live set fills the pool. Each growth then holds old and new pools together. When this is done, the compiler knows the worst mandatory lightmap demand of any camera cell and reports it to the author, pools are sized from it once at level load, all-zero mask planes are neither stored nor resident, and a walk within the bound never grows. Frames do not change.

## Decisions
- **Density stays 0.025 m/texel.** Reverting would be a visible quality drop on every map at the default, and the other causes exist at either density. This records the re-measure `done/lighting-scale--lightmap-layer-budget-gate` required before that default moved.
- **Id 42 stores no all-zero plane.** A plane whose encoded bytes are all zero has no blob, and the sampler reads it as zero. All-zero BC5 decodes to zero, so this is exact even on an owned face, and §4's miss and no-owner rules stand.
- **Mask planes get their own pool.** Each present plane is one placement, and the block table carries each plane's address. This replaces the side-by-side layer and shared layer index in `rendering_pipeline.md` §4 and `build_pipeline.md`. A map rich in masks costs what it costs today, and nothing caps owners per block.
- **A block stays one atomic pair across both pools.** Its lightmap and mask halves install in one drain or not at all, and leave together. `done/spatial-residency--lightmap-cell-blocks`'s pair rule is unchanged; only placement splits.
- **Repack escalates to full compaction.** The cheap downward repack runs first. Only if it fails may blocks move to any layer, staged through that pool's spare, never a second copy of a pool. This reverses §4's "a repack never moves a block to a higher layer", and makes the bound provable: any set that packs from scratch into the pool also fits at runtime.
- **Repack plans for the whole targeted demand.** It holds real reserved room for every mandatory and visible target, not only this drain's pairs, so a large demand arriving over several drains does not repack on each one.
- **The compiler computes the bound; bake over compute.** Worst mandatory layers per pool over every camera cell, at the default lead or the baked maximum where that is lower, using the runtime's placement code and order. A camera cell's set is its id 51 entries within that lead, plus pins. Stored in its own small section: a separate resource with its own version, present only when lightmaps stream. See Wire format.
- **Planar mirrors stay out of the bound.** Planar runs only at the top two reflection tiers, so including it would charge the default tier for an opt-in feature. The bound computes through one cell input. Adding mirror cells later still needs the sweep, and a second stored figure if the default tier is to stay free of them (`research.md`). A large water map that grows reopens this.
- **The bound reaches the author.** Build Summary reports lightmap pool bytes and the worst cell, and warns above 384 MiB, about what the other pools leave of the target once SH holds its 512 MiB. A level with no id 51 reports all-resident bytes, no worst cell, and the same warning. Automatic, with no author key.
- **Pools are sized once at load** from the bound times 5/4, rounded up, capped at what the level needs all-resident. With compaction the margin no longer covers fragmentation: it holds drawn demand the bound does not model and keeps band blocks, which cut reads and misses. The first split-pool walks confirm 5/4 (`research.md`); a disagreement goes to the owner.
- **The pool is the cap.** The fixed default cap and the dev cap slider retire, because a second capacity figure invites the bug it would hide, and no drain carries a cap. Grown layers count, and band blocks fill what mandatory demand leaves. Harnesses set pool size directly; the lead slider stays.
- **Growth stays a logged safety net.** Demand the bound does not model evicts band blocks, then repacks, then grows that pool once, by the same 5/4 over targeted demand, and warns, naming the pool and the camera cell. That demand is drawn cells outside the camera cell's set for the bound, a planar mirror's included, a raised dev lead, Settling on a fallback path, and E25's seam pre-commit. Pools never shrink, so an overshoot stays for the level. The §4 device-limit deferral stands.
- **Mask layers join the compile gate.** An all-resident level's mask pool can need up to twice the lightmap pool's layers, and mask presence is known only after the bake. A level that cannot stream and exceeds the array-layer floor in either pool fails by name; one that can stream warns, as today.
- **Non-goals.**
  - Runtime shadow map memory: the 427 MiB pool is separate, and lowering its resolution trades quality.
  - Shorter lead: measured worse, with twice the misses for three layers (`research.md`).
  - A new packer: MaxRects matched the shelf allocator on the worst cells.
  - Chart-level mask trimming: block-level absence already takes most of the zeros.
  - Pool shrink: it would add a second copy peak.
  - Memory tiers: `large-map-spatial-residency.md` owns them, and the bound gives them a checkable input. The hallway misses Low at this density, at any measured lead (`research.md`).
  - The animated atlas, weight maps and the unitemised 650 MiB: `large-map-spatial-residency.md` stage 5 and `E20--memory-visibility` own them.
  - Frame-thread payload split and cold staging: `drafts/lightmap-payload-split-frame-thread.md` and `drafts/lightmap-staging-warmup.md` carry them under `development_guide.md` §1.4.

## Acceptance
### Automated
**Format and compiler:**
- [ ] A plane with any nonzero byte is stored unchanged; an all-zero plane has no blob. Covered for a block missing plane A only, plane B only, both and neither.
- [ ] A PRL with the old tag is refused by name, and on a level that needs the bound section, its absence or an unknown version is refused by name.
- [ ] An id 42 record whose nonzero plane length is not the block's, whose absent plane keeps an offset, or whose present planes are not adjacent is refused by name.
- [ ] Bound on a hand-computed fixture equals the expected layers per pool at the default lead, where the two pools' worst cells differ, an entry exactly at the lead counts, one just past it does not, and a block both pinned and within lead counts once.
- [ ] A cell added to a camera cell's set for the bound raises the bound where it makes that cell the worst, with id 58's layout unchanged.
- [ ] The bound's cell input reads no water-plane data (grep gate).
- [ ] Id 58 is byte-identical warm and cold.
- [ ] The compiler and runtime agree on layer counts for one block set, in both pools.
- [ ] A level with no usable id 51 has no bound section and runs all-resident.
- [ ] Build Summary reports bytes and the cell with the most bytes, lowest id on a tie, for every level with a bound, all-resident bytes and no worst cell for one without, and warns only above the threshold for both, tested on both sides.
- [ ] A non-streaming level over the array-layer floor in the mask pool alone fails by name, cold and warm; one under it passes; a streaming level over it warns.
- [ ] An all-resident load whose mask pool exceeds the device's array layers drops the mask pool alone by name, as an unusable mask does today, and installs no plane.

**Sampling:**
- [ ] An absent plane samples identically to a stored all-zero plane, in the union subtraction and owner specular, for an unowned face, an owned face, and owners on channels 0 and 1 with plane B absent.
- [ ] A block whose lightmap, plane A and plane B sit at three different layers and offsets samples each from its own address.
- [ ] No-lightmap entries still read all-visible, misses still read zero, and a level without id 42 still reads the fully-visible fallback.
- [ ] All-resident and streamed modes place planes in the same pool shape and sample identically.
- [ ] A level with id 42 and no present plane loads in both modes and reads zero, not the fully-visible fallback (R27).

**Atomic pairs:**
- [ ] A pair needing room in both pools, where one cannot make room this drain or the mask pool holds only one of its two planes, installs nothing, leaves every rect free and retries whole; a band pair in that state is refused whole (R1).
- [ ] A pair whose one half needs growth while the other pool's generation retires defers whole and grows neither pool (R13).
- [ ] Evicting a band block frees both halves, even when only one pool needed room (R2).
- [ ] A band victim returns whole when every rect it held is still free, and stays out whole when any is taken (R16).
- [ ] A band pair refused for one pool's room is read again only when that pool has room for it (R17).
- [ ] A block with no mask planes installs with no mask placement and no mask read, and a block with one plane takes exactly one mask placement (R3).
- [ ] A payload whose planes disagree with the block's plane presence fails whole.
- [ ] A streamed block with plane A absent reads exactly plane B's bytes, and one with plane B absent exactly plane A's.
- [ ] An aborted drain restores both pools and the block table; a failed install frees both halves of its pair and leaves the drain's moves and growth standing (R4).
- [ ] Level change and a new controller's first drain leave nothing resident in either pool (R5).

**Compaction:**
- [ ] After any history of arrivals and departures, any resident and pending mandatory set that packs from scratch, tallest first, into the pool is placed by repack with no growth, and its copies complete through that pool's one spare on the copy mirror with every texel intact (property test, reporting how many cases reached compaction and staged a cycle, on a generator biased to near-full pools and mixed block sizes).
- [ ] A repack the downward pass can satisfy for the whole targeted demand moves no block upward; one that fits only the drain's pairs gives way to compaction in that drain (R10, R18).
- [ ] A demand larger than one drain's budget, whose targets pack into the pool together, arriving shortest first over consecutive drains, places with one repack and no growth (R11, R20).
- [ ] Targeted demand past the pool compacts at most once before it grows (R32).
- [ ] A block with reserved room stays a miss with no table write until its pair uploads, and no band pair lands in that room, in the drain that reserves it or a later one (R28).
- [ ] Room reserved for a target that leaves targeted demand before its pair is ready, by removal or by dropping to band, returns in that drain (R21).
- [ ] Reported band headroom leaves out reserved room, counts a reserved pair in hand once, and rises when that room returns, so a band pair refused for it is read again (R29).
- [ ] A compaction with no copy order through the spare leaves the layout as it was and falls through to growth, warned by name (R22).
- [ ] Moves up, moves down, moves within a layer and a two-block swap each land with texels intact on a mirror of the copy list, and a block's two planes stay told apart in the mask pool (R6).
- [ ] Repack of one pool leaves the other's placements alone, except that a band block it drops leaves both pools, and one drain's copies, uploads and table writes reach the GPU in one submission, counted at the queue (R7, R15).
- [ ] A repack of the mask pool alone rewrites the entry of every block with a moved plane, and no other (R14).
- [ ] One pool repacking while the other grows lands in one submission with texels intact, and only the grown pool retires (R23).
- [ ] Both pools repacking on one drain land in one submission with texels intact, and a block moved in both has one table write carrying both addresses (R31).
- [ ] A repack while a generation retires touches only the live generation (R8).

**Sizing:**
- [ ] First generation of each pool equals its stored bound plus margin, capped at all-resident.
- [ ] A walk with mandatory demand within the bound never grows with the pool sized at the bound and no margin, on a fixture that grows today at that size.
- [ ] A level capped at all-resident holds every block with no growth, on a block set that packs tighter in block-id order than tallest first, including one wholly mandatory from a single cell (R24).
- [ ] A band pair places in any layer of the pool, on a level sized past the old default and after a growth, under the first controller and one replaced mid-level (R25).
- [ ] No non-test source sizes a pool from the old default, carries a cap on a drain, or offers a pool cap lever (grep gate).
- [ ] `rendering_pipeline.md` and `large-map-spatial-residency.md` describe no lightmap cap, cap slider or default cap (grep gate).
- [ ] A capture scene's pool override, one figure per pool, sets that pool's first generation and nothing else, the capture report names no cap, and a streamed capture at a pool holding every block still matches all-resident.
- [ ] Demand past the pool grows that pool once and warns, naming the pool and the camera cell, only when a ready pair cannot be placed after evicting every band block and every repack; band blocks it could not use stay; a device limit defers as today (R19).
- [ ] A growth reserves the whole targeted demand's from-scratch pack times the shared margin, capped at all-resident and the device limit, and no separate growth headroom remains.
- [ ] After a growth of either pool alone, the next draw samples a block in a grown layer through the bound resources, with texels intact (R30).
- [ ] A second demand past a grown pool, after its retirement, grows it again and warns again (R26).
- [ ] A Settling set past the pool grows once, warns by name, reaches the reveal, and keeps its size for the level (R12).
- [ ] Growth of one pool leaves the other's generation alone, both growing on one drain retire independently, and the memory meter reports both pools, spare layers and retiring generations included, with one pool's release leaving the other's retiring bytes counted (R9).
- [ ] Build Summary's bytes equal the first-generation bytes the runtime allocates for that level: spares included on one level below the all-resident cap and one at it, and no spare in either pool on a level with no id 51.
- [ ] The walk harness's first layers and bytes equal the first generation the runtime allocates for that level, in both pools, with no size passed in.
- [ ] Pool policy identity differs from the pre-change string (grep gate), so measurement records never pair across policies.
- [ ] Rows that prove a GPU result run with an adapter required; a skip is not a pass.

### Manual
On the Mac 5300M, Metal, windowed, cold PRL rebaked at this commit, using `Metal Resource Events` and the 2 km hallway route at 7 m/s from `done/spatial-residency--sh-bounded-pool/research.md`, except Walks and Planar mirrors, which run in the no-GPU harness on the same PRLs.
- [ ] **Hallway ceiling.** Lightmap pools and total allocation recorded, steady and peak, against 2,170 and 2,574 MiB in the `resource-size` column, with no lightmap growth on the walk. Measure and report; the target is a finding, not a gate.
- [ ] **Walks.** Both walks in `research.md` at the shipped margin and one step either side: growths, reads per step and missed block-frames recorded against its table. A better step than 5/4 goes to the owner.
- [ ] **Frames.** Full-lighting shots at fixed poses match a pre-change build within each map's noise, excluding shots a mover explains, on the hallway, `campaign-test`, `occlusion-test`, `specular-shadowmask-capture` and `shadowmask-groups-capture`.
- [ ] **Repack cost.** Worst-case frame time and rate of each repack kind recorded on the hallway. A visible hitch goes to the owner.
- [ ] **Mask-heavy map.** Pool sizes and frames recorded on `shadowmask-groups-capture`, the case with fewest absent planes.
- [ ] **Theatrical moments.** A closet reveal and a door opening on a map with both: no dark frame, no growth, and no compaction during the reveal.
- [ ] **Planar mirrors.** `reflection-lake-test` and `campaign-test` walk at a planar tier at the shipped margin: growth recorded against the same walk with the mirror off, with each pool's first layers against its all-resident layers, since a pool at that cap grows only if compaction fails. Any growth goes to the owner.
- [ ] **Bound cost.** Compile time the bound adds on the hallway recorded, warm and cold. A warm build it visibly slows goes to the owner.
- [ ] **Cleanup.** Exit returns driver in-use memory to baseline, and level reload does not grow it.

## Path
Non-binding.
- **Riskiest assumption first.** First slice: compaction alone on today's single pool, in `LightmapPoolModel`, with the walk harness at about 28 to 30 layers. It tests the bound's premise and the repack's GPU cost before any format work. If compaction is a visible hitch, the next shape is budgeted incremental moves across drains.
- **Compaction shape.** Treat moves as a dependency graph: run any move whose destination is free, and stage a blocker into the spare to break a cycle. WebGPU copies within one texture need distinct subresources, so same-layer moves always stage. `schedule_repack_copies` and `gpu_mirror` are the seams.
- **Completeness.** The bound's proof rests on one spare per pool always being enough. No stuck case is known, and none is proved absent; the property test decides.
- **Pool seams.** `LightmapPoolModel`, `place_never_refused`, `growth_reserve_layers`, `GROWTH_HEADROOM` (retires for the shared margin), `POOL_POLICY_IDENTITY`, `PoolTextures`, `static_plan`, `DEFAULT_LIGHTMAP_POOL_CAP_LAYERS`, `LightmapLevers`. One placement type per pool under one model keeps the journal atomic. `place_never_refused` passes `repack` the drain's list today.
- **Sampling seams.** `resolve_lightmap_block`, `sample_shadowmask_atlas`, `lightmap_pool_uv`. Only the forward pass reads masks. The table is VERTEX-only and `VertexOutput` has spare locations, so a flat varying adds no fragment binding, which matters at the sampled-texture wall. The cost is one more varying; check it does not move the forward pass on the 5300M.
- **Read seams.** `LevelBlockMap::build` already handles a block with no mask range; `payload_from_pair_bytes` splits planes on the frame thread.
- **Compiler seams.** `lightmap_pool_gate` runs before the bake and stays; the mask check runs after it. Pack holds ids 22, 42 and 51, so the bound needs no new stage. The tallest-first order joins shared placement in `postretro-level-format`, because the compiler has `postretro-render-cpu` only as a dev dependency. The `shadowmask_atlas` memo needs a version bump. Precedent for the bound and its report: `sh_mandatory_bound`, `ShMandatoryReport`, `sh_pool_cost`. `DEFAULT_LEAD` is in `postretro-level-format`.
- **Disk.** A full Metal route records about 19 GB of raw `xctrace` data and leaves it on exit; budget 25 GB and delete it after.
- **Harnesses.** `walk_measurement/pool_mirror.rs` and the compiler dry run mirror the pool and need the split.
- **Rival shapes.** Downward-only repack with a large margin: less code, and layers are cheap once masks leave them. Rejected because 36 layers grew where 38 did not, so any margin is one route's luck. One allocator with mask backing on a prefix of layers: no second pool, but the journal is already single, and it blocks per-mover masks.
- **Doors.** `real_map_mirror` is the template if mirror cells ever join a bound. `E25--map-transitions` leaves shared versus per-level pools open; per-level bounds serve either. Its seam pre-commit settles a union, so its growth would land at the two-level peak: E25 must price that against both levels' bounds.
- `crates/renderer/src/lighting/lightmap/stream/tests.rs` and `crates/level-format/src/shadowmask_atlas.rs` are past 800 lines: split first.

## Wire format
All integers little-endian and unsigned. Old PRLs are refused; pre-release, no migration.

| Surface | Change |
|---|---|
| Id 42 tag | `SMB7` → `SMB8`. `SMB7` joins the retired tags refused as named re-bakes |
| Id 42 block record | `group_a_len` or `group_b_len` of 0 means the plane is absent, and its offset is 0. A nonzero length must equal the block's plane length. Present planes stay adjacent in record order |
| New id 58 `LightmapPoolBound` | Present when id 22 has blocks and id 51 exists. `u32 version` (= 1), `u32 worst_lightmap_layers`, `u32 worst_mask_layers`, at the default lead or the baked maximum where that is lower. Each is a maximum over all camera cells, taken per pool |

## Boundary inventory
| Name | Sides | Note |
|---|---|---|
| Plane presence | Compiler → id 42 → loader → block table → sampler | Absent reads zero; distinct from a missed block and the no-lightmap entry |
| `worst_lightmap_layers`, `worst_mask_layers` | Compiler → PRL → pool sizing | Same placement code and order on both sides |
| Pool margin | `postretro-level-format` → compiler report and runtime | One value in that crate, so reported bytes equal allocated bytes |
| Capture pool override | Capture scene → first generation; report → both pools' layers | One figure per pool, replacing `lightmap_pool_cap_layers`. Developer facing |
| Default lead | `postretro-level-format` → compiler and runtime | `DEFAULT_LEAD`, shared with the SH bound |
| Bound report | Compiler → Build Summary | Developer and modder facing log text, no script surface |

## Open questions
- Block table layout for the two plane addresses, within the VERTEX-only table — **delegated**: keep one `vec4` per block and add no fragment binding, unless packing cannot fit; the executor reports the layout, and why if it needs more, in the plan of record
