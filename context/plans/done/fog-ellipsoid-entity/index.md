# Fog Ellipsoid Entity

## Goal

Add `fog_ellipsoid`, a new brush fog entity whose density profile follows an ellipsoid inscribed in the brush's AABB. Fills the gap between `fog_lamp` (uniform-scale sphere) and `fog_volume` (plane-sweep convex brush) for designers who need a large, axis-non-uniform radial fog (jumbotron glow, tall streetlight haze) without authoring a `radial_falloff` KVP on a cube brush.

## Scope

### In scope

- New FGD `@SolidClass fog_ellipsoid` with `density`, `scatter`, `falloff`, `_tags`.
- New compiler resolver in `parse.rs` that builds a `MapFogVolume` with `planes = vec![]` from a brush's AABB.
- Discriminant on the wire that lets the shader pick the ellipsoid path inside the existing zero-plane branch.
- Shader ellipsoid branch using `inv_half_ext` (already baked for every record by `pack.rs::encode_fog_volumes`).
- Rename of `FogVolumeComponent.edge_softness` is **not** part of this plan; ellipsoid ignores `edge_softness`.

### Out of scope

- Removing or renaming `fog_lamp`, `fog_tube`, `fog_volume`. They keep their current FGD names, defaults, and shader paths.
- Per-entity fog color (settled: ambient comes from the SH irradiance volume).
- Non-convex brush support.
- Migrating `fog_lamp` to a degenerate `fog_ellipsoid`. Noted as future consolidation; not done here.
- Authoring-time editor model for the `@SolidClass` (TrenchBroom uses the brush itself).

## Acceptance criteria

- [ ] A `.map` containing one `fog_ellipsoid` brush compiles to a PRL whose `FogVolumesSection` carries one `FogVolumeRecord` with `plane_count == 0`, `inv_half_ext` matching the brush AABB (`inv_half_ext[i] = 1.0 / ((aabb.max[i] - aabb.min[i]) * 0.5)`), and the new shape discriminant indicating ellipsoid.
- [ ] At runtime the brush produces fog density that peaks at the brush center and reaches ~zero at the brush face centers (the inscribed ellipsoid surface), regardless of axis aspect ratio. Density at the AABB corners is visibly lower than at face centers.
- [ ] An axis-non-uniform brush (e.g. 4×1×8 m) produces a flattened/elongated fog falloff that no `fog_lamp` placement can reproduce.
- [ ] `fog_lamp` and `fog_tube` placed in the same map continue to render with their existing radial-falloff semantics — no behavior change.
- [ ] `fog_volume` (plane-sweep) placed in the same map continues to render with its existing `edge_softness` semantics — no behavior change.
- [ ] FogCellMasks (PRL section 31) bit assignments include the ellipsoid volume; portal-driven fog culling skips it when no visible cell touches its AABB.
- [ ] Authoring `density`, `scatter`, `falloff` KVPs on the entity round-trips through the compiler: the emitted `FogVolumeRecord` carries the correct `density`, `scatter`, and `radial_falloff` wire values. Script observability via `world.query` requires the companion spec `fog-volume-reactions`.
- [ ] Compiler rejects a brush whose AABB has zero extent on any axis with an actionable error.
- [ ] PRL emitted before this change must be recompiled; the byte layout is identical so existing validators will not reject old files — old PRLs silently render fog incorrectly (old `inv_height_extent` values misread as `shape_mode`). We do not add a backward-compat path (pre-release mantra); user-visible breakage is: recompile required.

## Tasks

### Task 1: Repurpose `inv_height_extent` as `shape_mode` discriminant

`inv_height_extent: f32` is reserved/dead in the GPU `FogVolume` struct, the WGSL `FogVolume` struct, and the wire `FogVolumeRecord`. Rename it to `shape_mode: f32` end-to-end and redefine its semantics: `0.0` = legacy radial (sphere/capsule fade against `half_diag`), `1.0` = ellipsoid (normalized fade against `inv_half_ext`). Layout is unchanged — same 80-byte GPU struct, same wire format byte offsets — only the field name and meaning move. `pack.rs::encode_fog_volumes` writes `0.0` for every existing producer (`fog_volume`, `fog_lamp`, `fog_tube`) and the new ellipsoid path writes `1.0`. The `FogVolumeBridge::FogVolumeAabb` cache field is renamed to match. The format crate's `read_f32` non-finite guard already covers validity.

`MapFogVolume` gains `is_ellipsoid: bool`; `pack.rs::encode_fog_volumes` converts it to `shape_mode: 0.0`/`1.0` at write time. All existing producers (`fog_volume`, `fog_lamp`, `fog_tube`) set `is_ellipsoid: false`; `resolve_fog_ellipsoid` sets it `true`. The three existing call sites in `parse.rs` (`resolve_fog_volume`, `resolve_fog_lamp`, `resolve_fog_tube`) each gain an `is_ellipsoid: false` field in their `MapFogVolume` struct literal.

