# Fog Volume Shapes

## Goal

Extend fog volumes beyond AABB cuboids by adding sphere, ellipsoid, and capsule membership tests; an optional half-space clip plane for angled cuts and hemispheres; and arbitrary capsule axis orientation. Each shape is selected per-volume by a FGD KVP. The AABB remains the conservative bounding volume throughout; only the per-step membership test changes.

## Scope

### In scope

- Three new shapes: `sphere`, `ellipsoid`, `capsule`.
- `capsule` axis specified via `capsule_pitch` / `capsule_yaw` KVPs; defaults to longest-AABB-dimension auto-inference when not authored.
- Optional half-space clip plane per volume: `clip_pitch` + `clip_yaw` + `clip_offset` KVPs. Applies after the shape membership test for all shapes. Enables hemisphere fog (sphere + horizontal clip), angled slab cuts, and ramp-conforming fog.
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
- [ ] An `env_fog_volume` with `shape "capsule"` and explicit `capsule_pitch` / `capsule_yaw` KVPs renders a pill oriented along the specified axis regardless of brush proportions. The capsule fits strictly inside the AABB; for awkward axis/brush combinations (e.g., a near-diagonal axis in a near-cubic brush) the pill may degenerate to the inscribed sphere (`half_height = 0`) — this is a valid result, not an error.
- [ ] An `env_fog_volume` with `shape "sphere"` and a horizontal clip plane at the sphere's equator renders a hemisphere; the upper half of the brush region is fog-free.
- [ ] A volume with no clip KVPs authored clips nothing; existing volumes without those KVPs are unaffected.
- [ ] A volume with no `shape` KVP (or `shape "box"`) behaves identically to the current implementation; existing maps are unaffected.
- [ ] `prl-build` rejects an unknown `shape` value with a descriptive error and non-zero exit code.
- [ ] `FogVolumesSection::from_bytes(section.to_bytes())` round-trips correctly for all four shapes, with and without clip plane.
- [ ] The GPU struct compile-time assert passes: `FOG_VOLUME_SIZE == 128`.

## Tasks

### Task 1: Level format — shape fields

Add `shape: u32` (discriminant), `capsule_axis: [f32; 3]`, `capsule_radius: f32`, `capsule_half_height: f32`, and `clip_plane: [f32; 4]` to `FogVolumeRecord` in `crates/level-format/src/fog_volumes.rs`.

Update `to_bytes()` and `from_bytes()` to serialise the new fields in the per-record block. Update the `MIN_RECORD_SIZE` sanity check (was 92 bytes; grows by 10 × 4-byte fields = 40 bytes (the 10 scalars: `shape`, `capsule_radius`, `capsule_axis_x/y/z`, `capsule_half_height`, `clip_plane_nx/ny/nz`, `clip_plane_d`) → new minimum is 132 bytes for the fixed portion excluding tags; note this is the non-aligned on-disk byte count, not the GPU struct size). `MIN_RECORD_SIZE` is a local `const` inside `from_bytes()` — it does not need to be promoted to module scope.

When deserialising `clip_plane_d`, read the 4 bytes raw with `f32::from_le_bytes(...)` without going through `read_f32` — the `f32::INFINITY` sentinel must bypass the finite check `read_f32` applies to all other fields. Add a comment at the read site: `// sentinel: INFINITY when no clip — bypass finite check`. The round-trip test for the no-clip case must assert the sentinel survives encode→decode unchanged.

