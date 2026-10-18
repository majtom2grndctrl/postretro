# fluid-surface-t-junction-closure — research

Read at `64d069e19` (branch `fluid-base-pass-cost`; the grid code is on this branch, not yet on `main`).

## Current behaviour (code)

- **Builder.** `encode_environment_volumes_section` (`level-compiler/src/environment_volumes.rs`) maps each volume to its faces in one pass: `fluid_surface_pieces` (`environment_volumes/surface.rs`), then `surface_face` per piece. That function encodes positions as f32 and derives texel UVs from the piece's own side (`brush_side_texel_uv`). The stage is uncached and runs on every build, warm and cold, just before TextureMips (`build_pipeline.md` §Fluid surfaces).
- **Per side.** The side is subtracted by nearby solids (`SolidIndex`, an AABB tree with ascending results) and by sibling brushes (all-pairs over the volume's brushes, AABB-culled). It is then split along push brushes (`PushBrushes::split`, `push_split.rs`, `DRIFT_SAMPLE_DEPTH` 2 mm).
- **Closure gate.** `close_t_junctions` runs only when `push_cut` is set. The comment states the reason: "a volume no push cut keeps its subtraction output exactly". The gate came from `plans/done/fluid-surface-motion` review round 2: closing changed id-52 bytes for burial-only maps against that plan's byte-identity row A11.
- **`close_t_junctions`.** Pairs every piece of one volume with every other (AABB pre-check, O(n²) in the volume's pieces). Each piece's *original* vertices are tested against every other piece's edges (`edge_holding`), then inserted, sorted by edge and position along it. So:
  - the result does not depend on piece order;
  - the inserted vertex is the neighbour's own DVec3, so it encodes to the identical f32 on both sides;
  - consecutive inserts within the tolerance dedupe.
- **Constants.**
  - `T_JUNCTION_EPSILON` = `SPLIT_EPSILON` = 1e-4 m (0.1 mm), for both the on-line distance and the near-corner exclusion.
  - `FLUSH_EPSILON` 1e-3 m.
  - `MIN_PIECE_AREA` 1e-6 m².
  - `WELD_EPSILON` 1e-6 m, which collapses consecutive duplicates only, within one piece.
- **Shared corners.** No step makes corners shared by two pieces identical. Each corner is the f64 result of its own clip sequence, so identical f32 encoding is likely for axis-aligned integer brushes but not guaranteed. The first slice measures it.
- **Renderer coupling.** `COLLINEAR_DISTANCE` = 2e-4 m (`render-cpu/src/fluid_surface/mod.rs`). Its comment reads: "The compiler inserts a T-junction vertex within 0.1 mm of its edge's line; twice that also covers f32 position rounding." The grid's `PLANE_TOLERANCE` equals it (`grid.rs`). Widening the compiler tolerance would break both.
- **Test pins today.**
  - `volumes_no_push_cuts_keep_their_subtraction_output_with_no_inserted_vertex` (`surface_tests.rs`) asserts burial and unequal-sibling T-junctions stay open.
  - The `t_junctions` helper checks one record only.
  - `adjacent_fluid_entities_each_draw_their_shared_face` counts faces, so closure does not change it.
  - No integration test compares id-52 bytes across builds. `environment_volumes_compile.rs` pins every *other* section byte-identical.

## The grid case

`fluid-base-pass-cost` plan of record, A3, records P13 as open: an edge at a shallow angle θ to a lattice plane passing within tolerance of an open T-vertex. The long side gains a crossing up to (tolerance + T offset)/sin θ from T. A probe measured this at 0.4 m. The crack lies on the open edge's line, so it is no wider than the T-junction itself. The pin is `an_open_t_junction_keeps_its_vertex_and_the_crossings_agree_within_tolerance` (`grid_tests.rs`).

With closure, both faces hold P→T→Q with T bit-identical. Crossings are a function of an edge and a plane alone (`grid::crossing`, from the lexicographically lower endpoint), so the case disappears. `faces_sharing_an_edge_carry_identical_vertices_along_it` already compares with `assert_eq!`, which needs bit-identical endpoints. That is why the brief asks for bit-identity and not just "within tolerance".

## Consumers of id-52 surface faces

| Consumer | Reads | Effect of closure |
|---|---|---|
| Fluid pack (`pack_fluid_surfaces`) | positions, UVs, order | More collinear vertices. An ungridded face takes the centroid fan (`triangulate_convex_face`), a gridded one ear-clips (`triangulate_convex`, O(n²) in a part's vertices). Already handles inserts. |
| Drift maps (`drift::volume_drift_maps`) | positions, UVs, per volume | Inserted vertices lie on the face's own plane within 0.1 mm (fit tolerance 1 mm) and carry their own side's UV, so split parts keep sharing one map. Risk: `FaceMap::fit` searches every vertex triple, O(n³) in a face's vertex count. |
| Runtime drift resolve (`resolve_fluid_surface_drift`, `postretro/src/fluid_presentation/drift.rs`) | vertex-mean centroid, 2 mm in | The mean moves with inserts but stays inside the convex piece. Each piece is wholly under or clear of each push by its 2 mm samples, so the resolved push is unchanged. The edge case is a push touching a piece by under 1 mm², which the split leaves whole. |
| Cull bounds (`surface_bounds`) | vertex bounds | Grow by ≤ 0.1 mm at most. |
| Probe placement (`reflection_probes/placement.rs`) | area centroid, Newell normal, (x, z) outline, (volume, face) numbering | The area centroid is already documented as unmoved by inserts. Outline bounds may move by ≤ 0.1 mm, so box fit and capture bytes may shift by that much. Face numbering is unchanged as long as faces are neither added nor dropped. |
| Capture-visibility warning (`capture_visibility::seen_fraction`) | outline fan from vertex 0, sub-triangle samples | Samples redistribute and the fraction can shift slightly. Advisory warning only. |
| id-54 stage key (`reflection_probes/stage_key.rs`) | the whole id-52 bytes | Changed id-52 bytes miss the cache once per affected map, which recaptures the probes. No ongoing cost. |
| Loader binding check (`level-loader/src/prl_environment.rs`) | face count per volume | Unchanged. |
| Level content digest (`runtime_movers.rs` `hash_environment_volumes`) | skips `surface_faces` | No co-op, admission or wire effect (`networking.md` §Level identity and level content digest). |
| Physics, environment resolution | brush leaves only | None. |

The 2 mm split depth is a depth along the face normal and is untouched by in-plane vertex insertion. The test pinning it to the runtime constant is unaffected.

## Bake cost

- **Today.**
  - At E24 landing, `stress-env-volumes` had 5,063 fluid faces, and subtraction took 24 ms of a 76 s compile (`plans/done/E24--water-and-environment-volumes/plan.md`, task 9).
  - Closure is per-volume all-pairs and runs only in push-cut volumes.
- **Generalised naively.** Level-wide all-pairs on 5k faces is about 25M AABB tests, which is tolerable there. It is quadratic for an authored ocean of many abutting brushes, and the stage runs on every warm build. The index must be output-sensitive.
- **Index precedents in-tree.**
  - `SolidIndex`: an AABB tree with deterministic ascending results.
  - `partition/manifold.rs`: groups edges by quantized supporting line (`ANCHOR_QUANTUM` 1e-4). Quantized keys alone split near-equal points across a cell boundary, so a closure lookup must also probe neighbouring cells (dmap's `GetHashVert` comment, below).
- **Install.** No new install pass. Closure raises per-face vertex counts, and two install steps are superlinear in them: the drift fit (cubic) and the gridded ear clip (quadratic per part). A large face bordered by many small pieces is the case to bound.

## Prior art

Units: 1 Quake map unit is 0.0254 m here (`UNIT` in `surface_tests.rs`). Rows marked † come from a research agent's reading of source and were not re-read this session. All other values were re-read from source this session.

| Tool | Where | Tolerance / quantization | Slivers, degenerates | Across objects |
|---|---|---|---|---|
| Quake qbsp `TJUNC.C` | compile | `EQUAL_EPSILON` 0.001 u, `T_EPSILON` 0.01 u along the edge. Edges canonicalised to lines ("wedges") in a 1024-bucket XY hash. No vertex snapping. | Faces past `MAXPOINTS` (28) split at real corners † | One BSP tree per call † |
| ericw-tools `qbsp/tjunc.cc` | compile | `PointOnEdge` uses `DEFAULT_ON_EPSILON` 0.1 u (2.5 mm). Works on an already-shared output vertex list †. Candidates come from BSP nodes overlapping the edge's AABB grown by 1 u. | Minimum-weight triangulation rejecting angles under 0.01°, then start-vertex rotation, then retopologize into fans, then the superface as-is. `maxedges` (64) splits fragments † | `Welds()`: same contents weld; translucent never welds to opaque; backfaces never weld. A FIXME notes liquids are not fully handled. Each model tree is separate † |
| Source vbsp `faces.cpp` | compile | `INTEGRAL_EPSILON` 0.01 (snap to integer), `POINT_EPSILON` 0.1 (vertex merge in a global table †), `OFF_EPSILON` 0.25 (on-edge), `HASH_BITS` 7 (128 u XY cells), `MAX_SUPERVERTS` 512 | Start-vertex choice; with none, `badstartvert`, and world faces get an explicit triangle list | Global vertex table merges across the map †; the triangle-list sewing is world-only |
| q3map2 `tjunction.c` † | compile | `LINE_POSITION_EPSILON` and `POINT_ON_LINE_EPSILON` 0.25. Edge lines found by a linear scan, so no hash. `MAX_SURFACE_VERTS` 256 | Start-vertex rotation; `FixBrokenSurface` removes near-coincident vertices | Per draw surface; scope unverified |
| Doom 3 dmap `tritjunction.cpp` (identical in dhewm3 and RBDOOM-3-BFG †) | compile | Snaps to 1/32 u (`SNAP_FRACTIONS`); `COLINEAR_EPSILON` 1.8/32 u (1.4 mm); merges with neighbouring snapped cells; `HASH_BINS` 16³ over the bounds | Rejects a split whose two triangles' normals disagree; drops triangles that collapse on snapping. "T junction fixing never creates more xyz points." | `FixGlobalTjunctions` hashes every area group of the world and adds `func_static` model vertices as splitters; skips `IsDiscrete` materials |
| GLQuake † / Xash3D FWGS `gl_rsurf.c` | load (reverse) | Removes collinear points per face (0.001 per axis of normalized directions) unless `gl_keeptjunctions` is set. | — | Skips `SURF_UNDERWATER`, so liquids keep the compiler's T-junction vertices |
| Godot 4.4+ CSG `csg_shape.cpp` | build (editor/runtime) | Manifold `mesh.tolerance = 2 * FLT_EPSILON`, then `Merge()` | Manifold guarantees a manifold output | Per CSG tree |
| func_godot | import | `_VERTEX_EPSILON` 0.008 snap; optional per-entity `vertex_merge_distance` (default 0) | Drops consecutive duplicates in a winding | No T-junction step; nothing across brushes or entities † |
| Unreal BSP | — | Unverified (source not public) | — | — |

**Bearing on the brief.**
- **Close globally, on a shared vertex set.** dmap's global pass, vbsp's global vertex table and ericw's shared vertex list all close across faces from different sources against one vertex set. This supports both cross-volume scope and bit-identity: weld near-coincident points to one canonical position, then insert.
- **Spatial hashing everywhere.** Every surveyed compiler bounds lookup with a hash or tree, except q3map2's linear line scan. Bucket choice must handle points near a cell boundary: dmap probes neighbouring cells; vbsp snaps first.
- **Tolerance.** ericw-tools, vbsp and q3map2 use 2.5 to 6.4 mm, and dmap snaps at 0.8 mm with a 1.4 mm colinear test. Those are sized in map units around snapping, integer snapping in vbsp's case. The original qbsp is tighter (0.025 mm on equality). Ours clips in f64 from f32 map coordinates at 0.1 mm, and the renderer's collinear tolerance and the grid's plane tolerance are derived from it. Keep it.
- **Slivers are a triangulation problem.** The compilers above emit triangles or fans and fight slivers at triangulation. PostRetro's compiler emits convex polygons. The renderer already keeps inserted vertices as corners: the centroid fan for ungridded faces, ear clipping for gridded parts. The compiler's duty is no degenerate polygon and no lost area.
- **Translucent versus opaque.** ericw never welds translucent faces to opaque ones, and Quake-lineage renderers keep liquid T-junction vertices. Fluid-to-world welding is unnecessary, because the opaque, depth-written wall behind a fluid edge fills any crack.
- **Per-face vertex limits.** qbsp, ericw, vbsp and q3map2 all cap vertices per face, by splitting or by error (†: qbsp `MAXPOINTS` 28, ericw `maxedges` 64, vbsp `MAXEDGES` 32, q3map2 `MAX_SURFACE_VERTS` 256). PostRetro has no cap, so the install consumers that scale superlinearly must be bounded instead. Splitting faces would change face numbering for no gain, since drift maps already share across parts.
- **No surveyed engine closes T-junctions at load.** The load-time precedent (GLQuake, Xash) goes the other way: it removes collinear points and exempts liquids.

Sources:
- https://raw.githubusercontent.com/id-Software/Quake-Tools/master/qutils/QBSP/TJUNC.C
- https://raw.githubusercontent.com/ericwa/ericw-tools/main/qbsp/tjunc.cc · `include/common/qvec.hh` (`DEFAULT_ON_EPSILON`)
- https://raw.githubusercontent.com/ValveSoftware/source-sdk-2013/master/src/utils/vbsp/faces.cpp
- https://raw.githubusercontent.com/TTimo/GtkRadiant/master/tools/quake3/q3map2/tjunction.c
- https://raw.githubusercontent.com/id-Software/DOOM-3/master/neo/tools/compilers/dmap/tritjunction.cpp · `dmap.cpp`
- https://raw.githubusercontent.com/id-Software/Quake/master/WinQuake/gl_rsurf.c
- https://raw.githubusercontent.com/FWGS/xash3d-fwgs/master/ref/gl/gl_rsurf.c
- https://raw.githubusercontent.com/godotengine/godot/master/modules/csg/csg_shape.cpp
- https://raw.githubusercontent.com/func-godot/func_godot_plugin/main/addons/func_godot/src/util/func_godot_util.gd · `src/core/geometry_generator.gd`

Not covered: ZHLT/VHLT `hlbsp` tjunc, Xash3D and PrimeXT compile tools, Unreal, and library repair (CGAL `stitch_borders` and snap rounding, libigl, meshoptimizer), which are known only from memory and not cited.

## Rivals

| Shape | For | Against |
|---|---|---|
| Close in the compiler, every face, level-wide (chosen) | Bake over compute (`index.md` §2). One closure rule replaces a gated special case. f64 positions and clip context are available. Zero install cost. Every surveyed BSP compiler closes at bake. | id-52 bytes change and the byte-identity row retires. Adds to every build, since the stage is uncached. Maps must be rebuilt to benefit. |
| Weld at fluid install in render-cpu, before the grid cut | No format change or rebake; about 150–250 lines; a few ms at install | Runs every load, against near-instant boot. Works on f32 positions with no clip context. Leaves two closure owners (compiler for push cuts, renderer for the rest). Gating it on an active grid leaves ungridded levels cracked. No precedent found for load-time closure. |
| Status quo plus the grid's P13 exception | No work | Hairline cracks stay at burial edges, unequal siblings and volume seams. The surface is translucent, so they can show as sparkles at grazing angles. |
| Split over-long faces (qbsp-style vertex cap) instead of bounding consumers | Bounds per-face vertex counts | Changes face count and order for no correctness gain |

## Pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | volume with burial and a push cut | subtract → push split → close | closure sees the split pieces; every split corner on a neighbour's edge is inserted |
| P2 | two volumes sharing an edge, the second later in map order | every volume's pieces built, then one closure pass, then encode | each volume gains the other's vertices on its edges, whichever comes first |
| P3 | corner of A within 0.1 mm of corner of B, not equal in f32 | weld before insert | one encoded position on both faces |
| P4 | vertex within 0.1 mm of a corner and also within 0.1 mm of a neighbouring edge | weld first | merged into the corner; no extra vertex beside it |
| P5 | texture interning while encoding | closure precedes encode; encode keeps volume then face order | texture indices and TextureNames order unchanged |
| P6 | first warm build after landing on a map with an open T-junction | id-52 bytes change, so the id-54 stage key misses | probes recapture once; the next warm build hits |
| P7 | PRL built before the change | load | loads; its cracks remain until rebuilt; no version check |
| P8 | weld would collapse a face (edge under 0.1 mm across) | weld judged per pair | no face drops below three distinct vertices or loses more than tolerance × perimeter of area |
