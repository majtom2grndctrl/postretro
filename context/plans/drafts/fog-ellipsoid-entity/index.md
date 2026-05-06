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
- Add `falloff` to `FogVolumeComponent` so the radial exponent is runtime-settable for all fog volume types.

### Out of scope

- Removing or renaming `fog_lamp`, `fog_tube`, `fog_volume`. They keep their current FGD names, defaults, and shader paths.
- Per-entity fog color (settled: ambient comes from the SH irradiance volume).
- Non-convex brush support.
- Migrating `fog_lamp` to a degenerate `fog_ellipsoid`. Noted as future consolidation; not done here.
- Authoring-time editor model for the `@SolidClass` (TrenchBroom uses the brush itself).

## Acceptance criteria

- [ ] A `.map` containing one `fog_ellipsoid` brush compiles to a PRL whose `FogVolumesSection` carries one `FogVolumeRecord` with `plane_count == 0`, `inv_half_ext` matching the brush AABB, and the new shape discriminant indicating ellipsoid.
- [ ] At runtime the brush produces fog density that peaks at the brush center and reaches ~zero at the brush face centers (the inscribed ellipsoid surface), regardless of axis aspect ratio. Density at the AABB corners is visibly lower than at face centers.
- [ ] An axis-non-uniform brush (e.g. 4×1×8 m) produces a flattened/elongated fog falloff that no `fog_lamp` placement can reproduce.
- [ ] `fog_lamp` and `fog_tube` placed in the same map continue to render with their existing radial-falloff semantics — no behavior change.
- [ ] `fog_volume` (plane-sweep) placed in the same map continues to render with its existing `edge_softness` semantics — no behavior change.
- [ ] FogCellMasks (PRL section 31) bit assignments include the ellipsoid volume; portal-driven fog culling skips it when no visible cell touches its AABB.
- [ ] Authoring `density`, `scatter`, `falloff` KVPs on the entity round-trips through the compiler and is observable via `world.query({ component: "fog_volume" })` from script.
- [ ] Setting `component.falloff` via `setComponent` on a `fog_ellipsoid` entity changes the visible falloff shape on the next frame.
- [ ] Setting `component.falloff` on a `fog_lamp` or `fog_volume` entity is accepted (no error); for `fog_volume` (plane path) it has no visible effect; for `fog_lamp` it changes the radial exponent.
- [ ] Compiler rejects a brush whose AABB has zero extent on any axis with an actionable error.
- [ ] PRL emitted by an older engine without ellipsoid awareness must be recompiled; mismatched record layout fails the existing format validators (we do not add a backward-compat path — pre-release mantra).

## Tasks

### Task 1: Repurpose `inv_height_extent` as `shape_mode` discriminant

`inv_height_extent: f32` is reserved/dead in the GPU `FogVolume` struct, the WGSL `FogVolume` struct, and the wire `FogVolumeRecord`. Rename it to `shape_mode: f32` end-to-end and redefine its semantics: `0.0` = legacy radial (sphere/capsule fade against `half_diag`), `1.0` = ellipsoid (normalized fade against `inv_half_ext`). Layout is unchanged — same 80-byte GPU struct, same wire format byte offsets — only the field name and meaning move. `pack.rs::encode_fog_volumes` writes `0.0` for every existing producer (`fog_volume`, `fog_lamp`, `fog_tube`) and the new ellipsoid path writes `1.0`. The `FogVolumeBridge::FogVolumeAabb` cache field is renamed to match. The format crate's `read_f32` non-finite guard already covers validity.

`f32` discriminant rather than `u32` because rotating the field's type would change the wire layout's interpretation of those four bytes; keeping it `f32` lets old test fixtures and round-trip tests continue to compile after only renaming the field. Comparison in WGSL is `shape_mode > 0.5` to avoid float equality.

### Task 2: Add `fog_ellipsoid` to the FGD

Add `@SolidClass = fog_ellipsoid : "Ellipsoidal volumetric fog (brush)"` to `sdk/TrenchBroom/postretro.fgd` with KVPs `density (default 0.5)`, `scatter (default 0.6)`, `falloff (default 2.0)`, `_tags`. No `model()` declaration — `@SolidClass` brushes are their own visual in TrenchBroom; the brush hull is the gizmo. Write a comment block above the entity matching the pattern used for `fog_volume` explaining when an author chooses ellipsoid vs. plane-bounded vs. point-entity sphere/capsule.

### Task 3: Compiler resolver `resolve_fog_ellipsoid`

In `crates/level-compiler/src/parse.rs`, mirror `resolve_fog_volume`'s vertex-walk to derive an AABB from the entity's brushes — but skip the face-plane collection entirely. Result is a `MapFogVolume` with `planes: vec![]`, `edge_softness: 0.0`, and `radial_falloff` populated from the `falloff` KVP. Reject zero-extent brushes with an actionable error matching the style of the radius/height validators on `fog_lamp` / `fog_tube`.

