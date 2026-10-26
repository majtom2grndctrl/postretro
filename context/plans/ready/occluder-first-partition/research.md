# occluder-first-partition — research

Derivation behind `index.md`. Source read at 0380ba710. Measurements ran on the owner's checkout with the same walk (`PORTAL_WALK_EPOCH` 3 at both); tooling and outputs are in `measurement/`.

## Cause

`select_splitter` (`partition/brush_bsp.rs`) considers every plane of every candidate brush and scores it `spanning × SPLIT_PENALTY + |front − back| × IMBALANCE_PENALTY`, counting candidates by AABB routing. Nothing in the score knows a plane's area, axis or the brush's role. A pillar's side plane that spans few candidates wins early, splits the whole current region, and cuts open space floor to ceiling. Every later plane in that region is clipped by it, so the hall becomes columns.

On `occlusion-test.map` the retaining wall is brush 37: Quake `[0,1184,0]..[1024,1216,256]`, 26 × 0.81 × 6.5 m, under its 512 u (13 m) ceiling. It covers the middle 26 m of the hall's 52 m width, with a lane past each end. The red pillars are about 1.0 m square (brush 41 is 40 × 40 × 56 u). The over-the-wall pose (Quake 544,700, facing north, eye 1.5 m) is south of the wall, well below its top: the eye cannot see over it. The column cells beyond are reachable through the vertical portals in the gap between wall top and ceiling, so their floors and pillar tops draw.

## Measurements

### Offline: rect walk, exact oracle, ray-cast seen set

`portal-gran/summary.txt`, `decomp_out.txt`. R = rect walk cells, E = exact oracle cells, S = cells a ray cast from the eye hits. w* columns weight by triangles.

| Set | Poses | R | E | S | Share of over-draw from cell granularity (E − S) |
|---|---|---|---|---|---|
| All | 168 | 33.6 | 29.3 | 19.9 | 0.69 |
| E ≥ 20 | 33 | 147.3 | 127.6 | 80.8 | 0.70 |
| Hall, E ≥ 20 | 17 | 128.1 | 102.2 | 82.6 | 0.43 |
| Room 2 / passage, E ≥ 20 | 16 | 167.6 | 154.6 | 78.9 | 0.85 |

- Worst pose: passage (544,560), E 158, S 21.
- Over-the-wall pose, yaw 90: R 225, E 183, S 157. Cell-level S understates the waste there: the ceiling of a column is seen and its floor is hidden, but they share a cell.
- No oracle trip; worst oracle steps 9,796.

### Portal shape

`portal-gran/stats_*.txt`.

| Map | Cells | Portals | Walkable | Vertical walkable | Taller than 3× width |
|---|---|---|---|---|---|
| occlusion-test | 519 | 882 | 670 | 614 | 427 |
| campaign-test | 487 | 837 | 524 | 421 | 149 |
| stress-warren-hallway-inspection | 5,671 | 5,800 | 4,722 | 3,387 | 153 |

No portal under 0.1 m² survives on any of them (`MIN_PORTAL_AREA_M2`). `slivers_matched.txt` ties each of the highest-aspect occlusion-test portals to the brush faces coplanar with it.

### Live frame cost

`frame-cost/results/`. Dev + dev-tools build, Metal Radeon Pro 5300M, vsync, stationary, nine CPU-timing windows per run after warm-up. Variant bakes used `--lightmap-density 0.025`, which byte-matches the repo bake.

Over-the-wall pose (P1):

| | Base | Wall raised to ceiling | Red pillars removed |
|---|---|---|---|
| Walk | 0.53 ms | 0.06 ms | 0.10 ms |
| Portals tested | 1,880 | 181 | 351 |
| render_submit | 5.37 ms | 4.36 ms | 4.50 ms |
| Work CPU | 8.3 ms | 7.0 ms | 6.8 ms |
| Cells in map | 519 | 517 | 190 |

- Every pose held 60 fps. The owner's drop was not reproduced standing still.
- GPU was about 12.7 of 16.7 ms, led by the fog raymarch and the skinned viewmodel, not occlusion. Shadows and streaming were negligible.
- Likely cause of the drop while moving: vsync quantization on thin GPU headroom, plus the walk and submit excess. Unconfirmed: there is no scripted camera path.

## Shape

### Every brush stays in the tree

A region is solid only when one brush contains it (`brush_contains_region`). With no qualifying splitter, `build_recursive` makes the region an empty leaf. Dropping a brush's planes therefore makes its interior empty, which breaks:

- solidity (`SolidCells`, `crates/physics/src/collision/solid_cells.rs`, which also treats exterior cells as blocking);
- hidden-face culling (`face_extract.rs` `clip_side_by_tree`, `put_hull_into_areas`, `PlaneMatch` routing);
- buried lights (`buried_lights.rs` `sphere_wholly_in_solid`);
- every bake that point-classifies against the tree (`sh_bake`, `sdf_bake`, `reflection_probes::capture_point`).

Brush-side projection re-splits each side at every node plane it crosses, and nothing re-merges coplanar fragments. Face count therefore follows plane order as well as plane count: an occluder plane chosen high in the tree crosses open space and splits every face it passes. Lightmap charts are per face, so chart count follows face count.

### Cells stay convex

Merging leaves after the BSP into non-convex cells is ruled out by its consumers:

