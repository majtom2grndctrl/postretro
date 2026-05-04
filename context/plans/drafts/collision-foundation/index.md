# Collision Foundation (M7)

> **Status:** draft
> **Depends on:** scripting-primitives-folder refactor — ready
> **Prerequisite for:** Movement Scripts (M7)
> **Related:** `context/lib/scripting.md` · `context/lib/build_pipeline.md`

---

## Goal

Add parry3d to the engine, register PRL static geometry as a trimesh collider at level load, and expose a full collision primitive set to scripts. No movement logic — this plan establishes the collision infrastructure that movement scripts will call.

---

## Tasks

### 1. parry3d dependency + CollisionWorld

- Add `parry3d` to `crates/postretro/Cargo.toml`.
- Create `crates/postretro/src/collision/mod.rs` with a `CollisionWorld` struct that owns the Parry pipeline and the static trimesh handle.
- `CollisionWorld` is constructed at level load and dropped on level unload.
- Store `CollisionWorld` on the engine's level state struct so behavior-scope primitives can reach it via the same `Rc<RefCell<_>>` handle pattern used by existing primitives.

### 2. PRL static geometry → trimesh collider

- At level load, after PRL geometry is parsed, iterate static draw chunks and collect their triangle lists. Skip non-solid materials (transparent surfaces, sky faces, trigger-only materials).
- Build a Parry `TriMesh` from the collected triangles and insert it into `CollisionWorld`.
- Static world geometry only — no dynamic or kinematic colliders at this stage.

### 3. Collision primitives

Register in `scripting/primitives/collision.rs` (behavior-scope):

| Primitive | Parameters | Returns |
|-----------|------------|---------|
| `capsuleCast` | `origin: Vec3, dir: Vec3, radius: number, halfHeight: number, maxDist: number` | `CastHit` or null/nil |
| `rayCast` | `origin: Vec3, dir: Vec3, maxDist: number` | `RayHit` or null/nil |
| `overlapCapsule` | `origin: Vec3, radius: number, halfHeight: number` | `boolean` |

`CastHit` and `RayHit` are plain objects/tables: `{ point: Vec3, normal: Vec3, distance: number }`. Miss returns `null` (QuickJS) / `nil` (Luau). `Vec3` marshalling follows the convention established by existing primitives.

`dir` must be a unit vector. Non-unit input is undefined behavior — document the requirement; do not silently normalize engine-side.

All three dispatch into `CollisionWorld`. Missing or invalid arguments surface as `ScriptError`.

### 4. SDK type definitions + documentation

- Run `cargo run -p postretro --bin gen-script-types` after task 3 to regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau`.
- Add a **Collision** section to `docs/scripting-reference.md` covering all three primitives: parameters, return shapes, a worked example for each, and the `dir` normalization requirement.
- Drift-detection test must pass.

---

## Acceptance criteria

- `capsuleCast`, `rayCast`, and `overlapCapsule` callable from both TypeScript and Luau scripts against a loaded PRL level.
- All three primitives documented in `docs/scripting-reference.md`.
- SDK drift-detection test passes.
- Static world geometry blocks a capsule cast — verified by a unit test constructing a minimal `CollisionWorld` with a known floor.