Wire the new classname into the brush-entity dispatch arm in `parse.rs` alongside `fog_volume`. The two share the `MAX_FOG_VOLUMES` cap; route both through the same overflow warning. Multi-brush ellipsoid entities are accepted (one entity, one AABB unioned over all brushes) — the multi-brush rejection on `fog_volume` exists because plane intersection of multiple brushes silently changes shape; an AABB-only resolver has no such hazard.

### Task 4: Shader branch in `sample_fog_volumes`

In `crates/postretro/src/shaders/fog_volume.wgsl`, inside the existing `plane_count == 0` else branch, add a sub-branch on `shape_mode`. The legacy radial path stays unchanged for `shape_mode <= 0.5`. The new ellipsoid path computes:

```wgsl
let rel = pos - v.center;
let ellipsoid_t = clamp(length(rel * v.inv_half_ext), 0.0, 1.0);
let radial_inv = 1.0 - ellipsoid_t;
fade = select(pow(max(radial_inv, 1.0e-6), max(v.radial_falloff, 1.0e-6)), 1.0, v.radial_falloff <= 0.0);
```

Activate `inv_half_ext` (currently dead). The AABB-gate prologue at the top of the loop is unchanged — corner regions inside the box but outside the ellipsoid evaluate the math but reach near-zero density quickly via `pow`.

### Task 5: Promote `falloff` (radial_falloff) to a runtime-mutable component field

Extend `FogVolumeComponent` in `crates/postretro/src/scripting/registry.rs` with `falloff: f32`. Carry it through:

- `populate_from_level`: copy `entry.radial_falloff` into the new component field at level load.
- `update_volumes`: copy `component.falloff` into `FogVolume.radial_falloff` instead of `aabb.radial_falloff` when the component is present. The bridge's `FogVolumeAabb.radial_falloff` field is removed in favor of the component carrying the value.
- `conv.rs` `FromJs` and `FromLua` for `ComponentValue::FogVolume`: parse `falloff` alongside `density`, `scatter`, `edge_softness`. Default to 0.0 if missing on the script-side input.
- `typedef.rs` hand-written `FogVolumeComponent` TypeScript and Luau types: add `falloff: number`. Regenerate `sdk/types/postretro.d.{ts,luau}` via `cargo run -p postretro --bin gen-script-types` (debug runtime auto-emits these as well).
- `docs/scripting-reference.md` `FogVolumeComponent` row: add `falloff` to the field list.