- `crates/level-loader/src/portal_planes.rs` orients portals from the mean of a cell's portal centroids ("Cells are convex…");
- the walk's facing rule (`rendering_pipeline.md` §2 step 6) relies on that orientation;
- `cell_residency_bake/pvs_sampling.rs` `eye_points` samples a lattice over cell bounds;
- `cell_residency_bake/teleport_routes.rs` builds one hull per leaf from `defining_planes` and asserts leaf count equals cell count.

An occluder-tier leaf is convex, and portals lie only on its boundary, so all of them keep working. The empty part of the cell is not convex where exempt solids sit inside it; consumers that sample by point location already skip solid space.

### Rivals

| Shape | Why not, or when |
|---|---|
| Splitter score only: one leaf per cell, plus an occluder and area term in `select_splitter` | Strongest rival. The wall-top plane would likely win early and fix over-the-wall, but pillar planes still split their own room into columns, so the in-room portal count stays. |
| Drop small brushes' planes | Breaks solidity and every reader above. |
| Merge leaves after the BSP | Non-convex cells; breaks orientation, facing, sampling, teleport hulls. |
| Convex-only leaf merge after the BSP | Collapses parallel slivers, but a cell cannot contain an exempt solid, so pillar rooms keep their columns, and over-the-wall still needs the area term. |
| Two BSPs (occluder-only for cells, full for solidity) | Full-tree leaves straddle occluder-tree boundaries, so face and solid ownership needs clipping and point reconciliation; one tree nests them for free. |
| Hull-plane splitters for stacked and stepped solids | Pre-decided escalation if the arena sweep shows slicing; additive to clusters (§Stacked and stepped solids). |
| Size-cap splitters, like q3map's blocksize | Non-goal and named follow-up (§Cell size). |
| Runtime occluder culling | Owner ruling: the baker is the fix. Adds per-frame cost to save per-frame cost. |
| Full PVS bake | Owner ruling (no Quake-style PVS); `rendering_pipeline.md` §2 rejects precomputed visibility. |
| Hint brushes / hand-placed portals | Authoring surface; the industry account in `index.md` §Problem is the cost this brief removes automatically. |

Precedent: Quake 3's structural/detail split. Structural brushes build the BSP and portals; detail brushes are clipped into existing leaves and never create portals. Here the role is derived, not authored.

**Area term.** A score that ranks by brush-face area picks the retaining wall's back face (26 × 6.5 m) before its top (26 × 0.81 m). The region beyond the wall then stays one floor-to-ceiling cell reached over the wall, and the occlusion row fails. Ranking by the plane's cross-section through the current region picks the top first.

### Divergence from portal-walk-bounded-regions

Its non-goal "Compiler detail brushes or cell merging" rejected changing cell granularity for fog masks, SH clusters, lightmap blocks and draw indexing, and an authoring surface. The bounded-region walk made cost per visible cell, but every sliver is still a visible cell with portals: at the over-the-wall pose the base walk tests far more portals than the wall-raised variant (§Live frame cost). This brief adds no authoring surface and remaps the cell-keyed consumers once at the compiler, so the cost that non-goal priced is paid here deliberately.

### Classification precedents

- `done/reflection-probe-capture-exclusion` (`resource_management.md` §5.2 Exclusion): automatic size-based brush classification, measured on the axis-aligned hull, relative to the space it serves, with initial anchors, tuned visually; override deferred to a `func_group` brush entity. Its `_reflection_capture` keeps `auto` as a presented value; the owner's rule for any future `_occluder` key is the opposite (absent means automatic, `auto` never presented). Retrofitting `_reflection_capture` is outside this brief. Its axis-aligned hull does not carry over (§Rotated brushes).
- `done/perf-baked-visibility-region-masks`: anything that can move, open or break is modelled open; permanent structural brushes may reduce visibility. Movers are peeled off before static BSP construction, so they never classify as occluders; a closed door seals at runtime only, through doorway portals the occluder tier keeps (§Doors).

## Occluder rule

### Constants

- **Floor, 1.2 m.** Player capsule radius 0.2 m (`content/dev/scripts/player.ts`), so 0.4 m wide; the floor is three player widths. A broad axis shorter than that hides little more than a player. The same floor is the least depth of open space in front of a face that counts: a face fronting a crack faces no space a player stands in, so a column's sides in a tight niche cannot make it occlude.
- **Horizontal: 20 m, or 50% of the open span.** Occlusion value across the view scales with the share of the space a brush blocks, not its size alone: a 2.84 m slab fills little of a shaft, and a deck fills its corridor. The absolute length passes a long wall or building where the open span is too wide for the share, as along an open street.
- **Vertical: see-over height, 2.5 m.** Height asks whether a player can see over the brush, which depends on eye height, not on the distance to the ceiling or sky; the 1.5 m eye height is `player.ts`'s capsule and eye offset. An open-air maze of 3 m walls must occlude, though its sky is far above. Over-classifying is safe: an occluder never hides anything, it only adds cells, and the wall-top plane leaves a portal for elevated views. The test reads how far the top stands above the floor in front of the face, so a high beam whose broad axes both reach 1.2 m passes it too.
- **Movement proxy.** How high a viewer gets depends on movement the compiler cannot read: jump height, double jumps, wall-running, rocket packs. The see-over height is the per-engine proxy for it; a game whose movement reaches past it is the case for the per-project setting the brief defers.
- **Slabs.** A brush whose two broad axes are both horizontal, a deck or platform, takes the horizontal test on each.
- **Both broad axes.** Any single axis passing would make a floor-to-ceiling column (full height, narrow), a long curb or a pipe (long, thin) occlude, and each passes on one axis only.
- **Either broad face.** A wall standing in a larger space sees its room from one face and the space around it from the other. Requiring both faces to pass would exempt every wall of a room standing in a hall and merge the room into the hall's cell (pin R31).

