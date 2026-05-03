# Fog Volume Shapes

## Goal

Replace the single AABB-shaped `env_fog_volume` brush entity with a two-tier authoring model:

- **Primitive layer** — a `fog_volume` brush entity. The mapper draws any convex brush; the compiler extracts every face plane and bakes them into the PRL fog record. The shader does N plane-side tests at each raymarch sample. Wedges, slashes, ramp-following prisms, and angled cuts come from brush geometry directly.
- **Semantic layer** — named point entities (`fog_lamp`, `fog_tube`, …) that ship with unit-geometry display models scaled live by their sizing KVPs. Each carries fog parameters tuned for its use case. Semantic entities compile to standard fog records with zero planes — pure AABB membership at runtime.

The brush IS the fog shape. Semantic point entities are authoring sugar over the same record format, same GPU struct, same shader path.

## Scope

### In scope

- `fog_volume` brush entity. Any convex brush shape valid. Mapper draws geometry; the compiler bakes face planes.
- Two reference semantic point entities: `fog_lamp` (sphere-shaped halo) and `fog_tube` (oriented capsule-shaped strip glow).
- Each semantic entity ships a unit-geometry TrenchBroom display model resized live via `model()` `scale` expression driven by sizing KVPs.
- FGD comment block documenting the pattern so modders can author additional semantic fog entities (point entity + sizing KVPs + `model()` scale + tuned defaults).
- `FogVolumeRecord` gains `plane_count: u32` and a per-plane `Vec<[f32; 4]>` payload (length-prefixed, same variable-length pattern as the existing tag list). AABB fields retained as a conservative bound.
- A flat `fog_planes` GPU storage buffer holds every volume's planes packed contiguously. Each `FogVolume` carries `plane_offset: u32` and `plane_count: u32` into that buffer. Same offset/count indirection pattern used by chunk light lists, animated lightmap composer, curve eval, and SH composer.
- WGSL `sample_fog_volumes()` replaces per-component AABB membership with a plane-sweep loop: a sample is inside iff `dot(pos, plane.xyz) <= plane.w` for every plane in the volume's range.
- Single `edge_softness` KVP on `fog_volume` replaces `box_fade` / `height_gradient` / `radial_falloff`. Computed per sample as the minimum signed distance to any face, normalized into `[0, 1]`, used to scale density.
- `radial_falloff` survives in the GPU struct for semantic entities. Both `fog_lamp` and `fog_tube` use AABB slab-clip + spherical `radial_falloff` fade. Capsule appearance for `fog_tube` comes from AABB proportions only — no capsule-specific test at runtime.
- Slab-clip prologue (AABB ray-interval clamp) unchanged — AABB remains the conservative bound for the raymarch.
- Round-trip serialization tests in `crates/level-format` for: a 6-plane box brush, a 5-plane wedge, a zero-plane volume (semantic entity case), and a volume with planes plus tags (variable-length coexistence).

### Non-goals

- Per-shape membership branches (sphere / ellipsoid / capsule discriminants) in the shader. Brush plane-sweep covers the primitive case; AABB membership covers the semantic case.
- A `shape` KVP on `fog_volume`. The brush carries the shape.
- A `clip` KVP. Angled cuts are authored as brush face planes natively.
- `height_gradient`, `box_fade`, and `falloff` on the primitive entity.
- Room/portal-conforming fog generated from cell geometry. Separate future spec.
- Additional reference semantic entities beyond `fog_lamp` and `fog_tube` at initial ship. Modders extend via the documented pattern.
- Backward compatibility with the prior `env_fog_volume` brush entity. Maps re-author against the new entity names. Pre-release API policy.

## Acceptance criteria

### Primitive entity (`fog_volume`)

