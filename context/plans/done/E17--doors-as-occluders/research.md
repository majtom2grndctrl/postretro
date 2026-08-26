# Doors as Occluders — Research Notes

Investigation behind `index.md`. Not the spec. Code-grounded against source this session.

## Two visibility systems — the framing that shapes the spec

The tree has two distinct visibility systems, and doors-as-occluders belongs to the first:

1. **Runtime camera portal flood** — `crates/visibility/src/portal_vis.rs::flood`, per-frame, decides
   what renders. Three per-step decisions: portal-cycle (`path.contains(portal_idx)`),
   `cell_is_solid(neighbor)`, geometric clip/`narrow_frustum`. No portal-state concept; no mover↔portal
   link. This is what a closed door must extend.
2. **Baked `CellVisibility` Cell→Cell relation** — shipped `plans/done/cell-visibility-relation`, PRL
   section id 46, `crates/level-loader`. View-independent, for the *non-camera* consumers (net
   relevance, audio PAS, AI perception, VFX cull). Its own research doc:
   "This substrate is not for camera rendering."

E18-E's sealed monster-closet is a **camera** problem (don't render the enemies behind the closed
door), so it extends system 1. The substrate (system 2) deferred dynamic geometry as an additive
`(a,b,portal-state)` blocker-mask seam for *its* consumers; none has a live need, so building it now
would skew the substrate. Not skewing it = not modifying it: build one neutral, portal-keyed
dynamic-portal primitive (baked association + runtime blocked set), consumed by the camera now, reused
by the substrate's future mask layer later.

## Lifecycle — per-frame stateless derivation (no latch)

```mermaid
sequenceDiagram
    participant Bake as prl-build (offline)
    participant Load as Loader / spawn
    participant Tick as Fixed-tick loop
    participant Frame as Render stage (per frame)
    participant Flood as Camera flood

    Bake->>Bake: coverage test — mover closed brush vs generated portals
    Bake->>Load: KinematicGeometry v5: per-mover sealed_portal_ids
    Load->>Load: validate ids in-range; seed onto KinematicMoverComponent
    Tick->>Tick: advance mover phase (host sim or client predict/seed)
    Frame->>Frame: for each mover fully docked-closed → set its sealed ids true
    Frame->>Flood: determine_visible_cells(..., &blocked_portals)
    Flood->>Flood: skip blocked portal (reject beside cell_is_solid)
    Flood->>Frame: drawable VisibleCells (interior behind closed door absent)
```

The blocked set is **rebuilt every frame from current phase** — no persistent blocked-state, no event
latch. This sidesteps the ordering-bug class latches invite (ping-pong, auto-close, reversal,
block-partway). The only cross-seam contract is portal-index stability bake→load.

## Code-grounded facts (verbatim identifiers)

### Camera flood (`crates/visibility`)
- Entry `determine_visible_cells(camera_position, view_proj, world, capture_portal_walk, scratch)`
  (`visibility.rs:522`) → `determine_visible_cell_set` → `portal_traverse_detailed(camera_position,
  camera_cell, frustum, world, capture)` (`portal_vis.rs:81`) → `portal_traverse_with_step_limit` →
  `portal_traverse_inner` → `fn flood(state, cell, frustum, path, clip_scratch_a, clip_scratch_b)`
  (`portal_vis.rs:260`). Shared data rides `DfsState` (built ~`portal_vis.rs:179`) — the natural home
  for a `blocked_portals` field rather than a positional param on every fn.
- Flood reject point: `portal_idx` at `portal_vis.rs:321`, `&state.world.portals[portal_idx]` at 325,
  `cell_is_solid(neighbor)` reject at 347. Add the blocked-portal skip here.
- `PortalTraversalStats` (`portal_vis.rs:27-38`): counters `rejected_solid`, `rejected_clipped`,
  `rejected_narrow`, `rejected_invalid`, `rejected_path_cycle`, `rejected_depth_limit`,
  `step_limit_hit`. Add `rejected_blocked_portal`, emit like the others (trace summary ~239-245).
- Produces `visible: Vec<bool>` per cell → `VisibleCells::{Culled(Vec<u32>), DrawAll}`
  (`visibility.rs:13`) + `fog_reachable`. Blocking a portal shrinks both (fog culling shares
  `fog_reachable`). Visibility path stays `PrlPortal`, so the candidate-cull fast path is unaffected.