### Available space

Owner-confirmed reading: a horizontal broad axis's span is measured in the open space in front of the brush's (or cluster section's) broad face, not along the brush's own line, bounded by solid or by exterior void, which counts as blocking. A room wall then compares to its room's extent, a deck's cross span to its corridor, a shaft platform to its shaft. The vertical test reads no span.

A line along the brush's own length fails the most common case. Box rooms built from slabs that meet only edge to edge, as in occlusion-test's first room, leave an empty corner column, so each wall's line runs past its ends into the void. Read as unbounded space, the relative test fails, every wall shorter than 20 m goes exempt, and two rooms sharing a wall merge into one cell (pin R30).

Counting exterior void as blocking needs exterior classification before the occluder tier is built. Today's full tree and its exterior flood can supply it as a first pass; that is a Path option, not a Decision.

Open questions the executor settles: where across the face the span is taken (centre or worst case), which brushes count as blocking (occluders only, or every brush), and which test a sloped broad axis takes. The pass/fail examples pin the outcome of the first two whatever reading is chosen; no row pins the third.

**Mutual dependence.** If only occluders count as blocking, one brush's class can depend on another's. In an L, only the wall abutting the other's side can have its span ended by it, so an L tests a dependence one deep, which a single pass in brush order passes in either order. A chain tests two deep, in a hall 40 m by 25 m and 13 m tall, all its walls 7 m tall, so each passes the see-over test:

- A 21 m wall runs along the 25 m axis, 2 m from each end wall, and occludes on the absolute length.
- A 12 m wall abuts its side 8.5 m from one end and runs along the 40 m axis to a free end 2.5 m from the hall wall.
- A third 12 m wall abuts the second's side at its midpoint and runs along the 25 m axis to a free end 2.5 m from the hall wall.

Every line in front of either face of a 12 m wall meets the wall it abuts; with the third flush with the second's free end instead, its outer face would see the hall wall to wall. Visited first to last, each 12 m wall sees 12 of 14.5 m (83%) and occludes. Visited last first, the third sees 12 of 25 m (48%) and the second 12 of 40 m (30%), and both stay exempt. Pin R28 holds every chain wall occluding in every order, whatever counts as blocking.

### Rotated brushes

The precedent's axis-aligned hull cannot serve. A 25 m × 0.3 m × 0.3 m curb turned 45° in plan has a hull about 17.9 × 17.9 × 0.3 m, a slab; in a room 30 m across on both horizontal axes, both its axes pass half the span (60%), so it would occlude. In a space wider than about 35.8 m on either axis that axis fails and a world-axis build passes, so the pin fixes the room. Pin R5 holds the curb exempt and the turned wall occluding, which rules that reading out: extents follow the brush's own geometry, not the world axes.

Clusters too. A 30 m × 15 m strip facade turned 45° has a world footprint of about 21 × 21 m; on world axes its two largest axes are horizontal, its broad face is the footprint, coverage is tiny, and it goes exempt. Pin R23 holds it bounding cells, so a cluster's extents and coverage are measured in its own frame.

### Owner examples at the starting constants

Measures match the Acceptance classification table, which is authoritative.

| Example | Broad axes (m) | Floor | Horizontal | Vertical | Result |
|---|---|---|---|---|---|
| Retaining wall | 26 long × 6.5 tall | pass | 26 ≥ 20 | 6.5 ≥ 2.5 | occludes |
| Shaft stair platform | 2.84 × 2.84 slab, in a 9.75 m shaft | pass | 2.84 of 9.75 = 29%, on both | slab | exempt |
| Floor-to-ceiling column | full height × 1.5, in a 13 m wide hall | pass | 1.5 of 13 or more = 12% or less | pass | exempt |
| Column in a niche | full height × 1.5, flush in a niche 2 m wide, hall wall 13 m long | pass | front face: 1.5 of 13 = 12%; side faces front 0.25 m and the back face none, under the floor, so they do not count | pass | exempt |
| Columns down a corridor | full height × 1.5, corridor 6 m wide | pass | faces along the corridor: 1.5 of 6 = 25%; faces across it: 1.5 of the corridor's length | pass | exempt |
| Lamp pole | 5 × 0.2 | fails | — | — | exempt |
| Curb | 25 × 0.3 | fails | — | — | exempt |
| Street deck | 30 × 4 slab, 5 m above a corridor 4 m wide | pass | 30 ≥ 20; 4 of 4 = 100% | slab | occludes |
| Red pillar | 3.7 × 1.0 | fails | — | — | exempt |
| Maze wall | 12 long × 3 tall, corridors 2 m wide, sky 100 m up | pass | each face's corridor runs 2 m past the free end to the boundary wall: 12 of 14 = 86% | 3 ≥ 2.5 | occludes |
| Maze railing | corridor length × 1 tall | fails | — | — | exempt |

Shaft platforms are `STAIR_HALF` 56 u (112 u square) and `STAIR_THICK` 24 u in `tools/gen_stress_map.py` `emit_shaft_stairs`, in a `SHAFT` 384 u hole. The top landing is larger and touches the floor rim, which occludes alone and so never joins its cluster.

Other fixtures at the same constants. Measures not given here are in the Acceptance rows; rotated fixtures are in §Rotated brushes, the chain in §Available space and the arena run below.

| Fixture | Arithmetic | Result |
|---|---|---|
| 10 m cube, room 18 × 30 m (R18) | top face, a slab: 10 of 18 = 56%, 10 of 30 = 33%; faces whose horizontal axis runs the 18 m width: 56%, 10 ≥ 2.5; faces whose horizontal axis runs the 30 m length: 33% | the tie-break decides; brush order cannot |
| R30 synthetic rooms | each slab's inner face: 8 of 8 = 100%; walls 4 ≥ 2.5; outer faces front void and do not count | each occludes |
| R30, occlusion-test first room | walls 2, 6, 7: 6.5 of 6.5 m, 3.25 ≥ 2.5; east cluster {3, 4, 5}: face 6.5 × 3.25, coverage 81%; ceiling 1: its x span ends at lintel 5. Floor 0: its x span runs through the east doorway and the passage to room 2's east wall, 7.32 of 17.07 m = 43%, under 20 | walls and ceiling occlude; the floor's class is not pinned |
| Room in a hall (R31), hall 40 m square | inner faces: 8 of 8 = 100%, 4 ≥ 2.5; outer faces: 8 of 40 = 20%; roof's inner face 8 of 8 on both axes | each wall and the roof occlude from the inner face |
| occlusion-test (R24) | wall 37 as the retaining wall; red pillars 1.016 m square, under the floor | wall occludes; pillars exempt |
| Hinted dividers (R27) | pieces 2.03 and 1.63 m across a room 5.69 m wide, 36% and 29%; lintel 2.03 m, 36%; cluster face 5.69 × 6.1: 100%, 6.1 ≥ 2.5, coverage 76% | pieces exempt; the cluster occludes |
| Strip facade arm (R13, R23, R26) | 30 × 15: 30 ≥ 20, 15 ≥ 2.5; coverage in §Clusters | each section occludes |
| L lamp post in an alcove 2.5 m wide | pole, arm and head each under the floor on a broad axis; bounding face 1.5 × 5: 1.5 of 2.5 = 60%, 5 ≥ 2.5; coverage in §Clusters | exempt |
| Colonnade under a beam | columns and beam each under the floor on a broad axis; bounding face passes by the row's precondition; coverage in §Clusters | exempt |
| R1, R3, R8, R15, R22 | every brush under the floor on a broad axis | exempt |

**Arena-staircase margin.** The arena staircase (`emit_walk_stairs`: `STAIR_WALK_RISE` 12 u, `STAIR_WALK_DEPTH` 28 u, climbing `STORY_H` + `SLAB_T`/2 = 320 u, 2 × `ARENA_STAIR_HALF` wide) is a run 19.2 m long, 8.2 m high and 6.5 m wide. Each step's broad face spans the run's width, 6.5 m of a hall over 13 m wide, under half, so each step is exempt alone however tall. The run's broad face is the side profile: its coverage, about 52%, passes the 50% start, and its 8.2 m height passes the see-over test. It stays exempt only because 19.2 m is under the 20 m absolute length and under half the arena's span. An absolute constant at or under 19.2 m lets it occlude, so its riser and tread planes slice the arena and fire the stacked-solid escalation. Pin R19 catches that in a fixture before the sweep.

## Clusters

A facade of narrow strips between windows must occlude as one, so touching exempt brushes are judged together. These readings fail:

- If occluder brushes join clusters, every pillar joins the floor it stands on and occludes. Clusters form only among brushes each exempt alone.
- If a cluster is judged by its bounding extents alone, an L-shaped lamp post and a colonnade under a beam both occlude. Their solid covers a small share of their bounding face; a facade with windows covers most of it.

Coverage: the area of the cluster's solid projected onto its bounding broad face, over that face's area. Rough shares for the owner's cases: an L lamp post (pole 0.2 × 5 m, arm 1.5 m) under a fifth; a colonnade of 1 m columns at 4 m centres under a slim beam about a third; a facade with a third of its area in windows about two thirds. A starting constant of 50% sits between the colonnade and the facade and reads "mostly solid" as more than half; tuning is the executor's on the street map's facades and the fixtures.

**Membership.** A brush that occludes alone stays out, but an exempt fixture touching a cluster joins it. A pillar on a floor of touching exempt tiles joins the floor's cluster, which covers its face and occludes, so the pillar's planes reach the occluder tier and slice the room into columns again; a wall lamp touching a strip facade slices the street the same way. Plane selection has no locality term to contain this. Candidate rules: only members lying within the cluster's broad-face slab reach the tier; or each member is judged against the cluster without it, and a member whose removal still lets the cluster pass stays exempt. Pin R4 holds either.

**Sections.** A cluster need not be planar. An L of two 30 m strip-facade arms, 15 m tall, has two horizontal largest axes, so its bounding broad face is the footprint and its coverage a few percent: exempt as one cluster. A touching exempt sign joining two facades across a street does the same to both. The street map's abutting row snakes around corners, so each near-planar section of a cluster is judged on its own broad face (pin R26). How sections are found is delegated with membership.

**Touching.** Brush-side projection treats a gap of 1 mm or less as contact (`brush_bsp.rs`; `resource_management.md` §5.2 Contact). The same epsilon for clusters keeps strips a hair apart from grid rounding in one facade. The spaced-strips row depends on this rule.

## Stacked and stepped solids

The arena stress map is `content/dev/maps/stress-warren-showcase.map`, generated by `tools/gen_stress_map.py --preset warren` (`PRESETS["warren"]`, `arenas=1`). `emit_arena` calls `emit_walk_stairs`, which emits each step as a full-height box from the floor to its own tread, touching its neighbours, so the run is a stepped solid of `STAIR_WALK_RISE` 12 u risers and `STAIR_WALK_DEPTH` 28 u treads, 2 × `ARENA_STAIR_HALF` wide.

On paper each step is exempt alone and the run's cluster fails the rule inside an arena of football-field interior (margin: §Owner examples, arena-staircase margin), so no riser or tread plane reaches the occluder tier. If the sweep shows otherwise, or a smaller room makes the cluster occlude, every riser and tread plane becomes an occluder-tier splitter and slices the room. The pre-decided escalation is a further role: such a solid occludes through its outer hull's planes, fed as non-solid splitters through the doors' path, while its brushes stay below the tier. Hull planes alone would lose the windowed facade: its hull face spans the windows, so it would yield one portal through the whole facade. The role is therefore added for stacked and stepped solids, not swapped in for clusters.

## Cell size

An occluder-tier cell has no size limit. A long street or a hall of exempt pillars can become one cell. Cluster partition caps are `DEFAULT_PRIMITIVE_LIMIT` (64 BVH leaves) and `DEFAULT_CELL_LIMIT` (32 cells) in `cluster_directory_bake.rs`; a cell over the leaf cap becomes a singleton cluster flagged `CLUSTER_FLAG_INDIVISIBLE_OVERSIZE`, so its SH and lightmap residency cannot divide. The flag fires only when that cell alone exceeds the leaf cap (`canonical_partition.rs`), so a large cell with sparse geometry, the street case, stays unflagged and still cannot divide; the oversize count under-reports coarse cells, and the cell size distribution is reported beside it. CellVisibility rows, keyed per cell, coarsen with it.

q3map's blocksize splits the world on a fixed grid before structural splitting. Here the equivalent is non-solid size-cap splitters through the same path doors use. It is deferred to the follow-up `occluder-cell-size-cap`, opened if the cell sizes, oversize-cluster counts or id-51 misses the sweep reports call for it.

## Bake gap and id-51 sampling

`rendering_pipeline.md` §2 holds any change that can alter a visible set to the bake-gap gate, worded for a walk change over one bake: the pool defers no more blocks than under the walk it replaces. The walk's algorithm stays, but its cells and portals change on every map, so every visible set changes and the block count stops comparing:

- The instrument is `walk_measurement` (`crates/postretro/src/session/lightmap_residency/`) in its completed-reads mode, as `done/portal-walk-bounded-regions` ran it. It counts drawn non-resident blocks per frame (`walk_measurement/report.rs`).
- A block is one cell's charts packed up to the pool-layer edge; a cell opens another block only when one will not fit (`lightmap_bake/cell_blocks.rs`). Coarser cells give fewer, larger blocks, so one deferred block after the change can hold the texels of many before it. A block count can pass a regression or fail an improvement; deferred texel area per frame (deferred blocks' area summed) compares across partitions.
- Pairing is cell-derived. Walks step along portal neighbours (`walk_measurement/paths.rs`). The paired-run record fingerprints each frame from step, walk cell, eye, yaw, pitch and projection (`trip_record.rs`), stores only the fingerprint, and marks a pair invalid when locates differ (`walk_measurement/mod.rs`, which defines a pose only over one cell layout). So the before run records each frame's eye, yaw, pitch and projection, the after run replays them at the same world positions, and a pair is valid when those match frame for frame; walk cells and located cells are not compared.
- The ray-cast guard has the same need. `real_map_sweep` takes poses from cell centres and casts no rays; the portal-gran snapshot casts a ray grid over fixed occlusion-test poses only (`measurement/portal-gran/gran/src/gran.rs`). World-space poses and rays extend to every swept map in test modules or tools, outside the walk's runtime code.

