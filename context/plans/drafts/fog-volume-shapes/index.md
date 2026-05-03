# Fog Volume Shapes

## Goal

Extend fog volumes beyond AABB cuboids by adding sphere, ellipsoid, and capsule membership tests. Each shape is selected per-volume by a FGD KVP. The AABB remains the conservative bounding volume throughout; only the per-step membership test changes.

## Scope

### In scope

- Three new shapes: `sphere`, `ellipsoid`, `capsule`.
- `capsule` axis auto-inferred from the longest AABB dimension at compile time.
- New `shape` KVP on `env_fog_volume` in the FGD; default `"box"` preserves backward compatibility.
- Shape stored as a discriminant integer in `FogVolumeRecord` and the GPU struct.
- Capsule baked fields (`capsule_axis`, `capsule_radius`, `capsule_half_height`) added to `FogVolumeRecord` and the GPU struct.
- GPU struct grows from 96 to 112 bytes; compile-time size assert updated.
- On-disk `FogVolumesSection` binary format gains the new per-record fields.
- Round-trip serialisation tests in `crates/level-format`.
- WGSL `sample_fog_volumes()` branches on shape for the membership test and falloff evaluation.

### Out of scope

- Arbitrary-angle capsule rotation via KVP (axis is always one of ±X, ±Y, ±Z).
- Cylinder shape (open-ended capsule): covered by `ellipsoid` + `height_gradient`.
- Cone or frustum shapes.
- Shape-specific FGD UI helpers (angles, radius sliders).

## Acceptance criteria

- [ ] An `env_fog_volume` brush with `shape "sphere"` renders fog that terminates sharply at the AABB's inscribed sphere; square corners of the brush region are fog-free.
- [ ] An `env_fog_volume` with `shape "ellipsoid"` terminates at the AABB's inscribed ellipsoid on all three axes.
- [ ] An `env_fog_volume` with `shape "capsule"` renders a pill with hemispherical caps; the capsule axis is the longest AABB dimension.
- [ ] A volume with no `shape` KVP (or `shape "box"`) behaves identically to the current implementation; existing maps are unaffected.
- [ ] `prl-build` rejects an unknown `shape` value with a descriptive error and non-zero exit code.
- [ ] `FogVolumesSection::from_bytes(section.to_bytes())` round-trips correctly for all four shapes.
- [ ] The GPU struct compile-time assert passes: `FOG_VOLUME_SIZE == 112`.

## Tasks

### Task 1: Level format — shape fields

Add `shape: u32` (discriminant), `capsule_axis: [f32; 3]`, `capsule_radius: f32`, and `capsule_half_height: f32` to `FogVolumeRecord` in `crates/level-format/src/fog_volumes.rs`.

Update `to_bytes()` and `from_bytes()` to serialise the new fields in the per-record block. Update the `MIN_RECORD_SIZE` sanity check (was 92 bytes; grows by 5 × f32 + 1 × u32 = 24 bytes → new minimum is 116 bytes for the fixed portion excluding tags; note this is the non-aligned on-disk byte count, not the GPU struct size).

Add round-trip tests for each shape value.

### Task 2: FGD — shape KVP

In `sdk/TrenchBroom/postretro.fgd`, add to `env_fog_volume`:

```
shape(choices) : "Volume shape" : "box" =
[
    "box"       : "Box (AABB)"
    "sphere"    : "Sphere"
    "ellipsoid" : "Ellipsoid"
    "capsule"   : "Capsule (pill)"
]
```

### Task 3: Level compiler — parse + bake

`MapFogVolume` in `crates/level-compiler/src/map_data.rs` gains a `shape` field using an internal `FogVolumeShape` enum (`Box`, `Sphere`, `Ellipsoid`, `Capsule`).

`resolve_fog_volume()` in `crates/level-compiler/src/parse.rs` parses the `shape` KVP. Unknown values → compiler error. Missing key → `FogVolumeShape::Box`.

