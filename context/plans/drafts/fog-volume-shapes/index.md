# Fog Volume Shapes

## Goal

Extend fog volumes beyond AABB cuboids by adding sphere, ellipsoid, and capsule membership tests; an optional half-space clip plane for angled cuts and hemispheres; and arbitrary capsule axis orientation. Each shape is selected per-volume by a FGD KVP. The AABB remains the conservative bounding volume throughout; only the per-step membership test changes.

## Scope

### In scope

- Three new shapes: `sphere`, `ellipsoid`, `capsule`.
- `capsule` axis specified via `capsule_pitch` / `capsule_yaw` KVPs; defaults to longest-AABB-dimension auto-inference when not authored.
- Optional half-space clip plane per volume: `clip_normal` + `clip_offset` KVPs. Applies after the shape membership test for all shapes. Enables hemisphere fog (sphere + horizontal clip), angled slab cuts, and ramp-conforming fog.
- New `shape` KVP on `env_fog_volume` in the FGD; default `"box"` preserves backward compatibility.
- Shape stored as a discriminant integer in `FogVolumeRecord` and the GPU struct.
- Capsule baked fields (`capsule_axis`, `capsule_radius`, `capsule_half_height`) added to `FogVolumeRecord` and the GPU struct.
- Clip plane baked field (`clip_plane: vec4`) added to `FogVolumeRecord` and the GPU struct.
- GPU struct grows from 96 to 128 bytes; compile-time size assert updated.
- On-disk `FogVolumesSection` binary format gains the new per-record fields.
- Round-trip serialisation tests in `crates/level-format`.
- WGSL `sample_fog_volumes()` branches on shape for the membership test and applies clip plane after shape test.

### Out of scope

- Cylinder shape (open-ended capsule): covered by `ellipsoid` + `height_gradient`.
- Cone or frustum shapes.
- Shape-specific FGD UI helpers (angle widgets, radius sliders).
- Multiple clip planes per volume.

## Acceptance criteria

- [ ] An `env_fog_volume` brush with `shape "sphere"` renders fog that terminates sharply at the AABB's inscribed sphere; square corners of the brush region are fog-free.
- [ ] An `env_fog_volume` with `shape "ellipsoid"` terminates at the AABB's inscribed ellipsoid on all three axes.
- [ ] An `env_fog_volume` with `shape "capsule"` and no axis KVPs renders a pill whose axis is the longest AABB dimension.
- [ ] An `env_fog_volume` with `shape "capsule"` and explicit `capsule_pitch` / `capsule_yaw` KVPs renders a pill oriented along the specified axis regardless of brush proportions.
- [ ] An `env_fog_volume` with `shape "sphere"` and a horizontal clip plane at the sphere's equator renders a hemisphere; the upper half of the brush region is fog-free.
- [ ] A volume with no clip KVPs authored clips nothing; existing volumes without those KVPs are unaffected.
- [ ] A volume with no `shape` KVP (or `shape "box"`) behaves identically to the current implementation; existing maps are unaffected.
- [ ] `prl-build` rejects an unknown `shape` value with a descriptive error and non-zero exit code.
- [ ] `FogVolumesSection::from_bytes(section.to_bytes())` round-trips correctly for all four shapes, with and without clip plane.
- [ ] The GPU struct compile-time assert passes: `FOG_VOLUME_SIZE == 128`.

## Tasks

### Task 1: Level format — shape fields

Add `shape: u32` (discriminant), `capsule_axis: [f32; 3]`, `capsule_radius: f32`, `capsule_half_height: f32`, and `clip_plane: [f32; 4]` to `FogVolumeRecord` in `crates/level-format/src/fog_volumes.rs`.

Update `to_bytes()` and `from_bytes()` to serialise the new fields in the per-record block. Update the `MIN_RECORD_SIZE` sanity check (was 92 bytes; grows by 9 × f32 + 1 × u32 = 40 bytes → new minimum is 132 bytes for the fixed portion excluding tags; note this is the non-aligned on-disk byte count, not the GPU struct size).

