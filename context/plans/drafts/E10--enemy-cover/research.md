# E10--enemy-cover research

## Id 46 meaning
- A graded coupled pair: the nearest `FANOUT_K` cells by portal-path distance, with aperture. A cell around a corner is coupled; a far cell in plain view may not be. The penalty reads proximity, not sight.

## Seams (by symbol)
- `bake_navmesh_with_movers` and `erode` (`crates/level-compiler/src/navmesh_bake.rs`): erosion drops its boundary cells, which also count drops and the grid edge, so rays must tell a wall from a ledge. Wall normals need the triangle list or a bake ray.
- The bake BVH from `bvh_build::build_bvh` is in scope before the NavMesh stage but not passed in. Bake rays go through `ray_traversal::BoundedRay`.
- `NavMeshSection` (`crates/level-format/src/navmesh.rs`): accepts versions 2 and 3 today; sorted, deduplicated tables.
- `score_candidate` and the slot batch (`crates/sim/src/combat_positioning/scoring.rs`): `has_tactically_direct_target_path` is the direct-path filter cover skips; the LOS test uses the enemy's current eye offset.
- `perception::enemy_eye` (hitbox top, else 0.9 × nav agent height) and `HealthComponent.hitbox`: the stance seams. `perceive_target` runs before `select_transition_path` (`crates/ai/src/compute.rs`).
- `astar_corridor` (`crates/sim/src/nav/path.rs`): the cost sites a hook touches.
- Id 46 (`level-format/src/cell_visibility.rs`): portal-graph coupling with reachability components and the nearest `FANOUT_K` graded pairs. No AI or sim code reads it or the cell locator yet.
- Replication: enemies replicate as generic component payloads; mesh-less display copies carry only a hit volume. `WIRE_VERSION` (`net/src/handshake.rs`) and `SNAPSHOT_VERSION` (`net/src/wire.rs`) gate it.

## Source facts (read at 7ee9389f)
- Navmesh bake: `bake_navmesh_with_movers` holds static triangles, `WalkGrid` (walkable floor heights per column, post-erosion), regions and portals. `erode` drops its `boundary` list; cells within about the agent radius of a wall are removed. Walls add nothing to the grid; a wall normal needs triangles or a bake ray.
- Obstruction height is partly recoverable from the grid: a crate's flat top reads as a neighbour floor above step height (half cover); a wall to the ceiling reads as an empty column (full). A sloped top or one failing the clearance filter also reads as empty, so a ray settles it.
- Bake cost: NavMesh stage 0.06–2.39 s; lightmap 860 s on the same map. Roughly 5k candidates × 16 directions × 4 rays is about 320k BVH rays. The stage is sequential and byte-deterministic.
- Cache: `"navmesh"` stage, `NAVMESH_STAGE_VERSION = 5`, does not fold `TRACER_EPOCH`. Cover folds into this key with the tracer epoch and the cover entities; a tracer bump rebakes regions too, at most about 2.4 s.
- Movers are absent from static geometry and the BVH. `collision::line_of_sight` is static only.
- Runtime: `perceive_target` casts one static ray per enemy per tick, eye to eye, symmetric. LOS measures about 0% of AI cost. `sim_ai` is 5.8 ms at 64 agents against a 1.0 ms target; the cost is registry walks and `NavGraph::region_at`.
- `perception::enemy_eye` derives from the hitbox top or `EYE_FACTOR` × agent height; `AgentComponent.height` and `HealthComponent.hitbox` are fixed at attach. Hit zones (`HitZoneStore`) re-sample the posed animation per ray.
- Mixamo rifle library: `idle_crouching`, `idle_crouching_aiming`, `walk_crouching_{forward,backward,left,right}`, `crouching_turn_90_*`. No cover or peek clips.
- Pathfinder: `astar_corridor` prices edges by XZ midpoint distance plus `RIDE_COST`; no hook. Paths are per agent; replans are capped at `REPLAN_BUDGET_PER_TICK`.
- Splash: `splash::entities_in_sphere` casts one static ray to the nearest body point. Projectiles are swept spheres, so an eye ray slightly over-reports cover against them.

## Movers (E17)
- Bake side exists: `nav_poses` (`navmesh_bake/mover_poses.rs`) poses every non-spinning mover's convex brushes at each waypoint; the mover light gate's `Pose::segment_touches` tests segments against posed brushes. Runtime dock state: `MoverDockTable::docked_waypoint`, watched only for movers named by gate or link records.
- Runtime side missing: no combined static + mover `line_of_sight`; it is task 1 of `drafts/E17--movers-block-shots-and-sight`. Owner chose static cover now and a mover-aware cover follow-up on Epic 17.