Add round-trip tests covering all 8 (shape × clip) combinations — one test per (box/sphere/ellipsoid/capsule) × (clip present/absent). Each clip-absent test must assert that the sentinel value in `clip_plane[3]` survives encode→decode unchanged.

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
capsule_pitch(float) : "Capsule axis pitch (degrees, -90..90; leave key absent to auto-orient along longest brush axis)" : "0"
capsule_yaw(float)   : "Capsule axis yaw (degrees, 0..360; leave key absent to auto-orient along longest brush axis)" : "0"
clip(choices)        : "Clip plane" : "none" =
[
    "none"  : "Off (no clip)"
    "plane" : "Half-space"
]
clip_pitch(float)    : "Clip plane normal pitch (degrees, -90..90; used when clip is 'plane')" : "0"
clip_yaw(float)      : "Clip plane normal yaw (degrees, 0..360; used when clip is 'plane')" : "0"
clip_offset(float)   : "Clip plane offset from volume center (world units along clip normal; 0 = cut through center; used when clip is 'plane')" : "0"
```

The compiler detects the no-clip case from `clip == "none"` (the default). Authors who want a clip plane set `clip "plane"` and configure `clip_pitch`, `clip_yaw`, `clip_offset`; all three default to `0` and are ignored when `clip == "none"`.

`capsule_pitch` and `capsule_yaw` are ignored for non-capsule shapes. When both keys are absent from the entity, the compiler falls back to longest-AABB-dimension inference. When `clip == "plane"`, `clip_pitch`, `clip_yaw`, and `clip_offset` are read and converted to a clip plane normal using the same formula as `capsule_axis` baking. This matches the `angles` KVP pattern used by `light_spot` and `light_sun` in the same FGD.

**Clip plane convention.** The clip plane normal points INTO the removed region (the cut-away half-space). The shader discards samples where `dot(pos, n) > d`; fog persists where `dot(pos, n) <= d`. `clip_pitch` / `clip_yaw` define the normal in engine coordinates (Y-up): `pitch=0, yaw=0` → +X; `pitch=+90` → +Y (up); `pitch=-90` → −Y (down). `clip_offset` is a **center-relative** delta: the compiler bakes `d = dot(center, n) + clip_offset`, so `clip_offset "0"` always cuts through the volume center regardless of world position.

**Worked example (AC #5 — sphere with fog in lower hemisphere).** Set `clip "plane"`, `clip_pitch "+90"` (n = (0, +1, 0) — up-facing, pointing into the removed upper half) and `clip_offset "0"`. The compiler bakes `d = dot(center, +Y) + 0 = cy`. The shader keeps `dot(pos, +Y) <= cy` → `pos.y <= cy` — fog in the lower half, upper half fog-free. No arithmetic required from the level designer.

### Task 3: Level compiler — parse + bake

`MapFogVolume` in `crates/level-compiler/src/map_data.rs` gains a `shape` field using an internal `FogVolumeShape` enum (`Box`, `Sphere`, `Ellipsoid`, `Capsule`), optional `capsule_pitch: Option<f32>` and `capsule_yaw: Option<f32>` fields, and an optional `clip_plane: Option<[f32; 4]>` field.

`resolve_fog_volume()` in `crates/level-compiler/src/parse.rs`:
- Parses `shape` KVP; unknown values → compiler error; missing → `FogVolumeShape::Box`.
- Parses `capsule_pitch` and `capsule_yaw` (degrees); stores as `Option<f32>` — `None` when the key is absent. If only one of the two keys is present, treat the absent key as `0.0` for the angle conversion.
- Parses `clip_pitch` and `clip_yaw` (degrees) and `clip_offset`; if `clip == "none"` (or key absent), stores `None` (no clip) — `clip_pitch`, `clip_yaw`, `clip_offset` are ignored; if `clip == "plane"`, reads `clip_pitch`, `clip_yaw`, `clip_offset`; converts pitch/yaw to a unit normal using the same formula as `capsule_axis` baking.

`encode_fog_volumes()` in `crates/level-compiler/src/pack.rs`:

- Maps `FogVolumeShape` to the `u32` discriminant (0=box, 1=sphere, 2=ellipsoid, 3=capsule).
- For `Capsule`: when `capsule_pitch`/`capsule_yaw` are present, computes `capsule_axis` from the angles (pitch = elevation from XZ plane, yaw = azimuth from +X). If either `capsule_pitch` or `capsule_yaw` is `Some`, use the explicit angles for both (substituting `0.0` for `None`); if both are `None`, infers the axis as the longest half-extent dimension and bakes the corresponding unit vector. Bakes `capsule_radius` and `capsule_half_height` from the unit axis `a` and the AABB half-extents `H = (hx, hy, hz)` such that the capsule fits strictly inside the AABB. The fit constraint is `half_height·|a_i| + radius ≤ h_i` for each axis `i ∈ {x,y,z}` (the per-axis maximum extent of the capsule equals the endpoint displacement plus the sphere radius). Bake `capsule_radius = max(min(hx, hy, hz), 1e-6)` (the AABB's inscribed-sphere radius, clamped) and `capsule_half_height = max(0.0, min over {i : |a_i| > 1e-6} of (h_i - capsule_radius) / |a_i|)`. This formula matches the cardinal auto-infer case (e.g., `a = (1,0,0)` with `hx ≥ hy, hz` gives `r = min(hy, hz)`, `half_height = hx - r`) and degrades gracefully for arbitrary axes (e.g., `a = (1,1,0)/√2` in a cube degenerates to the inscribed sphere with `half_height = 0`; the same axis in an elongated XY brush gives a useful 45° pill).
- For non-capsule shapes: `capsule_axis = (0,0,0)`, `capsule_radius = 0.0`, `capsule_half_height = 0.0`.
- For clip plane present: bakes `clip_plane = (nx, ny, nz, dot(center, n) + clip_offset)` — `clip_offset` is a delta from the volume center's projection along the normal, so `clip_offset "0"` always cuts through the center.
- For clip plane absent: bakes sentinel `(0.0, 0.0, 0.0, f32::INFINITY)` — the shader test `dot(pos, n) > d` is always false when `n` is zero and `d` is +∞.

### Task 4: CPU structs + WGSL

Delete `_pad: [f32; 2]` in `FogVolume` (`crates/postretro/src/fx/fog_volume.rs`) and replace it in the same struct slot with `shape: u32, capsule_radius: f32`, append a 16-byte row `capsule_axis: [f32; 3], capsule_half_height: f32`, and append a final 16-byte row `clip_plane: [f32; 4]`. Update the compile-time assert to `FOG_VOLUME_SIZE == 128`.

Propagate all new fields through `FogVolumeRecord → FogVolume` packing in `update_volumes()` in `crates/postretro/src/scripting/systems/fog_volume_bridge.rs`. Copy `clip_plane.w` bit-for-bit — do not apply any finite clamping; the `f32::INFINITY` sentinel must survive the pack.

Mirror the layout change in the WGSL `FogVolume` struct (`crates/postretro/src/shaders/fog_volume.wgsl`): delete `_pad: vec2<f32>` and replace it in the same struct slot with `shape: u32, capsule_radius: f32`, add `capsule_axis: vec3<f32>, capsule_half_height: f32` as a new struct row, and add `clip_plane: vec4<f32>` as the final row. WGSL field declaration order must match the Rust struct field order exactly.

Update the existing test `pack_fog_volumes_round_trips_all_baked_fields` in `crates/postretro/src/fx/fog_volume.rs` — it constructs a `FogVolume` literal that will not compile once the struct gains new fields.

### Task 5: Shader — shape-branched membership

Rewrite the inner body of `sample_fog_volumes()` in `fog_volume.wgsl` to branch on `v.shape`:

**Box (0) — unchanged:**
AABB gate → `box_fade` × `height_fade` × `radial_fade`.

Note: `falloff` is the source of `box_fade` and is a no-op for the three new shapes; authors setting it on a sphere/ellipsoid/capsule volume see no effect.

**Sphere (1):**
The inscribed-sphere `dist > r` check subsumes the per-component AABB test; skip the AABB component test. Derive the inscribed radius from existing struct data: `let r = min(min(1.0/v.inv_half_ext.x, 1.0/v.inv_half_ext.y), 1.0/v.inv_half_ext.z)`. Compute `dist = length(pos - v.center)`. If `dist > r`, skip. `radial_t = dist / max(r, 1e-6)`. Apply `radial_fade` with `v.radial_falloff` (soft interior) and `height_fade` as before; drop `box_fade`.

**Ellipsoid (2):**
Compute `local = (pos - v.center) * v.inv_half_ext`. If `dot(local, local) > 1.0`, skip. `radial_t = length(local)`. Apply `radial_fade` with `v.radial_falloff`; apply `height_fade`; drop `box_fade`.

**Capsule (3):**
Project `(pos - v.center)` onto `v.capsule_axis`; clamp to `[-v.capsule_half_height, v.capsule_half_height]`; nearest-on-axis = `v.center + v.capsule_axis × clamped`. `dist = length(pos - nearest_on_axis)`. If `dist > v.capsule_radius`, skip. `radial_t = dist / max(v.capsule_radius, 1e-6)`. Apply `radial_fade` with `v.radial_falloff`; apply `height_fade`; drop `box_fade`.

**Clip plane (all shapes):**
After the shape membership test passes, apply the half-space cut: `if dot(pos, v.clip_plane.xyz) > v.clip_plane.w { continue; }`. When the sentinel `(0,0,0,+inf)` is stored, the dot product is zero and the test is always false — no clipping. No branching on a "has clip" flag is needed.

The slab-clip prologue (`v.min` / `v.max_v` ray-interval computation) requires **no changes** — the AABB serves as a valid conservative bound for all shapes, and the clip plane only removes density inside that bound. The clip-plane test is shape-independent (it runs after the per-shape membership check), so AC #5 (sphere + clip) is the behavioral verification for the clip path across all four shapes.

## Sequencing

**Phase 1 (sequential):** Task 1 (level format) — wire format must be defined before compiler or runtime can use new fields.

**Phase 2 (concurrent):** Task 2 (FGD), Task 3 (level compiler), Task 4 + 5 (CPU structs + WGSL) — all consume the Task 1 format; Tasks 2–5 are mutually independent. Tasks 4 and 5 must be implemented atomically — the Rust GPU struct and WGSL struct must change in the same commit to keep layouts in sync.

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

// Capsule radius + half_height from unit axis `a` and AABB half-extents `h`.
// Constraint (necessary and sufficient for capsule ⊆ AABB):
//     half_height * |a_i| + radius <= h_i   for each i in {x,y,z}
// Pick radius = inscribed-sphere radius, then maximize half_height under the constraint.
let abs_a = capsule_axis.abs();
let capsule_radius = half_ext.min_element().max(1e-6);
let capsule_half_height = [
    (abs_a.x > 1e-6).then(|| (half_ext.x - capsule_radius) / abs_a.x),
    (abs_a.y > 1e-6).then(|| (half_ext.y - capsule_radius) / abs_a.y),
    (abs_a.z > 1e-6).then(|| (half_ext.z - capsule_radius) / abs_a.z),
]
.into_iter()
.flatten()
.fold(f32::INFINITY, f32::min)
.max(0.0);
// When h_along <= radius the result is half_height = 0 — the capsule degenerates
// to the inscribed sphere. This is correct (e.g., diagonal axis in a cube).
```