- [ ] A `fog_volume` brush authored as a 6-face axis-aligned box renders fog inside the box and nothing outside, identical in coverage to the previous AABB behavior.
- [ ] A `fog_volume` brush authored as a 5-face wedge (right-triangular prism) renders fog inside the wedge and nothing in the cut-away half-space — verified by sampling a point that lies inside the brush AABB but outside the brush itself.
- [ ] A `fog_volume` brush authored as a slash-shaped hexahedron (axis-aligned box with two opposing faces tilted) renders fog conforming to the slashed cross-section.
- [ ] A `fog_volume` with `edge_softness "0"` (or any non-positive value) produces a hard cutoff — full density inside, zero outside, no fade band.
- [ ] A `fog_volume` with `edge_softness "1"` produces a fade band of 1 world unit — density reaches 1.0 at any point ≥ 1 unit from the nearest face and tapers to 0 at the face boundary.
- [ ] `fog_volume` carries `density`, `color`, `scatter`, `edge_softness`, `_tags` KVPs and no others. `shape`, `clip*`, `height_gradient`, `radial_falloff`, `box_fade`, `falloff` are absent from the primitive entity FGD definition.
- [ ] `prl-build` rejects a `fog_volume` whose brush has zero faces (degenerate) or more than 16 faces (over budget) with a descriptive error and non-zero exit code.

### Semantic entities

- [ ] A `fog_lamp` with `radius "128"` renders a spherical halo of radius 128 centered on the entity origin. The TrenchBroom display model is a unit sphere scaled to match.
- [ ] Editing the `radius` KVP of a `fog_lamp` in TrenchBroom resizes the displayed model live in the viewport.
- [ ] A `fog_tube` with `radius "32"`, `height "256"`, default orientation renders a capsule-shaped glow of total height 256 and radius 32, oriented on +Y. The TrenchBroom display model is a unit capsule scaled to `(32, 256, 32)`.
- [ ] A `fog_tube` with explicit `pitch` and `yaw` KVPs reorients both the runtime fog volume and the TrenchBroom display model.
- [ ] `fog_lamp` and `fog_tube` compile to fog records with `plane_count == 0`. The shader runs the AABB-only path for them.
- [ ] A `fog_lamp` placed in a cell not reachable from the camera is absent from the active volume set and contributes no samples.
- [ ] The FGD includes a comment block documenting how to define additional semantic fog entities.

### Wire format and round-trip

- [ ] `FogVolumesSection::from_bytes(section.to_bytes())` round-trips for: a 6-plane box-brush record, a 5-plane wedge record, a zero-plane semantic record, and a record carrying both planes and tags.
- [ ] `MIN_RECORD_SIZE` is updated from 92 to 96 to account for the added `plane_count: u32` field.
- [ ] `FOG_VOLUME_SIZE` remains 96. `_pad: [f32; 2]` is replaced by `plane_offset: u32, plane_count: u32` (same 8 bytes). The size assert does not change.

## Tasks

### Task 1: Level format — replace shape fields with plane payload

In `crates/level-format/src/fog_volumes.rs`:

- `FogVolumeRecord` currently has no shape, capsule, or clip-plane fields. No removal needed.
- Add `plane_count: u32` and `planes: Vec<[f32; 4]>` (each plane stored as `(nx, ny, nz, d)` in engine coordinates, with the convention that a point is inside the volume when `dot(pos, n) <= d` for every plane).
- Serialize the planes after the existing fixed payload and before the tag list, using the same length-prefixed variable-length pattern the tag list already uses. The fixed payload carries `plane_count`; the variable section that follows carries `plane_count * 16` bytes of plane data.
- Update `MIN_RECORD_SIZE` to reflect the new fixed-payload size (add 4 bytes for `plane_count`; current value is 92).
- Add round-trip tests for: 6-plane box, 5-plane wedge, zero-plane volume, volume with planes and tags coexisting. Each test asserts every plane component round-trips bit-for-bit.

### Task 2: FGD — primitive brush entity and semantic point entities

In `sdk/TrenchBroom/postretro.fgd`:

- Replace the existing `@SolidClass env_fog_volume` definition with a `@SolidClass fog_volume` carrying only: `density`, `color`, `scatter`, `edge_softness`, `_tags`.
- Add `@PointClass fog_lamp` with sizing KVP `radius` and a `model()` declaration scaling a unit sphere asset by `radius`. Default fog parameters: `density "0.5"`, `color "1.0 0.85 0.6"` (warm amber), `scatter "0.6"`, `radial_falloff "2.0"`. Expose `density`, `color`, `radius`, `scatter`, `radial_falloff`, `_tags`.
- Add `@PointClass fog_tube` with sizing KVPs `radius`, `height`, optional `pitch` (default `0`) and `yaw` (default `0`). `model()` declaration scales a unit capsule asset (radius 1, total height 1 tip-to-tip, axis +Y) by `(radius, height, radius)` — result is a capsule of total height `height` and radius `radius` — rotated by `(pitch, yaw)` (yaw around +Y first, then pitch around +X). `height` is always tip-to-tip total length. Non-uniform scaling deforms the hemispherical caps into ellipsoids; this is acceptable for authoring feedback — the fog rendering uses AABB proportions, not the display model shape. Default parameters: `density "0.3"`, `color "0.6 0.85 1.0"` (cool blue-white), `scatter "0.6"`, `radial_falloff "1.5"`. Expose `density`, `color`, `radius`, `height`, `pitch`, `yaw`, `scatter`, `radial_falloff`, `_tags`.
- Add a comment block above the semantic entities documenting the modder pattern: define a `@PointClass`, declare sizing KVPs, point a `model()` declaration at a unit-geometry asset and use the `scale` expression to size it, set tuned defaults for `density` / `color` / `radial_falloff`. Reference the `fog_lamp` and `fog_tube` definitions as the worked examples.
- Display-model assets are an implementation choice; the spec requires only that each semantic entity has a unit-geometry asset and a `model()` declaration whose `scale` expression evaluates against the live KVPs.

### Task 3: Level compiler — bake brush planes and semantic entity AABBs

In `crates/level-compiler/src/map_data.rs`:

- `MapFogVolume` has no shape, capsule, or clip-plane fields. Add `planes: Vec<[f32; 4]>` (engine-space, inside-when-`dot(pos, n) <= d`).
- Retain the existing AABB fields (`min`, `max`) — the AABB stays as a conservative bound.

In `crates/level-compiler/src/parse.rs`:

- For `fog_volume` brush entities: `resolve_fog_volume()` calls `brush_hulls()` and `face_planes()` but currently retains only the resulting AABB. Add per-face plane extraction (Nx, Ny, Nz, d form) from the entity's face set and store on `MapFogVolume.planes`. Compute the AABB from the brush hull as today. Face planes from `brush_hulls()` / `face_planes()` use outward-pointing normals — confirmed by `brush_side_winding_aligns_with_side_normal` test in `parse.rs`. Interior points satisfy `dot(pos, n) <= d`, which matches the wire convention directly. Emit planes as `[n.x as f32, n.y as f32, n.z as f32, d as f32]` — no sign inversion needed. Set `radial_falloff = 0.0` for brush entities (the field is always present in the record; brushes use `edge_softness`, not radial fade).
- For `fog_lamp`: read `radius` (required, > 0) and `origin` from `props`; apply `quake_to_engine` and unit scale as in other entity parsing. `min = origin - (r, r, r)`, `max = origin + (r, r, r)`, `planes = vec![]`. Apply defaults when KVP absent: `density = 0.5`, `color = (1.0, 0.85, 0.6)`, `scatter = 0.6`, `radial_falloff = 2.0`.
- For `fog_tube`: read `radius`, `height` (required, > 0), optional `pitch` and `yaw` (degrees; yaw around +Y applied first, then pitch around the resulting +X — intrinsic Y-X). Let `a` be the unit capsule axis in world space derived from `(pitch, yaw)`. World-space AABB half-extents: per component `i`, `half_extent_i = |a_i| * (height/2 - radius) + radius` (AABB of two endpoint spheres at `±(height/2 - radius) * a`). `planes = vec![]`. Apply defaults when KVP absent: `density = 0.3`, `color = (0.6, 0.85, 1.0)`, `scatter = 0.6`, `radial_falloff = 1.5`. The same `(pitch, yaw)` convention drives the `model()` `scale` expression in Task 2 so display and runtime agree.
- Reject missing or non-positive sizing KVPs with a descriptive error naming the entity classname and offending field. Reject a `fog_volume` brush whose hull yields zero face planes or more than 16 face planes.