Add round-trip tests for each shape value and for the clip plane, including the no-clip sentinel.

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
capsule_pitch(float) : "Capsule axis pitch (degrees, -90..90)" : "0"
capsule_yaw(float)   : "Capsule axis yaw (degrees, 0..360)" : "0"
clip_pitch(float)    : "Clip plane normal pitch (degrees, -90..90; leave absent for no clip)" : ""
clip_yaw(float)      : "Clip plane normal yaw (degrees, 0..360)" : "0"
clip_offset(float)   : "Clip plane offset (world units along clip normal)" : "0"
```

`capsule_pitch` and `capsule_yaw` are ignored for non-capsule shapes. When both are absent (or zero on a capsule), the compiler falls back to longest-AABB-dimension inference. `clip_pitch` absent (key not authored) means no clip plane — the sentinel is baked automatically. When `clip_pitch` is present, `clip_yaw` and `clip_offset` are read; the compiler converts pitch/yaw to a unit normal using the same convention as `capsule_pitch`/`capsule_yaw`. This matches the `angles` KVP pattern used by `light_spot` and `light_sun` in the same FGD.

### Task 3: Level compiler — parse + bake

`MapFogVolume` in `crates/level-compiler/src/map_data.rs` gains a `shape` field using an internal `FogVolumeShape` enum (`Box`, `Sphere`, `Ellipsoid`, `Capsule`), optional `capsule_pitch: Option<f32>` and `capsule_yaw: Option<f32>` fields, and an optional `clip_plane: Option<[f32; 4]>` field.

`resolve_fog_volume()` in `crates/level-compiler/src/parse.rs`:
- Parses `shape` KVP; unknown values → compiler error; missing → `FogVolumeShape::Box`.
- Parses `capsule_pitch` and `capsule_yaw` (degrees); stores as `Option<f32>` — `None` when the key is absent.
- Parses `clip_pitch` and `clip_yaw` (degrees) and `clip_offset`; if `clip_pitch` is absent, stores `None` (no clip); if present, converts pitch/yaw to a unit normal using the same formula as `capsule_axis` baking.

`encode_fog_volumes()` in `crates/level-compiler/src/pack.rs`:

- Maps `FogVolumeShape` to the `u32` discriminant (0=box, 1=sphere, 2=ellipsoid, 3=capsule).
- For `Capsule`: when `capsule_pitch`/`capsule_yaw` are present, computes `capsule_axis` from the angles (pitch = elevation from XZ plane, yaw = azimuth from +X). When absent, infers the axis as the longest half-extent dimension and bakes the corresponding unit vector.  Bakes `capsule_radius` as the minimum of the two non-axis half-extents (clamped ≥ 1e-6) and `capsule_half_height` as `max(projection_of_half_ext_onto_axis - capsule_radius, 0.0)`.
- For non-capsule shapes: `capsule_axis = (0,0,0)`, `capsule_radius = 0.0`, `capsule_half_height = 0.0`.
- For clip plane present: bakes `clip_plane = (nx, ny, nz, offset)`.
- For clip plane absent: bakes sentinel `(0.0, 0.0, 0.0, f32::NEG_INFINITY)` — the shader test `dot(pos, n) < d` is always false when `n` is zero.

### Task 4: CPU structs + WGSL

Replace `_pad: [f32; 2]` in `FogVolume` (`crates/postretro/src/fx/fog_volume.rs`) with `shape: u32, capsule_radius: f32`, append a 16-byte row `capsule_axis: [f32; 3], capsule_half_height: f32`, and append a final 16-byte row `clip_plane: [f32; 4]`. Update the compile-time assert to `FOG_VOLUME_SIZE == 128`.

Propagate all new fields through `FogVolumeRecord → FogVolume` packing wherever fog volumes are uploaded to the GPU buffer.

Mirror the layout change in the WGSL `FogVolume` struct (`crates/postretro/src/shaders/fog_volume.wgsl`): replace `_pad: vec2<f32>` with `shape: u32, capsule_radius: f32`, add `capsule_axis: vec3<f32>, capsule_half_height: f32` as a new struct row, and add `clip_plane: vec4<f32>` as the final row.

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

**Clip plane (all shapes):**
After the shape membership test passes, apply the half-space cut: `if dot(pos, v.clip_plane.xyz) < v.clip_plane.w { continue; }`. When the sentinel `(0,0,0,-inf)` is stored, the dot product is zero and the test is always false — no clipping. No branching on a "has clip" flag is needed.

The slab-clip prologue (`v.min` / `v.max_v` ray-interval computation) requires **no changes** — the AABB serves as a valid conservative bound for all shapes, and the clip plane only removes density inside that bound.

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

GPU struct after change — 8 × 16-byte rows, 128 bytes total:

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
    clip_plane: vec4<f32>,
}
```

