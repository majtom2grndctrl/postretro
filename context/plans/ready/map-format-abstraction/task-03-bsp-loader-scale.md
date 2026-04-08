# Task 03: BSP Loader Unit Scale

**Crate:** `postretro` (engine)
**Files:** `src/bsp.rs`
**Depends on:** nothing (independent of Tasks 01–02)

---

## Context

The engine loads BSP files via `src/bsp.rs`. Like the PRL compiler, `bsp.rs` applies the idTech axis swizzle at load time via a local `quake_to_engine`. It does not apply a unit scale. After this task, BSP geometry will be in meters — matching the PRL output from Task 02. Both loading paths must agree on canonical units or maps will render at different scales depending on which format is loaded.

`bsp.rs` already has a comment near plane distance handling (around line 272) acknowledging that the swizzle affects the dot product. Read that section before modifying — the scale changes the analysis there too.

There is no `MapFormat` enum in the engine and none is needed. BSP files loaded by this engine always come from ericw-tools and are always idTech format. The scale is a constant.

---

## What to Change

In `src/bsp.rs`, add a scale constant and apply it:

```rust
// Proposed design — remove after implementation

const QUAKE_TO_METERS: f32 = 0.0254;

fn quake_to_engine(v: Vec3) -> Vec3 {
    Vec3::new(-v.y, v.z, -v.x) * QUAKE_TO_METERS
}
```

Vertex positions and bounding box min/max already pass through `quake_to_engine` — those sites pick up the scale automatically without further changes.

Plane distances: find every site in `bsp.rs` that reads a plane distance from BSP data and multiply by `QUAKE_TO_METERS`. These do not pass through `quake_to_engine`. Read the existing comment around line 272 to understand the current plane distance handling, then update it to reflect the scale.

Update tests in `bsp.rs`:
- `coordinate_transform_z_up_to_y_up`, `coordinate_transform_forward_axis`, `coordinate_transform_left_axis`: expected values now include the 0.0254 scale.
- `coordinate_transform_preserves_distance`: currently asserts `|original| == |transformed|` (magnitudes equal under pure rotation). With scale, this becomes `|transformed| == |original| * QUAKE_TO_METERS`. Update the assertion and the test comment.
- `coordinate_transform_roundtrip_orthogonality`: verify this test still makes sense after scale is added. A scaled rotation is not orthogonal in the strict sense — update or replace if the assertion breaks.

---

## Acceptance Criteria

- `cargo run -- assets/maps/test.bsp` renders at the same scale as a `.prl` compiled after Task 02.
- Plane distances are consistent with scaled vertex positions — no geometry misalignment.
- `cargo test -p postretro` passes, including all updated coordinate transform tests.