In `crates/level-compiler/src/pack.rs`:

- `encode_fog_volumes()` writes `plane_count` into the fixed payload, then `plane_count * 16` bytes of plane data into the variable payload. Tag list follows.

**FogCellMasks (PRL section 31):**

- Update the compiler's section-31 bake trigger from `env_fog_volume` to any fog volume entity (`fog_volume`, `fog_lamp`, `fog_tube`). Semantic-entity AABBs participate in per-leaf mask computation identically to brush AABBs (conservative AABB-vs-AABB).
- Section 31 is emitted when at least one fog entity of any kind is present in the map.

### Task 4: CPU and GPU structs — plane indirection

In `crates/postretro/src/fx/fog_volume.rs`:

- Replace `_pad: [f32; 2]` (the only padding in the current struct) with `plane_offset: u32` and `plane_count: u32`. `FOG_VOLUME_SIZE` remains 96 — the size assert does not change.
- `populate_from_level()` (called at level load, in `fog_volume_bridge.rs`) reads `FogVolumeRecord.planes` and stores them on the ECS fog component (`FogVolumeComponent.planes: Vec<[f32; 4]>`) alongside the existing fog parameters. `update_volumes()` (per-frame) iterates the active volume list, accumulates a `plane_offset` cursor, packs each volume's planes from `FogVolumeComponent.planes` into a CPU-side `Vec<[f32; 4]>`, and copies `plane_count`. The renderer uploads this buffer to the `fog_planes` storage buffer each frame.
- `plane_offset` and the `fog_planes` CPU buffer are rebuilt each frame inside `update_volumes()`, iterating the portal-culled dense active set in order. The dense ordering drives the cursor — source PRL index is irrelevant at runtime. The `fog_planes` storage buffer is re-uploaded each frame alongside the `FogVolume` array. Maximum 16 planes per volume; `fog_volume` brushes with more than 16 faces are rejected at compile time with a descriptive error. Total buffer capacity: `MAX_FOG_VOLUMES * 16 = 256` planes — fixed size, no dynamic allocation. Portal-based culling (FogCellMasks, section 31) keeps the active set small; the fixed buffer is sized for the worst case of all 16 volumes active simultaneously.
- Update the `pack_fog_volumes_round_trips_all_baked_fields` test to match the new struct shape.

In `crates/postretro/src/shaders/fog_volume.wgsl`:

- Mirror the Rust struct change: replace `_pad: vec2<f32>` with `plane_offset: u32`, `plane_count: u32`. Field order matches Rust.
- Declare a new storage buffer `fog_planes: array<vec4<f32>>` at `@group(6) @binding(6)` — the next available slot in group 6 (bindings 0–5 are occupied). Update the Rust-side fog bind group layout to match.

### Task 5: Shader — plane-sweep membership and edge-softness fade

Rewrite the inner body of `sample_fog_volumes()`:

- Slab-clip prologue (AABB ray-interval clamp) stays.
- Replace the per-component AABB membership test with a plane sweep. For `i in 0..v.plane_count`, fetch `p = fog_planes[v.plane_offset + i]` and accumulate `min_signed_dist = min(min_signed_dist, p.w - dot(pos, p.xyz))`. Do not early-exit — iterate all planes to compute `min_signed_dist`. A point is inside iff `min_signed_dist >= 0`.
- When `v.plane_count == 0`, the loop is empty and the AABB slab-clip prologue alone bounds the volume — this is the semantic-entity path (`fog_lamp`, `fog_tube`).
- Compute `edge_softness` fade for primitive volumes: when inside (`min_signed_dist >= 0`), `density_scale = saturate(min_signed_dist / edge_softness)`, where `edge_softness` is in world units. When `edge_softness <= 0`, `density_scale = 1.0` (hard cutoff — no division). Skip the sample entirely when `min_signed_dist < 0`.
- For zero-plane volumes (semantic entities), retain the existing spherical radial fade — no behavior change. Both `fog_lamp` and `fog_tube` use this path. Capsule approximation for `fog_tube` comes from AABB proportions; no capsule-specific computation is added. The primitive `edge_softness` path and the semantic `radial_falloff` path are mutually exclusive — `plane_count == 0` selects the semantic branch, `plane_count > 0` selects the primitive branch.