- Fallbacks (`visibility.rs`): solid-cell, exterior-camera, no-portals, portal-step-limit → per-cell
  AABB frustum cull, no portal walk ⇒ no door occlusion on those paths (conservative over-draw).

### Movers / doors (`crates/postretro`, `crates/entities`)
- A door is a `kinematic_mover`; no dedicated type. `KinematicMoverComponent`
  (`crates/entities/src/components/kinematic_mover.rs:49`) phase fields: `segment_index: u16`,
  `direction_sign: i8`, `segment_elapsed_ms: f32`, `current_linear_velocity: Vec3`, `started`,
  `completed`, `blocked`, `was_active_this_tick`, `target_segment: Option<u16>`.
- `mover_is_at_open_terminus` (`crates/postretro/src/kinematic_mover.rs:276`): `waypoints.len() >= 2 &&
  segment_index == waypoints.len()-1 && segment_elapsed_ms <= f32::EPSILON`. Closed = index 0
  (confirmed by `travel_toward_closed_terminus` setting `target_segment = Some(0)`,
  `kinematic_mover.rs:690`). Docked-closed = the mirror at segment 0 + at rest + `!blocked`. No stored
  `at_rest`/`is_closed` boolean; derived.
- Frame loop: `determine_visible_cells` call at `main.rs:3497`, in the render stage **after** the
  fixed-tick loop (`main.rs:2569-2967`). `App` holds `scratch_cells: Vec<u32>` (`main.rs:671`) and
  `kinematic_mover_tick_states: MoverTickStateTable` (`main.rs:735`). Mover *phase* is on the
  `KinematicMoverComponent` in the registry (`script_ctx.registry`), not on `MoverTickState`, so the
  blocked-set build reads the registry post-tick.
- Networking: `WireKinematicMoverState` (`crates/net/src/wire.rs:285`) mirrors `segment_index`,
  `direction`, `segment_elapsed_ms`, `blocked`, etc. Block decision is host-authoritative, never
  predicted; clients seed phase via `seed_kinematic_mover_phase` (`client.rs:2276`). So each peer
  derives docked-closed locally — no new wire.

### Bake + format (`crates/level-compiler`, `crates/level-format`)
- `generate_portals(tree: &BspTree) -> Vec<Portal>` (`portals.rs:27`), emits a portal only between two
  non-solid leaves; `Portal { polygon: Vec<DVec3>, front_leaf, back_leaf }` (`portals.rs:19`),
  `MIN_PORTAL_AREA_M2 = 0.1`.
- `kinematic_mover` brushes are excluded from the static BSP (`parse.rs` routes them to
  `pending_kinematic_movers`, never `world_brush_ids`), so a doorway bakes as an **open** portal
  regardless of the door — the "bake open" half is free.
- Coverage-test inputs both in scope at `pipeline.rs:376` (Geometry stage): `generated_portals`
  (from `pipeline.rs:352`) and `map_data.kinematic_movers[..].brush_volumes`. `MapKinematicMover`
  (`map_data.rs:152`): `origin: DVec3` (= first/closed waypoint), `brush_volumes: Vec<BrushVolume>`.
  `BrushVolume { planes: Vec<BrushPlane { normal: DVec3, distance: f64 }>, aabb, sides }`.
  `partition::Aabb` has `intersects`, `centroid`, `is_entirely_behind_plane` — **no `contains`**.
  Coverage is **union** coverage (a two-brush door counts), computed by convex carving of the portal
  polygon against each brush, reusing the compiler's `clip_winding_to_half_spaces` (`portals.rs`) — the
  same winding half-space clip `generate_portals` already uses. Sealed iff no uncovered fragment above
  a conservative epsilon remains; epsilon errs toward uncovered so a seam gap never falsely seals.
- `KinematicGeometry` section id 43, `KINEMATIC_GEOMETRY_VERSION: u16 = 4`
  (`kinematic_geometry.rs:11`); v3/v2/v1 stay loadable. `KinematicMoverRecord`
  (`kinematic_geometry.rs:37`) has `indices: Vec<u32>` serialized via `write_count` + per-`u32`
  (`kinematic_geometry.rs:188`) — the exact pattern for `sealed_portal_ids`. Loaded struct
  `LoadedKinematicMover` (`prl.rs:430`) via `From<KinematicMoverRecord>` (`prl.rs:466`); conversion
  `convert_kinematic_geometry_section` (`prl_loader.rs:187`). `KinematicGeometry` is noted excluded
  from static `Geometry`/`Bvh`/collision/lightmap/SDF/portals/navmesh, and its host-only block policy
  and timers already sit outside the multiplayer static-content hash — `sealed_portal_ids` joins them.