Clip plane sentinel (baked when `clip_pitch` KVP is absent):
```rust
// Proposed design — remove after implementation
let clip_plane = [0.0_f32, 0.0, 0.0, f32::INFINITY];
// dot(pos, (0,0,0)) = 0, which is never > +inf → no clipping.
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
| `clip_plane_d` | `f32` LE | Offset; `f32::INFINITY` when no clip authored |

Previous format emitted no shape field. Loading an old PRL at this offset reads into whatever bytes follow `inv_height_extent` — which is `tag_count`. The level-format crate has no version field, so old PRLs must be recompiled after this change. Existing maps compiled before this change will be invalid; they must be rebuilt with `prl-build`. This is acceptable per the project's pre-release API policy.

`MIN_RECORD_SIZE` in `from_bytes()` must increase from 92 to 132 bytes per record (adds 10 × 4 = 40 bytes to the fixed payload).

## Open questions

- ~~**Capsule radius tie-breaking:**~~ Resolved: use the AABB inscribed-sphere radius `min(hx, hy, hz)` for all axes (cardinal and arbitrary). Generalizes the original "min of two non-axis half-extents" idea — for a cardinal axis along the longest dimension the two reduce to the same value; for arbitrary axes only the inscribed-sphere form has a well-defined meaning.
- ~~**`height_fade` applicability:**~~ Resolved: keep `height_fade` for sphere/ellipsoid; author opts in by setting `height_gradient > 0`.
- ~~**`radial_falloff` for capsule:**~~ Resolved: reuse existing `radial_falloff` field; `dist / capsule_radius` as `radial_t`, consistent with sphere/ellipsoid.
- ~~**Clip plane authoring interface:**~~ Resolved: use `clip_pitch`/`clip_yaw` angle KVPs, consistent with `capsule_pitch`/`capsule_yaw` and the `angles` pattern used by `light_spot`/`light_sun` in this FGD. Authors think in degrees, not world-space vectors.
- ~~**Capsule axis when both KVPs are zero:**~~ Resolved: `Option`-based — if neither key is authored, auto-infer from longest AABB dimension; if either is present (even at value `0`), use the explicit angle.