The id-51 bake samples the walk from a fixed lattice per cell (`cell_residency_bake/pvs_sampling.rs`, `LATTICE_FRACTIONS` 0.1, 0.5, 0.9 of the cell AABB per axis). Coarser cells get the same lattice over more volume, so an eye between lattice points can see cells outside the baked set. Those are id-51 misses: drawn cells outside the camera cell's baked set, never-refused visible demand that the bake-gap gate charges. Scaling the lattice with cell size is the lever; the measured misses decide it.

## Wire format

No PRL layout or version changes. The runtime already reads cell ids everywhere:

- Geometry face `leaf_index` is documented in the loader as a runtime cell id; the loader requires it to match the cell whose face range covers the face (`validate_cells_against_geometry`).
- BVH leaf records carry `cell_id`, which `bvh_build` copies from the face's cell.
- CellLocator children are `Cell(id)`, validated only as `< cell_count` (`CellLocatorSection::from_bytes`); nothing requires one leaf per cell. The compiler passes the leaf count today.
- AlphaLights carry the light's cell, loaded as `cell_index`.
- Portal endpoints are read as cell ids, but their wire fields keep leaf-era names: `front_leaf` and `back_leaf` in level-format `portals.rs`, packed from the compiler portal's `front_leaf` in `pack/spatial.rs`.