## Open question

**What is the runtime behavior when a `fog_volume` brush yields zero face planes?**

This cannot occur for a well-formed convex brush — a brush has at least 4 faces by definition (tetrahedron lower bound). The compiler rejects degenerate brushes with zero faces during hull construction, so a primitive `fog_volume` reaching the runtime with `plane_count == 0` is impossible.

Resolution: **compiler error**. `prl-build` fails the build if any `fog_volume` brush produces a zero-plane hull. The runtime treats `plane_count == 0` unambiguously as the semantic-entity branch (AABB membership + `radial_falloff` fade). No primitive-entity fallback exists at runtime; the compiler is the authority.

This eliminates the ambiguity at the shader level — `plane_count == 0` is never a degenerate primitive volume; it is always a semantic entity by construction.

## Wire format

`FogVolumeRecord` per-entry layout, in order:

| Field | Type | Notes |
|---|---|---|
| Existing AABB / fog parameter fields | (unchanged) | `min`, `max`, `density`, `color`, `scatter`, `edge_softness`, `radial_falloff`, derived bounds |
| `plane_count` | `u32` LE | Number of planes that follow in the variable payload |
| (variable) `planes` | `plane_count × 4 × f32` LE | Each plane is `(nx, ny, nz, d)`; inside iff `dot(pos, n) <= d` |
| (variable) tag list | length-prefixed string list | Unchanged pattern |

`MIN_RECORD_SIZE` grows by 4 bytes for `plane_count` (92 → 96). The implementor updates the constant. `radial_falloff` is always present in the record; primitive `fog_volume` records write `0.0`, semantic entity records write the KVP value.

The level-format crate has no version field. Existing PRLs must be recompiled; existing `.map` sources must be re-authored against the new entity names. Pre-release API policy.

## GPU struct sketch

```wgsl
// Proposed design — remove after implementation
struct FogVolume {
    min: vec3<f32>, density: f32,
    max_v: vec3<f32>, edge_softness: f32,
    color: vec3<f32>, scatter: f32,
    center: vec3<f32>, half_diag: f32,
    inv_half_ext: vec3<f32>, radial_falloff: f32,
    plane_offset: u32, plane_count: u32,
    // padding falls out of the implementor's chosen layout
}

@group(N) @binding(M) var<storage, read> fog_planes: array<vec4<f32>>;
```

Replacing `_pad: [f32; 2]` (8 bytes) with `plane_offset: u32, plane_count: u32` (8 bytes) keeps `FOG_VOLUME_SIZE` at 96. The compile-time size assert does not change.

## Boundary inventory

| Layer | Primitive (`fog_volume` brush) | Semantic (`fog_lamp`, `fog_tube`, …) |
|---|---|---|
| FGD | `@SolidClass`, KVPs: `density`, `color`, `scatter`, `edge_softness`, `_tags` | `@PointClass`, sizing KVPs + tuned fog defaults, `model()` declaration with `scale` expression |
| Compiler input | Brush hull → face planes → `MapFogVolume.planes` | Entity origin + sizing KVPs → AABB; `planes = vec![]` |
| Wire format | `plane_count > 0`, plane data in variable payload | `plane_count == 0`, no plane data |
| GPU struct | `plane_offset / plane_count` index into `fog_planes` buffer | `plane_count == 0`; AABB-only membership |
| Shader path | Plane-sweep membership + `edge_softness` fade | AABB slab-clip + `radial_falloff` fade |

## Sequencing

**Phase 1 (sequential):** Task 1 — wire format defines the contract every other task consumes.

**Phase 2 (concurrent):** Tasks 2, 3, 4, 5 all consume Task 1's record shape. Task 4 (CPU+GPU struct) and Task 5 (shader) must land atomically — the Rust `FogVolume`, the WGSL `FogVolume` struct, and the `fog_planes` buffer binding must change in the same commit to keep CPU and GPU layouts in sync.