`_pad: vec2<f32>` (rows 6) becomes `shape: u32, capsule_radius: f32`. Rows 7–8 are new.

Capsule axis baking (pseudo, in `encode_fog_volumes`):
```rust
// Proposed design — remove after implementation
// When capsule_pitch / capsule_yaw are authored:
let pitch = pitch_deg.to_radians();
let yaw   = yaw_deg.to_radians();
let capsule_axis = Vec3::new(
    pitch.cos() * yaw.cos(),
    pitch.sin(),
    pitch.cos() * yaw.sin(),
).normalize();

// Fallback when KVPs are absent — infer from longest AABB dimension:
let (axis_idx, _) = [half_ext.x, half_ext.y, half_ext.z]
    .iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap();
let capsule_axis = [Vec3::X, Vec3::Y, Vec3::Z][axis_idx];
```

Clip plane sentinel (baked when `clip_normal` KVP is absent):
```rust
// Proposed design — remove after implementation
let clip_plane = [0.0_f32, 0.0, 0.0, f32::NEG_INFINITY];
// dot(pos, (0,0,0)) = 0, which is never < -inf → no clipping.
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
| `clip_plane_nx/ny/nz` | 3 × `f32` LE | World-space normal; `(0,0,0)` = no clip |
| `clip_plane_d` | `f32` LE | Offset; `f32::NEG_INFINITY` when no clip authored |

Previous format emitted no shape field. Loading an old PRL at this offset reads into whatever bytes follow `inv_height_extent` — which is `tag_count`. The level-format crate has no version field, so old PRLs must be recompiled after this change. Existing maps compiled before this change will be invalid; they must be rebuilt with `prl-build`. This is acceptable per the project's pre-release API policy.

`MIN_RECORD_SIZE` in `from_bytes()` must increase from 92 to 132 bytes per record (adds 10 × 4 = 40 bytes to the fixed payload).

## Open questions

- ~~**Capsule radius tie-breaking:**~~ Resolved: use `min` of the two non-axis half-extents — tighter fit.
- ~~**`height_fade` applicability:**~~ Resolved: keep `height_fade` for sphere/ellipsoid; author opts in by setting `height_gradient > 0`.
- ~~**`radial_falloff` for capsule:**~~ Resolved: reuse existing `radial_falloff` field; `dist / capsule_radius` as `radial_t`, consistent with sphere/ellipsoid.
- ~~**Clip plane authoring interface:**~~ Resolved: use `clip_pitch`/`clip_yaw` angle KVPs, consistent with `capsule_pitch`/`capsule_yaw` and the `angles` pattern used by `light_spot`/`light_sun` in this FGD. Authors think in degrees, not world-space vectors.
- ~~**Capsule axis when both KVPs are zero:**~~ Resolved: `Option`-based — if neither key is authored, auto-infer from longest AABB dimension; if either is present (even at value `0`), use the explicit angle.