`falloff` is accepted on every fog volume type. For `fog_volume` plane-sweep volumes it is stored but not consulted by the shader (the plane path doesn't read `radial_falloff`). For `fog_lamp` and `fog_tube` it drives the existing semantic radial path. For `fog_ellipsoid` it drives the new ellipsoid path. No per-classname enforcement at the script boundary — accept the field everywhere, document which paths consume it.

## Sequencing

**Phase 1 (sequential):** Task 1 — renames a field across Rust GPU struct, WGSL struct, wire format, bridge, and tests. Every later task depends on the new name and discriminant semantics.

**Phase 2 (concurrent):** Task 2 (FGD), Task 3 (compiler resolver), Task 4 (shader branch), Task 5 (component field). Independent files; no shared edits beyond the renamed `shape_mode` field already landed in Phase 1.

## Boundary inventory

| Name | Rust | Wire / serde | WGSL | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|---|
| Ellipsoid entity classname | n/a | n/a | n/a | n/a | n/a | `fog_ellipsoid` |
| Falloff exponent | `FogVolumeComponent.falloff`, `MapFogVolume.radial_falloff`, `FogVolumeRecord.radial_falloff`, `FogVolume.radial_falloff` | `radial_falloff: f32` (existing wire field, no rename) | `radial_falloff: f32` | `FogVolumeComponent.falloff: number` | `FogVolumeComponent.falloff: number` | `falloff` (on `fog_ellipsoid`); `radial_falloff` (existing on `fog_lamp` / `fog_tube`, not renamed) |
| Shape discriminant | `FogVolume.shape_mode: f32`, `FogVolumeRecord.shape_mode: f32`, `FogVolumeAabb.shape_mode: f32` | `shape_mode: f32` (was `inv_height_extent`; same 4-byte slot) | `shape_mode: f32` | n/a (not exposed to script — baked geometry) | n/a | n/a |
| Inverse half-extent (already exists) | `FogVolume.inv_half_ext: [f32;3]` | `inv_half_ext: [f32;3]` | `inv_half_ext: vec3<f32>` | n/a | n/a | n/a |

**Asymmetry note.** The component-side script field is `falloff` (matches the new `fog_ellipsoid` KVP and is the cleaner script-facing name now that the radial contract is implicit on the entity). The wire and Rust-internal field stays `radial_falloff` to avoid a much larger rename across `MapFogVolume`, `FogVolumeRecord`, the GPU struct, the WGSL struct, and the existing PRL wire format. The bridge maps `FogVolumeComponent.falloff` ↔ `FogVolume.radial_falloff` at the existing copy site.

## Wire format

`FogVolumesSection` (PRL section 30) byte layout is unchanged in shape. The `inv_height_extent: f32` slot at its existing offset is reinterpreted as `shape_mode: f32`:

- `0.0` = legacy semantic radial (`fog_lamp` sphere fade, `fog_tube` capsule fade — uses `half_diag`)
- `1.0` = ellipsoid (uses `inv_half_ext`)

No new fields. No reordering. No new section. Older PRLs that wrote `1 / max(max.y - min.y, 1e-6)` into the slot will be misinterpreted by the new shader as ellipsoid mode; this is acceptable per the pre-release mantra (recompile required), and the existing engine startup already validates section IDs but does not version-check section payloads. Compiler bumps no version number; user-visible breakage is "old `.prl` files render fog incorrectly until rebuilt."

`pack.rs::encode_fog_volumes`:
- `fog_volume`, `fog_lamp`, `fog_tube` resolvers → `shape_mode = 0.0`.
- `fog_ellipsoid` resolver → `shape_mode = 1.0`.

The format crate's `read_f32` non-finite guard already protects against corrupt floats in this slot.

## Rough sketch

Implementation pivot points in source:

- `crates/level-format/src/fog_volumes.rs` — rename `inv_height_extent` → `shape_mode` on `FogVolumeRecord`; update `to_bytes` / `from_bytes` field name; tests updated.
- `crates/postretro/src/fx/fog_volume.rs` — rename on `FogVolume` GPU struct; update offset comments and the round-trip test that spot-checks byte offsets.
- `crates/postretro/src/shaders/fog_volume.wgsl` — rename WGSL field; add ellipsoid sub-branch in `sample_fog_volumes` zero-plane else branch.
- `crates/level-compiler/src/pack.rs` — write `shape_mode = 0.0` from existing producers; populate from a new `MapFogVolume` field for ellipsoid.
- `crates/level-compiler/src/map_data.rs` — add `shape_mode: f32` to `MapFogVolume` (or a `bool ellipsoid` if cleaner; resolve at pack time).
- `crates/level-compiler/src/parse.rs` — add `resolve_fog_ellipsoid`; extend brush-entity dispatch arm.
- `crates/postretro/src/scripting/systems/fog_volume_bridge.rs` — drop `radial_falloff` from `FogVolumeAabb`; pull from component instead. Rename `inv_height_extent` → `shape_mode` on the cache.
- `crates/postretro/src/scripting/registry.rs` — `FogVolumeComponent` gains `falloff: f32`.
- `crates/postretro/src/scripting/conv.rs` — TS / Luau parsing accepts `falloff`.
- `crates/postretro/src/scripting/typedef.rs` — hand-written types include `falloff`.
- `sdk/types/postretro.d.{ts,luau}` — regenerated from the typedef generator.
- `docs/scripting-reference.md` — `FogVolumeComponent` row mentions `falloff`.
- `sdk/TrenchBroom/postretro.fgd` — `fog_ellipsoid` declaration added near the other fog entities.

## Open questions

None. Five open questions in the brief resolved as:

1. **Discriminant** — repurpose `inv_height_extent` as `shape_mode: f32`. Rationale: zero new bytes, no padding penalty, no new field on a struct that already has a comment-tracked reserved slot. Pre-release mantra absorbs the wire-format reinterpretation cost; older PRLs must be recompiled.
2. **inv_half_ext population** — already populated for every fog record by `pack.rs::encode_fog_volumes`. No new compute path; the ellipsoid branch reads what is already there. The `shape_mode` discriminant guarantees the legacy radial path doesn't change behavior even though `inv_half_ext` is non-zero on its records.
3. **fog_lamp future** — left in place. Note for a future plan: once ellipsoid ships, `fog_lamp` is a degenerate ellipsoid (uniform half-extent). Consolidation would simplify the shader by removing the legacy `half_diag` path; out of scope here.
4. **TrenchBroom @SolidClass model** — confirmed: the brush is the gizmo. No `model()` declaration on `fog_ellipsoid`. No editor `.obj` asset.
5. **`falloff` runtime mutability** — yes, exposed on `FogVolumeComponent` for all fog volume types. Plane-sweep `fog_volume` accepts it but its shader path ignores it; documented.
