# Task 00: Coordinate Transform

**Crate:** `postretro-level-compiler`
**File:** `src/parse.rs`
**Depends on:** nothing

---

## Context

TrenchBroom and DarkRadiant output Quake Z-up coordinates (X=right, Y=forward, Z=up). The engine uses Y-up, right-handed (X=right, Y=up, Z=back). Currently `parse.rs` converts shambler's nalgebra vectors to glam `Vec3` via a direct component mapping (`to_glam`) — no coordinate transform is applied. All downstream compiler stages and the engine receive Quake-space coordinates.

This task applies the Quake → engine transform at the parse boundary so all subsequent compiler stages and all future PRL sections work in engine-native coordinates.

---

## What to Change

In `src/parse.rs`, add a transform function and apply it to every coordinate exiting the parse stage:

```rust
// Quake: +X forward, +Y left, +Z up
// Engine: +X right, +Y up, -Z forward
// Swizzle: engine_x = -quake_y, engine_y = quake_z, engine_z = -quake_x
fn quake_to_engine(v: Vec3) -> Vec3 {
    Vec3::new(-v.y, v.z, -v.x)
}
```

This is identical to the transform in `postretro/src/bsp.rs`, which is proven correct by existing tests and by BSP maps rendering correctly. Do not invent a different swizzle — copy this one exactly.

Apply `quake_to_engine` to:
- Every vertex position in `world_faces`
- Every face normal (`Face::normal`)
- Every brush plane normal (`BrushPlane::normal`)
- Entity origins (`EntityInfo::origin`)

Do **not** transform `Face::distance` or `BrushPlane::distance`. The transform is an orthonormal rotation (pure axis swizzle), so `dot(R*n, R*p) = dot(n, p)` — the plane distance is invariant. This holds only because the transform has no scaling or translation component. If the transform is ever changed to include non-uniform scaling, plane distances would need adjustment.

**Remove the existing `quake_to_engine` call in `geometry.rs`** (line 37). That function currently applies the same transform during triangulation. After this task, coordinates are already in engine space when they reach `geometry.rs`, so the transform there would double-apply. Remove it and update any tests in `geometry.rs` that assert transformed coordinates.

The `to_glam` helper stays; it converts shambler's nalgebra type to glam. `quake_to_engine` is a second step applied after `to_glam`.

---

## Acceptance Criteria

- Unit test: `quake_to_engine(Vec3::new(0.0, 0.0, 1.0))` == `Vec3::new(0.0, 1.0, 0.0)` (Quake Z-up → engine Y-up).
- Unit test: `quake_to_engine(Vec3::new(1.0, 0.0, 0.0))` == `Vec3::new(0.0, 0.0, -1.0)` (Quake +X forward → engine -Z forward).
- Unit test: `quake_to_engine(Vec3::new(0.0, 1.0, 0.0))` == `Vec3::new(-1.0, 0.0, 0.0)` (Quake +Y left → engine -X left).
- These are the same assertions that exist in `postretro/src/bsp.rs` — they pass there and must pass in the compiler too.
- `cargo check` and `cargo test -p postretro-level-compiler` pass.
- `prl-build` compiles `assets/maps/test.map` without error. (Visual orientation will be incorrect until the engine loader is also updated in Task 06, but compilation must succeed.)

---

## Notes

Winding order: Quake faces use CCW winding viewed from outside the brush. The transform negates one axis (Y → -Y), which reverses winding. If the engine renders back-faces correctly for wireframe, this is a non-issue in early tasks. If face culling is active in the engine, the winding flip may need to be addressed. Flag this for Task 06 if it appears.
