# Task 02: Unit Conversion at PRL Parse Boundary

**Crate:** `postretro-level-compiler`
**Files:** `src/parse.rs`, `src/map_format.rs`
**Depends on:** Task 01

---

## Context

Engine canonical unit: 1 unit = 1 meter. Quake/idTech maps are in Quake units (1 unit ≈ 1 inch). Conversion factor: `0.0254` (exact). The parse boundary in `parse.rs` is the correct application point — all downstream compiler stages (BSP, portal generation, geometry packing) receive coordinates in meters.

**Plane distance invariant has changed.** The prior `quake_to_engine` was a pure rotation; prior docs and comments said "do not transform distance fields" because `dot(R*n, R*p) = dot(n, p)` for orthonormal R. Adding a uniform scale invalidates this. A plane `n·x = d` with `x` in Quake units becomes `n·x' = d * 0.0254` where `x'` is in meters. Plane distances must be explicitly scaled. Every comment repeating the old "do not transform distances" rule must be updated.

---

## What to Change

In `src/map_format.rs`, add to `MapFormat`:

```rust
// Proposed design — remove after implementation

impl MapFormat {
    /// Scale factor from map-native units to engine meters.
    pub fn units_to_meters(&self) -> f32 {
        match self {
            Self::IdTech2 => 0.0254,
            // IdTech3 and IdTech4 are unsupported — unreachable in practice
            // because is_supported() gates entry. Add their scales when implemented.
            _ => unreachable!(),
        }
    }
}
```

In `src/parse.rs`:
- Replace the hardcoded `quake_to_engine` with one that accepts a scale parameter, or derive it from `MapFormat::units_to_meters()`. Either way, the scale must come from `MapFormat`, not from a magic literal in `parse.rs`.
- Apply scale to: vertex positions, entity origins.
- Normals: swizzle only. Do **not** multiply normals by `units_to_meters`. Normals are direction vectors; re-normalize after swizzle if needed.
- Plane distances (`Face::distance`, `BrushPlane::distance`): multiply explicitly by `units_to_meters()` at each assignment site. These scalars do not pass through `quake_to_engine`.

Update all affected comments. The note at `parse.rs:30-31` currently reads "Do not transform `distance` fields" — revise to explain the new behavior.

Update tests in `parse.rs`:
- Coordinate transform assertions now include the 0.0254 scale. Example: `quake_to_engine(Vec3::new(0.0, 0.0, 1.0))` → `Vec3::new(0.0, 0.0254, 0.0)`.
- `faces_have_unit_normals` must still pass — normals must remain unit length.
- Add a distance scaling test: a face plane with Quake distance `64.0` outputs a plane distance of `1.6256` (64 × 0.0254).

Also update `context/lib/build_pipeline.md`:
- Add canonical unit declaration: "Engine canonical unit: 1 unit = 1 meter."
- Update the PRL compilation section to note the scale is applied at the parse boundary alongside the axis swizzle.
- Update the key invariant to replace "all maps are authored in TrenchBroom" with a statement that leaves room for other editors (e.g., "maps are authored in idTech-compatible editors; TrenchBroom is the default").

---

## Acceptance Criteria

- `prl-build assets/maps/test.map` produces a `.prl` with positions in meters. A 64-unit cube brush outputs edge vertices at 0 and 1.6256 m (64 × 0.0254).
- Plane normals remain unit length — scale not applied to normals.
- Plane distances are consistent with scaled vertex positions — no shear or geometry misalignment.
- All coordinate transform tests updated and passing.
- `cargo test -p postretro-level-compiler` passes.