`encode_fog_volumes()` in `crates/level-compiler/src/pack.rs`:

- Maps `FogVolumeShape` to the `u32` discriminant (0=box, 1=sphere, 2=ellipsoid, 3=capsule).
- For `Capsule`: infers the axis as the longest half-extent dimension; bakes `capsule_axis` as the corresponding unit vector (e.g. `(0,1,0)` if Y is longest), `capsule_radius` as the half-extent of the shorter dimensions (minimum of the two non-axis half-extents, clamped ≥ 1e-6), and `capsule_half_height` as `max(half_ext_along_axis - capsule_radius, 0.0)`.
- For non-capsule shapes: `capsule_axis = (0,0,0)`, `capsule_radius = 0.0`, `capsule_half_height = 0.0`.

### Task 4: CPU structs + WGSL

Replace `_pad: [f32; 2]` in `FogVolume` (`crates/postretro/src/fx/fog_volume.rs`) with `shape: u32, capsule_radius: f32`, and append a new 16-byte row `capsule_axis: [f32; 3], capsule_half_height: f32`. Update the compile-time assert to `FOG_VOLUME_SIZE == 112`.

Propagate the new fields through `FogVolumeRecord → FogVolume` packing wherever fog volumes are uploaded to the GPU buffer.

Mirror the layout change in the WGSL `FogVolume` struct (`crates/postretro/src/shaders/fog_volume.wgsl`): replace `_pad: vec2<f32>` with `shape: u32, capsule_radius: f32`, add `capsule_axis: vec3<f32>, capsule_half_height: f32` as a new struct row.

### Task 5: Shader — shape-branched membership

Rewrite the inner body of `sample_fog_volumes()` in `fog_volume.wgsl` to branch on `v.shape`:

**Box (0) — unchanged:**
AABB gate → `box_fade` × `height_fade` × `radial_fade`.

**Sphere (1):**
Skip the AABB component test (the slab-clip prologue has already bounded the interval conservatively). Compute `dist = length(pos - v.center)`. If `dist > v.half_diag`, skip. `radial_t = dist / max(v.half_diag, 1e-6)`. Apply `radial_fade` with `v.radial_falloff` (soft interior) and `height_fade` as before; drop `box_fade`.

**Ellipsoid (2):**
Compute `local = (pos - v.center) * v.inv_half_ext`. If `dot(local, local) > 1.0`, skip. `radial_t = length(local)`. Apply `radial_fade` with `v.radial_falloff`; apply `height_fade`; drop `box_fade`.

**Capsule (3):**
Project `(pos - v.center)` onto `v.capsule_axis`; clamp to `[-v.capsule_half_height, v.capsule_half_height]`; nearest-on-axis = `v.center + v.capsule_axis × clamped`. `dist = length(pos - nearest_on_axis)`. If `dist > v.capsule_radius`, skip. `radial_t = dist / max(v.capsule_radius, 1e-6)`. Apply `radial_fade` with `v.radial_falloff`; apply `height_fade`; drop `box_fade`.

The slab-clip prologue (`v.min` / `v.max_v` ray-interval computation) requires **no changes** — the AABB serves as a valid conservative bound for all shapes.

## Sequencing

**Phase 1 (sequential):** Task 1 (level format) — wire format must be defined before compiler or runtime can use new fields.

**Phase 2 (concurrent):** Task 2 (FGD), Task 3 (level compiler), Task 4 + 5 (CPU structs + WGSL) — all consume the Task 1 format; Tasks 2–5 are mutually independent.

## Rough sketch

Shape discriminant values (canonical):

| Value | Shape |
|-------|-------|
| 0 | box (default) |
| 1 | sphere |
| 2 | ellipsoid |
| 3 | capsule |

GPU struct after change — 7 × 16-byte rows, 112 bytes total:

```
// Proposed design — remove after implementation
struct FogVolume {
    min: vec3<f32>, density: f32,
    max_v: vec3<f32>, falloff: f32,
    color: vec3<f32>, scatter: f32,
    center: vec3<f32>, half_diag: f32,
    inv_half_ext: vec3<f32>, inv_height_extent: f32,
    height_gradient: f32, radial_falloff: f32, shape: u32, capsule_radius: f32,
    capsule_axis: vec3<f32>, capsule_half_height: f32,
}
```

`capsule_radius` in the new row reuses the space formerly occupied by `_pad[0]`; `_pad[1]` becomes `capsule_radius` in the last row holds `capsule_half_height` as the trailing scalar.

Capsule axis inference (pseudo, in `encode_fog_volumes`):
```rust
// Proposed design — remove after implementation
let half_ext = ((max - min) * 0.5).max(Vec3::splat(1e-6));
let (axis_idx, _) = [half_ext.x, half_ext.y, half_ext.z]
    .iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap();
let capsule_axis = [Vec3::X, Vec3::Y, Vec3::Z][axis_idx];
let capsule_radius = [half_ext.y.min(half_ext.z),
                      half_ext.x.min(half_ext.z),
                      half_ext.x.min(half_ext.y)][axis_idx].max(1e-6);
let capsule_half_height = (half_ext[axis_idx] - capsule_radius).max(0.0);
```

## Boundary inventory

| Name | Rust (`MapFogVolume`) | Rust (`FogVolumeRecord`) | Wire (on-disk u32) | WGSL (`FogVolume`) | FGD KVP value |
|---|---|---|---|---|---|
| Box | `FogVolumeShape::Box` | `shape: 0` | `0u32` | `shape == 0u` | `"box"` |
| Sphere | `FogVolumeShape::Sphere` | `shape: 1` | `1u32` | `shape == 1u` | `"sphere"` |
| Ellipsoid | `FogVolumeShape::Ellipsoid` | `shape: 2` | `2u32` | `shape == 2u` | `"ellipsoid"` |
| Capsule | `FogVolumeShape::Capsule` | `shape: 3` | `3u32` | `shape == 3u` | `"capsule"` |

## Wire format

On-disk `FogVolumeRecord` per-entry layout gains these fields **after** `inv_height_extent` and **before** `tag_count`:

| Field | Type | Notes |
|-------|------|-------|
| `shape` | `u32` LE | Discriminant; 0=box (default), 1=sphere, 2=ellipsoid, 3=capsule |
| `capsule_radius` | `f32` LE | Zero for non-capsule shapes |
| `capsule_axis_x/y/z` | 3 × `f32` LE | Unit vector; zero for non-capsule shapes |
| `capsule_half_height` | `f32` LE | Zero for non-capsule shapes |

Previous format emitted no shape field. Loading an old PRL at this offset reads into whatever bytes follow `inv_height_extent` — which is `tag_count`. The level-format crate has no version field, so old PRLs must be recompiled after this change. Existing maps compiled before this change will be invalid; they must be rebuilt with `prl-build`. This is acceptable per the project's pre-release API policy.

`MIN_RECORD_SIZE` in `from_bytes()` must increase from 92 to 116 bytes per record (adds 6 × 4 = 24 bytes to the fixed payload).

## Open questions

- **Capsule radius tie-breaking:** when two non-axis dimensions are equal, the `min` of the two is used as radius. If authors expect the brush to define a specific radius, the `min` rule preserves the tighter fit. A `max` rule would be looser. Current proposal: `min`.
- **`height_fade` applicability:** for sphere and ellipsoid, `height_fade` is applied (same as box). This lets authors use `height_gradient` with non-box shapes. If this is confusing UX, disable `height_fade` for sphere/ellipsoid and force `height_fade = 1.0`. Current proposal: keep it, since it's author-controlled via `height_gradient = 0.0` default.
- **`radial_falloff` for capsule:** the capsule uses `dist / capsule_radius` as `radial_t`. Setting `radial_falloff > 0` gives a soft interior. This matches the sphere/ellipsoid behavior. If capsule authors prefer a separate `edge_falloff` parameter, that's a follow-on.