Leaf id = cell id is a compiler identity (`encode_cells`: "Cell ids stay one-to-one with BSP leaf ids"), and docs, comments and field names also state it as the format and loader contract: `build_pipeline.md` §PRL section IDs (Cells: "without a runtime remap"), `section_registry.rs`, level-format `cells.rs`, `geometry.rs` and `portals.rs`, and loader `prl.rs`. No loader check enforces it, so the layouts and version constants hold, and those texts are restated at landing. BSP nodes and leaves are not emitted (ids 12 and 13 are retained only to detect stale PRLs).

## Leaf-id consumers

Read as cell ids today, remapped through the leaf → cell map:

| Consumer | Section / use |
|---|---|
| `visibility::encode_leaves` → `pack::spatial::encode_cells` | Cells id 38: one contiguous face range per cell |
| `geometry::build_leaf_ordered_faces` | Geometry face cell, cell-major face order |
| `cell_draw_index_bake` | CellDrawIndex id 37 |
| `lightmap_bake/cell_blocks.rs` (by `chart.leaf_index`) | Lightmap cell blocks id 22 |
| `fog_cell_masks` (iterates `tree.leaves`) | FogCellMasks id 31 |
| `cell_visibility_bake` (tree + generated portals) | CellVisibility id 46 |
| `cell_residency_bake::teleport_routes` | id 51 teleport routes; leaf-count assertion and per-leaf hulls |
| `bvh_build` | BVH leaf cell |
| `pack::spatial::encode_cell_locator` | CellLocator id 39; validated against leaf count |
| `kinematic_geometry::sealed_portal_ids_for_mover` | Portal ids in KinematicGeometry id 43 |
| `pack.rs` AlphaLights (`find_leaf_for_point`) | Light cell in id 18 |
| `affinity_grid::reachable_leaves` | Floods generated portals from a `find_leaf_for_point` leaf |
| `chunk_light_list_bake` per-light flood | Same pattern, id 23 |
| `streaming_hints::resolve_streaming_hints` | Seam portal ids (id 49/50 partition), resident and priority cells by AABB; must read the runtime (occluder-tier) portal set |