`f32` discriminant rather than `u32` because rotating the field's type would change the wire layout's interpretation of those four bytes; keeping it `f32` means old test fixtures and round-trip tests need only a field rename — no type annotation changes. Comparison in WGSL is `shape_mode > 0.5` to avoid float equality.

### Task 2: Add `fog_ellipsoid` to the FGD

Add `@SolidClass = fog_ellipsoid : "Ellipsoidal volumetric fog (brush)"` to `sdk/TrenchBroom/postretro.fgd` with KVPs `density(float) : 0.5`, `scatter(float) : 0.6`, `falloff(float) : 2.0`, `_tags`, using the same FGD type-spelling as `fog_volume`'s matching fields. No `model()` declaration — `@SolidClass` brushes are their own visual in TrenchBroom; the brush hull is the gizmo. Write a comment block above the entity matching the pattern used for `fog_volume` explaining when an author chooses ellipsoid vs. plane-bounded vs. point-entity sphere/capsule.

### Task 3: Compiler resolver `resolve_fog_ellipsoid`

In `crates/level-compiler/src/parse.rs`, mirror `resolve_fog_volume`'s vertex-walk to derive an AABB from the entity's brushes — but skip the face-plane collection entirely. Result is a `MapFogVolume` with `planes: vec![]`, `edge_softness: 0.0`, and `radial_falloff` populated from the `falloff` KVP. Reject zero-extent brushes with an actionable error matching the style of the radius/height validators on `fog_lamp` / `fog_tube`. The unioned AABB is what is validated; individual brush degeneracy within the union is not checked.

Wire the new classname into the brush-entity dispatch arm in `parse.rs` alongside `fog_volume`. The two share the `MAX_FOG_VOLUMES` cap; `fog_ellipsoid` joins the same overflow-warning arm as `fog_volume` — `fog_lamp`/`fog_tube` overflow handling is left as-is. Multi-brush ellipsoid entities are accepted (one entity, one AABB unioned over all brushes) — center is the unioned-AABB midpoint; the entity `origin` KVP is ignored for the fog record. The multi-brush rejection on `fog_volume` exists because plane intersection of multiple brushes silently changes shape; an AABB-only resolver has no such hazard. One `fog_ellipsoid` entity produces exactly one `FogVolumeRecord` with the unioned AABB, consuming one `MAX_FOG_VOLUMES` slot and one section-31 bit. No changes required in `fog_cell_masks.rs` or the per-leaf bitmask builder — the existing per-volume-index bit assignment handles `fog_ellipsoid` automatically once it lands in the `MapFogVolume` list. The `falloff` KVP defaults to `2.0` when absent, matching the FGD default.

### Task 4: Shader branch in `sample_fog_volumes`

In `crates/postretro/src/shaders/fog_volume.wgsl`, inside the existing `plane_count == 0` else branch, add a sub-branch on `shape_mode`. The legacy radial path stays unchanged for `shape_mode <= 0.5`. The new ellipsoid path computes:

```wgsl
let rel = pos - v.center;
let ellipsoid_t = clamp(length(rel * v.inv_half_ext), 0.0, 1.0);
let radial_inv = 1.0 - ellipsoid_t;
fade = select(pow(max(radial_inv, 1.0e-6), max(v.radial_falloff, 1.0e-6)), 1.0, v.radial_falloff <= 0.0);
```

Activate `inv_half_ext` (currently dead). The `radial_falloff <= 0.0 → 1.0` guard mirrors the identical guard already present in the legacy radial path above. The AABB-gate prologue at the top of the loop is unchanged — corner regions inside the box but outside the ellipsoid evaluate the math but reach near-zero density quickly via `pow`.

## Sequencing

**Phase 1 (sequential):** Task 1 — renames a field across Rust GPU struct, WGSL struct, wire format, bridge, and tests. Every later task depends on the new name and discriminant semantics.

**Phase 2 (concurrent):** Task 2 (FGD), Task 3 (compiler resolver), Task 4 (shader branch). Independent files; no shared edits beyond the renamed `shape_mode` field already landed in Phase 1.

## Boundary inventory

| Name | Rust | Wire / serde | WGSL | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|---|
| Ellipsoid entity classname | n/a | n/a | n/a | n/a | n/a | `fog_ellipsoid` |
| Falloff exponent | `MapFogVolume.radial_falloff`, `FogVolumeRecord.radial_falloff`, `FogVolume.radial_falloff` | `radial_falloff: f32` (existing wire field, no rename) | `radial_falloff: f32` | n/a (see `fog-volume-reactions`) | n/a (see `fog-volume-reactions`) | `falloff` (on `fog_ellipsoid`); `radial_falloff` (existing on `fog_lamp` / `fog_tube`, not renamed) |
| Shape discriminant | `MapFogVolume.is_ellipsoid: bool` (compiler-internal; `pack.rs` converts to wire `shape_mode`), `FogVolume.shape_mode: f32`, `FogVolumeRecord.shape_mode: f32`, `FogVolumeAabb.shape_mode: f32` | `shape_mode: f32` (was `inv_height_extent`; same 4-byte slot) | `shape_mode: f32` | n/a (not exposed to script — baked geometry) | n/a | n/a |
| Inverse half-extent (already exists) | `FogVolume.inv_half_ext: [f32;3]` | `inv_half_ext: [f32;3]` | `inv_half_ext: vec3<f32>` | n/a | n/a | n/a |

