# Collision Foundation (M7)

> **Status:** ready
> **Prerequisite for:** Movement Scripts (M7)
> **Related:** `context/lib/scripting.md` · `context/lib/build_pipeline.md` · `context/lib/entity_model.md`

---

## Goal

Add parry3d to the engine and register PRL static geometry as a trimesh collider at level load. parry3d is used as a query-only library — no rigid bodies, joints, or integrators — so the entity_model.md §9 non-goal against physics engine integration still holds.

Collision is Rust-owned. Scripts declare movement parameters; Rust executes movement and collision internally. No script ever calls a collision query — that would be the live-VM escape hatch ruled out by scripting.md §1.

This plan establishes the collision infrastructure that the Rust-side movement system will call. Movement itself lands in the Movement Scripts plan.

---

## Capsule convention

Capsule axis is world `+Y`. Endpoints sit at `origin ± halfHeight * Y`. Movement Scripts and any future capsule consumer assume this orientation. Pin this convention in a module-level doc comment in `collision/mod.rs` — that doc comment is the M7 deliverable.

---

## Tasks

### 1. parry3d dependency + CollisionWorld

- Add `parry3d` to `[workspace.dependencies]` in the root `Cargo.toml`. Reference it as `parry3d.workspace = true` in `crates/postretro/Cargo.toml`.
- Create `crates/postretro/src/collision/mod.rs` with a `CollisionWorld` struct that owns a single `parry3d` `TriMesh` and its world-space `Isometry3<f32>`. The isometry is identity — PRL geometry is already world-space.
- Queries use `parry3d::query::*` free functions directly. No `QueryPipeline`: there is exactly one static collider.
- `CollisionWorld` lives on `App` as a plain field alongside `light_bridge`, `fog_volume_bridge`, and `emitter_bridge`. Constructed empty in `App::new()`, populated via `populate_from_level(&LevelWorld)` at level load, cleared via `clear()` in the same teardown hook as `fog_volume_bridge.clear()`. Not exposed to scripts.
- `CollisionWorld`'s `mesh` and `isometry` fields are `pub(crate)`. Callers within the engine use `parry3d::query::*` free functions against these fields directly. A higher-level query API is not added in M7.
- Engine coordinates are glam `Vec3`. Convert to nalgebra at the `CollisionWorld` build site only. nalgebra types do not appear outside `crates/postretro/src/collision/`.

### 2. PRL static geometry → trimesh

- At level load, after PRL geometry is parsed, build `points: Vec<Point3<f32>>` by mapping each `WorldVertex` to `Point3::new(v.position[0], v.position[1], v.position[2])`. Build `triangles: Vec<[u32; 3]>` by chunking `LevelWorld.indices` with `.chunks_exact(3)`. Pass both to `TriMesh::new(points, triangles)` and store the result in `CollisionWorld`.
- All triangles are included. There is no material filter in M7 — trigger or sky geometry, if emitted into the PRL static mesh, acts as solid. This is a known M7 limitation; material-aware filtering is out of scope (see Non-goals).
- Static world geometry only. No dynamic or kinematic colliders.

### 3. Update `entity_model.md` §7

- Rewrite the World Collision subsection to describe the trimesh-from-vertex-data approach. Remove the BSP brush hull claim — PRL static geometry is the source of collision triangles, not brush hulls.

### 4. Unit test — CollisionWorld floor fixture

- Add `crates/postretro/src/collision/tests.rs`. Fixture: two triangles spanning the XZ plane at y = 0, vertices at (-1, 0, -1), (1, 0, -1), (1, 0, 1), (-1, 0, 1). Ray: origin (0, 1, 0), direction (0, -1, 0). Assert time-of-impact = 1.0 and surface normal = (0, 1, 0), within 1e-5 epsilon.

---

## Acceptance criteria

- `parry3d` is a workspace dependency, used by `crates/postretro`.
- `CollisionWorld` is populated from PRL static geometry at level load and cleared on teardown, following the `fog_volume_bridge` lifecycle pattern.
- The unit test in `crates/postretro/src/collision/tests.rs` passes.
- `context/lib/entity_model.md` §7 reflects the trimesh approach; the BSP brush hull claim is removed.

---

## Non-goals

- **Material-aware collision filtering.** Trigger volumes, sky surfaces, and other non-solid material classes are not separated from solid geometry in M7. Any triangle in the PRL static mesh is solid.
- **Dynamic and kinematic colliders.** Only the static world trimesh exists. Entity-entity collision continues to use the bounding-volume scheme described in entity_model.md §7.
- **Script-visible collision queries.** No `rayCast`, `capsuleCast`, or `overlapCapsule` primitive. Scripts declare movement parameters; Rust runs collision internally. This is a structural consequence of the no-live-VM invariant in scripting.md §1.
- **Physics simulation.** No rigid bodies, joints, constraints, or integrators. parry3d is used purely as a geometric query library.