The affinity-grid and chunk light-list floods need one id space for the seed and the portal graph they flood. Readers that only point-classify solidity keep reading the full tree. The loader's `validate_portal_adjacency` rejects a portal whose two sides are one cell, so portals inside an occluder-tier leaf must not be emitted.

## Exterior sub-leaves

An exempt brush can be the only thing between an interior room and the void (a plugged window). Its occluder-tier leaf then holds both interior and exterior sub-leaves. Flagging the whole cell exterior strips the room's faces and makes it block movement; flagging it interior lets the void read as walkable. Keeping exterior sub-leaves as their own exterior cells, like solid sub-leaves, avoids both.

## Doors

`sealed_portal_ids_for_mover` seals a portal only when the closed mover's brush union covers it fully. Mover brushes are not in the static BSP; doorway portals come from the planes of the surrounding wall brushes. A door set in a wall of exempt brushes would get one coarse portal the door covers only partly, so it would stop sealing. Candidate fixes: inject the closed-hull planes of each mover whose closed pose seals a doorway opening as non-solid occluder-tier splitters, or promote the brushes framing such a mover into the occluder tier. The non-solid splitter path is the one stepped-solid hulls and a size cap would reuse.

Recognising such a mover is itself open. Doors and lifts share one class, `kinematic_mover` (`sdk/TrenchBroom/postretro.fgd`). The only sealing test, `sealed_portal_ids_for_mover`, runs after the BSP against generated portals and requires full coverage, and those portals are what the classification now shapes: for the door in an exempt wall it answers "seals nothing". Options: a pre-BSP test that the closed hull, with the static brushes it touches, closes an opening; or a two-pass build that partitions, tests sealing, then re-partitions with the sealing movers' hull planes.

`sealed_portal_ids_for_mover` runs for every mover, not just doors, and the warren maps carry multi-brush lift cars (`tools/gen_stress_map.py`, `LIFT_HALF`). A lift car or platform seals no doorway; its hull planes would extend across the room it docks in and slice it into cells again, so it feeds no splitter (pin R25). The Decision fixes only the outcome; the mechanism stays open in Path. Today a lift car may seal a sliver portal its brush union happens to cover; after the change that sliver is gone, so the swept-map sealing guard compares doors only.

A wall that stays exempt has gaps, covers less of its face than the coverage constant, or has a bounding face that fails the occluder rule. With the door closed the walk can still reach cells beyond through those gaps, over the wall or around it, so on such a wall the door row tests the doorway itself: with the door closed, points either side of it lie in different cells, and at least one portal crosses the opening and seals (pin R8). Without the first two clauses a build that adds no doorway boundary passes vacuously. "Reaches no cell beyond the doorway" holds on the existing door fixtures, whose walls occlude. Movers are peeled off before classification, so a door never counts as a blocking surface for its wall's span (pin R17).

## Streaming hints

Hints are peeled before BSP and never block a door or change visibility (`build_pipeline.md` §Custom FGD and §Entity resolution; the FGD says the same). That commitment stands: no hint plane reaches classification, the BSP or portal generation, so cells, portals and visible sets match the same map with its hints removed (pin R29).

`streaming_hints::resolve_streaming_hints` requires the generated and packed portal lists to match one to one, and fails the build by name when a seam volume matches no generated portal with through-hull overlap. After the change the runtime portal set is the occluder tier only, while the full tree's portals live on for the exterior flood, so both lists must be the runtime set. A seam across an opening in a wall that classifies exempt and forms no occluding cluster matches no runtime portal and fails the build by name, as any unmatched seam does.