- `PortalData { polygon, front_cell, back_cell }` (`prl.rs:281`); `convert_usable_portals`
  (`prl_loader.rs:1293`) — **resolved: all-or-nothing.** It lowers every portal 1:1 in enumeration
  order, or on the first malformed portal (overflow, `<3` verts, non-finite, zero area) returns `None`
  ⇒ no-portals fallback for the whole map. It never drops-and-compacts individual portals. So portal
  indices are stable bake→load 1:1 whenever a graph exists; when it does not, the flood never runs and
  the baked `sealed_portal_ids` are never consulted. The load-time in-range guard is belt-and-braces,
  not a correctness load-bearer.

## Direction-question detail (kept out of the spec body)

- **Q2 placement.** Occlusion is engine-floor (not moddable per-tick) and belongs in the runtime
  camera flood (`crates/visibility`), which stays neutral (takes a blocked-portal slice, never reaches
  into entities). The association is baked (compiler), consistent with "baked over computed." The
  blocked-set build straddles game-logic (reads mover phase in `postretro`) and feeds the render-cull
  crate — the crate layering holds: `postretro` → `visibility`, never the reverse.
- **Q3 foreclosures.** Bake + auto-detect forecloses (for v1): partial-occluder doors, non-waypoint-0
  sealing poses, and blocking while ajar — all safe over-draw, all additive to revisit. Portal-index
  stability becomes load-bearing (guarded).
- **Q5 undo cost.** Two-way; deletes the v5 field, the frame-loop build, the flood predicate/stat, one
  visibility param. Section 46 untouched.

## Closet-reveal seam (downstream — not built here)

E17-F is the concealment half of the monster-closet scare; the reveal/lunge half is E18-E / Epic 16
combat. Two facts of this spec are load-bearing for that future work and are recorded here so it
inherits them rather than rediscovering them:

- **Occlusion lifts on leave-closed, not on fully-open.** The portal un-blocks the frame the door
  leaves the waypoint-0 dock (Orderings: "Door begins opening"), so the interior becomes visible for
  the *entire* door-open swing, not only once the door is fully open. That swing is the natural window
  in which a revealed occupant is seen coiled/telegraphing before it commits — good horror (you see it
  *about* to lunge) and the cover the netcode seam needs. A later "optimize occlusion to lift only when
  fully open" change would foreclose this; keep the early lift.
- **Cull-while-sealed ⇒ no interpolation history behind the door (AC14).** While the door is
  docked-closed the closet's occupants are not collected (transitively culled), so a networked client
  holds *zero* interpolation samples for them until the reveal. On the reveal frame an occupant has one
  sample and must accumulate history; a fast arc committed on that frame pops in (teleports) instead of
  being seen to launch.
- **The guarantee the future reveal owes:** hold the launch pose for ≥ interpolation delay after
  un-block, so history fills before the arc commits. The door-open swing supplies that window *when the
  door has a non-trivial open duration* — but a zero-duration/degenerate open segment exists (a door can
  open in one tick, per the segment-length floor), so the held-pose telegraph is the guarantee and the
  swing is only bonus cover. The telegraph also *is* the combat-fairness budget
  (`telegraph_ms ≥ worst-case interpolation delay + reaction budget`): hiding the interpolation seam and
  keeping the lunge fair are the same mechanism. (On the client the door itself is locally predicted, so
  its occlusion already matches its drawn pose; the reveal-seam concern is the *occupant's*
  interpolation history, not the door's — AC8.)

This spec builds none of it. It records the constraint and preserves the early un-block.

## Where this does NOT go (substrate neutrality)

No task edits `CellVisibility` (id 46), its lowering, or its query. Door-occlusion data lives on
`KinematicGeometry` (id 43) + the runtime blocked slice. When net/audio/AI later want door-aware
coupling, they build the substrate's deferred blocker-mask layer, reading the same per-mover
association — additive, at a section-46 version bump, with the destructible/consumer epic as its
driver, per `research/cell-visibility-substrate.md` §Dynamic geometry.