**KVP asymmetry note.** The FGD KVP on `fog_ellipsoid` is `falloff`; the wire field it populates is `radial_falloff`. The shorter name matches the script-facing `FogVolumeComponent.falloff` field defined in `fog-volume-reactions`. No rename of the wire field is planned.

## Wire format

`FogVolumesSection` (PRL section 30) byte layout is unchanged in shape. The `inv_height_extent: f32` slot at its existing offset is reinterpreted as `shape_mode: f32`:

- `0.0` = legacy semantic radial (`fog_lamp` sphere fade, `fog_tube` capsule fade — uses `half_diag`)
- `1.0` = ellipsoid (uses `inv_half_ext`)

No new fields. No reordering. No new section. Older PRLs that wrote `1 / max(max.y - min.y, 1e-6)` into the slot will be misinterpreted by the new shader as ellipsoid mode; this is acceptable per the pre-release mantra (recompile required), and the existing engine startup already validates section IDs but does not version-check section payloads. Compiler bumps no version number; user-visible breakage is "old `.prl` files render fog incorrectly until rebuilt."

`pack.rs::encode_fog_volumes`:
- `fog_volume`, `fog_lamp`, `fog_tube` resolvers → `shape_mode = 0.0`.
- `fog_ellipsoid` resolver → `shape_mode = 1.0`.

The format crate's `read_f32` non-finite guard catches NaN/Inf corruption in this slot; it does not protect against finite legacy values being misread as a valid `shape_mode`.

## Rough sketch

Implementation pivot points in source:

- `crates/level-format/src/fog_volumes.rs` — rename `inv_height_extent` → `shape_mode` on `FogVolumeRecord`; update `to_bytes` / `from_bytes` field name; tests updated.
- `crates/postretro/src/fx/fog_volume.rs` — rename on `FogVolume` GPU struct; update offset comments and the `pack_fog_volumes_round_trips_all_baked_fields` test that spot-checks byte offsets — rename `inv_height_extent` literal and the `inv_height_extent` field literal at line 239 and the comment-only token `inv_height_extent(60)` at line 228 (no local binding named `inv_h_ext` exists; the byte-range slice for offset 60 is not currently asserted by a named variable).
- `crates/postretro/src/shaders/fog_volume.wgsl` — rename WGSL field; add ellipsoid sub-branch in `sample_fog_volumes` zero-plane else branch.
- `crates/level-compiler/src/pack.rs` — write `shape_mode = 0.0` from existing producers; populate from a new `MapFogVolume` field for ellipsoid.
- `crates/level-compiler/src/map_data.rs` — add `is_ellipsoid: bool` to `MapFogVolume`; `pack.rs` converts to `shape_mode: f32` at write time.
- `crates/level-compiler/src/parse.rs` — add `resolve_fog_ellipsoid`; extend brush-entity dispatch arm.
- `crates/postretro/src/scripting/systems/fog_volume_bridge.rs` — rename `inv_height_extent` → `shape_mode` on the `FogVolumeAabb` cache.
  - Update the `FogVolumeAabb` doc-comment to describe `shape_mode` as a discriminant flag rather than a baked geometric metric. The rename is internal-only; `shape_mode` is not exposed via `world.query` in this plan.
- `sdk/TrenchBroom/postretro.fgd` — `fog_ellipsoid` declaration added near the other fog entities.

## Companion spec

Runtime mutation of fog volume parameters (`density`, `scatter`, `edge_softness`, `falloff`) for all fog entity types is handled in the companion spec `fog-volume-reactions`. That spec replaces the previously-planned `setComponent` path with named reaction primitives so the scripting VM is not live at runtime.

## Open questions

None. Four open questions in the brief resolved as:

1. **Discriminant** — repurpose `inv_height_extent` as `shape_mode: f32`. Rationale: zero new bytes, no padding penalty, no new field on a struct that already has a comment-tracked reserved slot. Pre-release mantra absorbs the wire-format reinterpretation cost; older PRLs must be recompiled.
2. **inv_half_ext population** — already populated for every fog record by `pack.rs::encode_fog_volumes`. No new compute path; the ellipsoid branch reads what is already there. The `shape_mode` discriminant guarantees the legacy radial path doesn't change behavior even though `inv_half_ext` is non-zero on its records.
3. **fog_lamp future** — left in place. Note for a future plan: once ellipsoid ships, `fog_lamp` is a degenerate ellipsoid (uniform half-extent). Consolidation would simplify the shader by removing the legacy `half_diag` path; out of scope here.
4. **TrenchBroom @SolidClass model** — confirmed: the brush is the gizmo. No `model()` declaration on `fog_ellipsoid`. No editor `.obj` asset.