The committed fixtures placing seams are `sh-streaming-hinted-door.map` and `lightmap-runtime-floor-verification.map`. Each divider is touching pieces spanning its room wall to wall, exempt alone, that occlude as a cluster (§Owner examples), so its seam still matches (pin R27). The committed `test-fixtures/sh-streaming-hinted-door.prl` byte check needs regenerating. Each seam overhangs its divider by about 0.1 m on both faces, so seam planes fed as splitters would slice a slab off each room; R29 is the guard against that. Resident volumes pin every cell they overlap by AABB, so a small volume can pin a whole coarse cell such as a street; the per-map report lists them.

## `MIN_PORTAL_AREA_M2`

`portals.rs` drops every generated portal polygon under 0.1 m² in `make_node_portal` and `distribute_portal`, and `find_exterior_leaves` floods only survivors. The cull stays as it is on every portal set, the full tree's and the occluder tier's. Retiring it is the follow-up `portal-area-cut-retirement`, kept apart so the measurements separate the partition's effect on portals from the cull's. The cull already hides real sightlines (`done/portal-walk-bounded-regions/plan.md`, capture A/B, lake slot); that stays until the follow-up.

Leak detection keeps its rule, but not every outcome. The full tree's plane order changes, so a narrow gap's portal fragments differently: an occluder plane chosen at an ancestor node can cut a gap just over the cull into pieces under it, and a crack today's order cuts below the cull can come out whole. Leak status near the cull can flip either way, so no build can promise it there. The runtime portal set passes through the same cull, so a sightline through a crack near it can lose or gain its portal the same way. Owner ruling: such a ray-cast miss or leak flip, through an opening under four times the cull area, is reported, not failed. Pin R14 holds what a correct build can: a leak through an opening of at least four times the cull area, and sealed fixtures.

## Pin table