## Co-op and naming
- Player crouch states: `crouch_idle`, `crouch_forward`, `crouch_backward` (`entity_model.md`).
- Display copies of zone-less enemies use the authored `health.hitbox`; host validation of a client's hit is static-world only (`networking.md`). Hence the replicated stance component.

## Industry reference (owner conversation)
- Shipped games mostly bake or author cover candidates and validate at runtime: F.E.A.R. and Gears authored nodes; Halo designer firing positions scored at runtime; Killzone baked waypoints with per-sector line-of-fire distances and runtime weighted scoring; The Last of Us a coarse exposure map refreshed about once a second.
- Unreal EQS and CryEngine TPS: generate, filter cheap-first, then score.
- Killzone's tactical A* added costs for being under fire; a view-cone path cost is the common flank recipe.
- DOOM (2016) rejects cover as a genre convention; Selaco uses F.E.A.R.-like cover and punishes camping. Cover stays opt-in per archetype here.

## Pin table

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Two enemies seek cover on one tick | Both qualify for one point in the same batch. | One claims it by the batch's deterministic order; the other takes its next best point or holds. A replay gives the same split. |
| P2 | Clip resolution under crouch and direction | Activity animation `walk`, crouch, travel left under target facing; the mesh may lack any of the composed names. | Tries `crouch_walk_left`, then `crouch_walk`, then the stand chain (`walk_left`, `walk`). The host resolves the name; it replicates as ordinary animation state. |
| P3 | Stance on activity entry | Tick N: a transition enters a crouch activity after N's perception. | The box applies from N's apply, before the Weapons stage. Perception reads the crouched eye from N+1, the first tick the activity's rows evaluate. Leaving restores standing the same way. |
| P4 | Duck after a peek | Tick N: peek → settle with `targetVisible` 1 and grace open. | Grace is cleared at N. N+1 reads the raw crouched-eye ray: hidden → 0, visible → 1. Without a stance change, today's grace holds. |
| P5 | Stance change and target switch on one tick | Tick N: crouch entry while selection switches target. | The switch already prunes grace. N+1 reads the crouched eye against the new target with no grace. |
| P6 | One-tick crouch activity | Enter at N, leave at N+1. | Box lowered for N's Weapons stage, restored at N+1. Grace cleared at both changes. A client that misses N's snapshot never shows a stuck crouch. |
| P7 | Stage order in tick N | Projectiles → AI apply sets stance → Weapons → snapshot. | N's projectiles see the standing box; N's hitscan sees the crouched box. N's snapshot carries the stance component and the `crouch_` clip name together. Perception changes from N+1. |
| P8 | Serialize-and-restore mid-crouch | Save during a crouch activity, then restore. | Stance is re-derived from the restored path. The box is lowered exactly once. Grace starts empty. |
| P9 | Late joiner | A client joins while an enemy crouches. | The join baseline carries the stance component; the first display copy has the crouched volume. |
| P10 | Target on a cover point | The target occupies a candidate. | Qualification refuses it: the target's eye sees the standing eye at its own position. It is never claimed. |
| P11 | Level change or hot reload alters cover | An enemy holds a cover slot. | The claim drops on reload; the next entry re-picks from the new table. |
| P12 | Cover point on a suppress boundary | A derived point lies exactly on an `cover_suppress` plane. | Counted inside and removed, identically across bakes. |
| P13 | Cover goal moves after arrival | Tick N: `arrived` reads 1; ticks N+1…: the target moves and the cover pick re-scores to another point. | `arrived` is live: it reads 0 while the enemy travels to the new pick and 1 again on reaching it. |
| P14 | Arrival and target switch on one tick | Tick N: the enemy reaches its goal while selection switches target. | Facts precede the batch, so tick N's `arrived` reads the previous tick's goal. A row firing on arrival at N acts against the old target's pick; the new target's pick applies from N+1. |
| P15 | Pick given away | Tick N: the batch hands the enemy's cover pick to another enemy. | `arrived` reads 0 until the enemy reaches its next pick, or stays 0 while it holds with none. |
| P16 | Stale arrival on entry | Tick N: an enemy enters `cover` while the previous activity's `arrived` read 1; its next refresh is up to 500 ms away. | `arrived` reads 0 from entry until a refresh finds the enemy at its new goal. The arrival row never fires on the entry tick. |