Orderings and edges the Acceptance rows prove; a row citing a pin is its test.

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| R1 | Sealed map, every brush exempt: brushes under 1.2 m on a broad axis, e.g. a 1.1 × 1.1 × 3 m closet | Classification finds no occluder; the occluder tier makes no split; the full-tree flood runs | One interior cell, plus solid and exterior cells; no runtime portal; leak result as the pre-change build; the walk takes the no-portals fallback; id 51 omitted with its log; build succeeds unless the all-resident floor fails by name |
| R2 | Every brush occludes | Classification, then a tier that holds the whole tree | Each empty BSP leaf is its own cell; every consumer reads its leaf's own id |
| R3 | Occluder-bounded space filled by touching exempt brushes, none filling it alone, each under 1.2 m on a broad axis, e.g. a 1.1 × 1.1 × 3 m closet | Occluder tier stops, then sub-splitting makes only solid sub-leaves | Solid: no portal leads into it, the walk never passes through it; the level loads |
| R4 | Exempt pillar on a floor of touching exempt tiles; exempt lamp touching a strip facade | Individual pass, then cluster pass, then membership | Pillar and lamp add no portal of their own; floor and facade still bound cells (§Clusters, membership) |
| R5 | Brush turned 45° in plan: the 25 m curb in a room 30 m across on both horizontal axes, where its world-axis box would pass both broad axes; the retaining-wall fixture, hall and all | Extents measured, then the rule applies | 25 m curb stays exempt; the wall fixture, hall and all, still occludes (§Rotated brushes) |
| R6 | Same map with its brushes reordered | Classification and clustering visit brushes in a different order | Same occluders, same clusters |
| R7 | Rooms A, B, C in a row behind occluding walls; door between A and B | The door's sealing boundary joins the occluder tier | Room C's cells and portals match the map without the door |
| R8 | Door 1.5 m wide between two wall pieces, each 1 m wide and touching only the floor and ceiling, so the wall stays exempt as a whole | The door's sealing boundary joins the occluder tier; sealing reads the runtime portals | Closed: points either side in different cells; at least one portal crosses the opening; each such portal blocked. Open: each passes (§Doors) |
| R9 | Teleporter region crossing several leaves of one cell | Cells grouped; then id-51 hulls; then couplings | That cell named once for source and for seamless arrival |
| R10 | Light exactly on an occluder plane in open space | Locate the light, then flood | Light records the plane's front cell; its flood reaches the back cell |
| R11 | Light within 1 mm outside an exempt pillar face | Buried-light test on the full tree; then locate | Light kept; records the empty cell around the pillar, never a solid cell |
| R12 | Brush exactly at the floor, the absolute length, half its span or the see-over height; a face fronting open space exactly the floor deep; cluster exactly at the coverage constant | Rule comparison | Occluder tests inclusive, so each passes and the face counts; coverage strict, so the cluster stays exempt |
| R13 | Exempt brushes touching only through an occluding floor | Individual pass first; the floor never joins | No cluster forms; each judged alone |
| R14 | Leak through an opening at least four times the minimum portal area; a sealed fixture; a swept map before and after | Full tree reordered; cull unchanged; flood over surviving full-tree portals | Leak still reported; sealed fixture reports sealed. A gap just over the cull may fragment below it, so a leak flip or ray-cast miss through an opening under four times the cull area is reported, not failed (§`MIN_PORTAL_AREA_M2`) |
| R15 | Alcove 1 m wide, under the floor constant, sealed from the void only by an exempt back wall | Full-tree flood marks the void sub-leaves exterior; cells split interior from exterior; portals attach | The alcove's portal joins its interior cell; the walk from the room reaches it |
| R16 | Same leaves, different grouping; or, with geometry held fixed, a classification changed by a test-only constant override | Classified and cells grouped, then cache keys computed | Every cell- or portal-reading stage misses; an unchanged rebuild hits |
| R17 | Same doorway with and without a door entity | Movers peeled off at parse; then classification | The wall's class is unchanged (§Doors) |
| R18 | Brush whose largest axes tie, as a cube | Choosing the two broad axes, visiting brushes or its sides in a different order | Tie-break delegated; whatever it is, classification is independent of brush order |
| R19 | Arena staircase run in a hall over 40 m long and over 13 m wide | Individual pass, then cluster pass | Exempt; no riser or tread plane reaches the occluder tier (§Owner examples, arena-staircase margin) |
| R20 | Cell spanning several BSP leaves; a fog volume touching only one of them | Leaves grouped into cells; then fog masks, face order, draw index, lightmap blocks, cell bounds and the cell-visibility bake read the grouping | The cell's fog bit is set; its face range, draw list and blocks hold every leaf's faces; its bounds enclose every leaf; portal-joined cells share a reachability component |
| R21 | Face whose chart exceeds one lightmap pool layer, in a cell spanning several BSP leaves | Leaves grouped and faces ordered cell-major; then the oversize-face cut rebuilds face ranges, BVH and draw index | Every sub-face keeps its parent's cell; each cell's face range and blocks stay contiguous; the level loads |
| R22 | Exempt plug flush with an occluder plane, in openings 1 m wide, under the floor constant: in a window to the void, and in an inner doorway thinner than the wall | Occluder-tier portals generated; full-tree flood marks exterior sub-leaves; sub-leaves grouped into cells; then each portal's two cells assigned | No portal names a solid cell; none joins an interior cell to an exterior cell; the level loads; the walk may reach the doorway's recess through the plug (over-inclusion only) |
| R23 | Strip facade turned 45° in plan | Cluster formed; its extents and coverage measured in the cluster's own frame, not world axes; then the rule applies | Bounds cells as the unturned facade does; spaced strips stay exempt (§Rotated brushes) |
| R24 | occlusion-test.map's retaining wall and red pillars, read from the map; the pillars 1.016 m square, just under the floor | Parse and unit scale; then extents, open span and the floor in front measured; then the rule applies | The wall occludes; each red pillar stays exempt (§Owner examples) |
| R25 | Lift car docked in a room below its shaft, away from any wall opening | Movers peeled; only movers whose closed pose seals a doorway opening feed occluder-tier splitters | The room's cells and portals match the map without the car (§Doors) |
| R26 | Strip facade turning a 90° corner, arms 30 m, 15 m tall, under a ceiling 25 m up; two strip facades joined by a touching exempt sign across the street | Cluster formed; split into near-planar sections; then each section's broad face and coverage measured | Each arm and each facade bounds cells (§Clusters, sections) |
| R27 | The committed hinted fixtures `sh-streaming-hinted-door.map` and `lightmap-runtime-floor-verification.map` | Hints peeled before BSP; classification; occluder-tier portals generated; then hints resolve against runtime portals | Each fixture builds and loads; each seam matches a runtime portal (§Streaming hints) |
| R28 | Chain in a hall 40 m by 25 m and 13 m tall: a 21 m wall along the 25 m axis, 2 m from each end wall; a 12 m wall abutting its side 8.5 m from one end; a 12 m wall abutting the second's side at its midpoint; each 12 m wall's free end 2.5 m from the hall wall, all 7 m tall. Every line in front of a 12 m wall's faces meets the wall it abuts, which ends its span only if that wall counts as blocking | Classify all; the order in which brushes are visited differs | Every chain wall occludes in every order, whatever counts as blocking (§Available space, mutual dependence) |
| R29 | Each hinted fixture, and the same map with its hint brushes removed | Hints peeled before BSP, so classification, BSP and portal generation never read them; then hints resolve | Cells and portals match between each pair; hints add no cell boundary (§Streaming hints) |
| R30 | Brushes bounding a room against the void, meeting the others only edge to edge, so each one's own line runs past its ends into the void: two sealed rooms 8 m square and 4 m tall sharing an 8 m wall, slabs 0.3 m thick; occlusion-test's first room | Exterior classified; open span measured in front of each broad face, bounded by solid or exterior void; then the rule applies | The two rooms are different cells; each synthetic slab, and occlusion-test's first-room walls and ceiling, occlude alone or as a cluster. That room's floor is not pinned: its span runs through the east doorway (§Owner examples) |
| R31 | Wall whose two broad faces see different open spans: a closed room 8 m square and 4 m tall, of 0.3 m slabs, standing in a hall 40 m square and 13 m tall | Exterior classified; each broad face measured; the brush occludes if either face passes | Each wall and the roof occlude from the inner face; the room's interior and the hall are different cells. A build requiring both faces exempts every wall and merges the room into the hall's cell (§Occluder rule, either broad face) |

## Related follow-ups

- Fog raymarch and skinned-viewmodel GPU cost: about 4.8 ms and 3.2 ms per frame at the over-the-wall pose (`frame-cost/results/gpu/`). Owner-carved follow-up; not on the roadmap from this brief.
- Roadmap "Sector Graph + Portal Culling" replaces the compiler-derived cell/portal graph with an authored one. This brief keeps the runtime cell, portal and locator contracts, so that work can still replace how cells are derived.
