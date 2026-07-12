# Lightmap Bake Scaling — Coarser Direction Atlas + Per-Surface Density

## Goal

Cut static-lightmap runtime storage (VRAM + the 256-layer `texture_2d_array` budget)
so interior-heavy and larger maps bake and load without exhausting either. Two
independent levers: shrink the **direction** atlas (it is uncompressed `Rgba8Unorm`
and ~80% of production lightmap bytes) by baking it coarser and dropping unused
channels; and give authors a **per-surface density override** so flat/distant
surfaces go coarse (quadratic byte savings) while high-contrast surfaces stay fine.
The irradiance atlas is already BC6H-compressed (~1 B/texel) and is left untouched.

## Scope

### In scope

- Bake the direction atlas at a lower resolution than irradiance, exploiting the
  format's already-decoupled `dir_width`/`dir_height`. Default half-res per axis
  (~4× fewer direction texels), factor selectable, factor 1 = current behavior.
- Drop the two constant/unused direction channels (`Rgba8Unorm` → `Rg8Unorm`) after
  verifying the shader samples only R/G for the static atlas (~2× further). New
  direction-format tag, no section version bump.
- Per-surface density override authored via a brush-entity scale region carrying a
  `_lightmap_scale` KVP; resolved per chart in chart planning, layered over the
  global density.
- Fold `_lightmap_scale` (and the direction-scale factor) into the bake cache key so
  changing either re-bakes rather than serving a stale atlas.

### Out of scope / non-goals

- **The irradiance BC6H path.** No change to irradiance dims, format, or encoder.
- **Compile-time peak RAM / incremental bake-and-flush.** A distinct memory-lifecycle
  contract — sibling plan `lighting-scale--lightmap-bake-incremental-flush`.
- **The storage-buffer / SH-delta footprint problem.** Separate spec
  (`lighting-scale--sh-delta-footprint-instrumentation`); unrelated GPU budget.
- **Block-compressing the direction atlas.** Octahedral lerp ≠ slerp — BC block
  compression corrupts decoded directions (the standing design note). Coarsening
  and channel-drop are the only direction levers here.
- **The animated direction atlas.** It keeps `Rgba8Unorm` — its `.a` is a live
  coverage flag the forward pass reads. Only the static atlas changes.
- **Old-`.prl` migration.** All fixtures re-bake from source.
- **Per-face granularity below a region.** The `.map` format exposes no per-face
  scalar channel; region-brush granularity is the authoring unit (see Task 3).

## Tasks

### Task 1: Coarser direction atlas

Bake the direction atlas at `dir_dims = irr_dims / DIRECTION_TEXEL_SCALE` per axis
(default `2`; `1` reproduces current output). The `LightmapSection` format already
carries independent `dir_width`/`dir_height`/`dir_texel_density`; today the encoder
sets them equal to irradiance. Reduce the composited direction (`Vec3` unit vectors)
and coverage buffers by the factor **inside the direction-encode step** — sum the
unit vectors over each `factor×factor` block and renormalize (a slerp-reasonable
dominant-direction reduction, unlike octahedral lerp), OR coverage across the block —
then octahedral-encode the reduced buffer and write the reduced dims + density. Do
the reduction after the byte-identity composite seam (both the warm per-light
composite and the cold monolithic bake reach the same full-res `CompositedAtlas`,
then encode identically), so the seam is unaffected. The factor must be a power of
two so it divides the pow2 atlas dims cleanly; expose it via a CLI flag defaulting to
`2` and fold it into the lightmap cache key. Per-vertex lightmap UVs are unchanged —
they normalize to `[0,1]` and sample the coarser atlas at the correct texel through
the existing nearest sampler, so no geometry or runtime change is needed beyond the
atlas dimensions the loader already reads from the section header.

### Task 2: Drop unused direction channels (Rg8)

Verify first, then act: the static direction encoder writes `[qx, qy, 128, 255]` —
blue and alpha are constant — and the forward pass decodes the static atlas from
`.r`/`.g` only (recovering z as `1 − |x| − |y|`), never reading `.a`. If that holds
against current source, add a second direction-format tag (`…_OCT_RG8`) and an
`Rg8Unorm` encode path that writes only the two meaningful bytes per texel; the
parser already rejects unknown direction-format tags, so this needs no section
version bump. The runtime direction-texture upload branches on the section's
direction-format tag to create the matching texture format; the BGL sample type
(float, filterable) and the shared octahedral decode are unchanged (an `Rg8Unorm`
sample yields `(r, g, 0, 1)`, and the static path reads only `r`/`g`). Leave the
animated direction atlas as `Rgba8Unorm` — its `.a` is a coverage flag. This lever
stacks on Task 1 (coarser + channel-drop ≈ 8× off the direction half).

### Task 3: Per-surface lightmap density

Add a brush-entity scale region (a `@SolidClass` mirroring the existing region
brush entities) carrying a positive-float `_lightmap_scale` KVP. Parse it to a
world-space AABB + bounding planes exactly as the existing region brush entities
resolve, so world brushes stay world geometry (authors draw a scale box around a
region rather than converting surfaces to entities). In chart planning, resolve each
chart's effective density as `region_scale × global_density` when the chart's origin
falls inside a scale region, else the global density (CLI/worldspawn/default) — so
chart texel dimensions scale with the per-chart density. Precedence: a scale region
overrides the global default for the surfaces it covers; overlapping regions resolve
by a stated rule (e.g. last-defined wins) — pick one and document it. Add the entity
to the FGD with the KVP and its default. Fold the resolved per-chart scale set into
the lightmap cache key so a region edit re-bakes. This changes chart sizing and map
parsing only; it does not touch layer assignment, so the leaf-cohesion invariant
(all of a leaf's charts on one atlas layer) is unaffected — coarser charts simply
pack smaller.

## Sequencing

**Phase 1 (concurrent):** Task 1 (coarser direction — direction-encode path) and
Task 3 (per-surface density — chart planning + parse). Different functions; no
shared edit surface beyond the file.
**Phase 2 (sequential):** Task 2 (Rg8) — consumes Task 1's reshaped direction-encode
path and shares the runtime direction-texture format branch.

**Cross-plan:** This plan should land before `lighting-scale--lightmap-bake-incremental-flush`,
which wraps a per-layer bake-encode-drop loop around this plan's direction-encode
reshaping in the same `lightmap_bake.rs` path; landing this plan first means
incremental-flush's per-layer encode simply calls the already-reshaped direction
encoder instead of chasing a moving target.

## Acceptance criteria

- [ ] At default settings on a lit fixture map, the direction blob byte count in the
  baked `.prl` drops ~4× versus the pre-change bake; the irradiance blob byte count
  is unchanged.
- [ ] With the channel-drop lever active, the direction blob drops a further ~2×
  (≈8× total off the direction atlas versus pre-change).
- [ ] Bumped-Lambert highlight direction on a normal-mapped fixture surface shows no
  visible regression in a before/after A/B; low-frequency direction is preserved.
- [ ] A map with a coarse scale region bakes fewer total lightmap bytes than the same
  map without the region; surfaces outside every region are byte-identical to the
  no-region bake, and surfaces inside a region are coarser (fewer texels).
- [ ] Per-surface precedence is observable: a surface inside a scale region uses
  `region_scale × global_density`; a surface outside every region uses the global
  density unchanged.
- [ ] A `.prl` carrying the new direction-format tag loads and renders direction
  correctly; a `.prl` with an unknown direction-format tag is rejected at load with a
  clear error.
- [ ] Re-baking the same map twice yields byte-identical `.prl` output (determinism
  gate holds), including with a scale region present and with a non-default direction
  scale.
- [ ] The byte-identity gate between the warm per-light composite and the cold
  monolithic bake still passes.
- [ ] A normal (non-verbose) bake gains no new per-item log spam: any new per-surface
  or per-region footprint breakdown appears only under `-v`/`--verbose`, and the
  single-line atlas summary stays one line.

## Rough sketch

Format — `crates/level-format/src/lightmap.rs`: `LightmapSection` already exposes
`dir_width`/`dir_height`/`dir_texel_density`/`dir_format`; `DIRECTION_FORMAT_OCT_RGBA8
= 0` with the parser rejecting unknown tags (the tag exists so new encodings skip a
version bump). `encode_direction_oct` returns `[qx, qy, 128, 255]`.

Bake — `crates/level-compiler/src/lightmap_bake.rs`: `encode_section` sets the dir
dims equal to irr and calls `encode_direction_rgba8(&direction, &coverage)`; the
`CompositedAtlas { direction: Vec<Vec3>, coverage: Vec<bool> }` is the byte-identity
seam (both bake paths reach it, then encode). `plan_charts` computes
`width_texels = ceil(u_extent / density) + padding` — the per-chart density hook for
Task 3. `Chart.leaf_index` and the multi-bin packer keep leaf cohesion; Task 3 does
not touch either. `DEFAULT_TEXEL_DENSITY_METERS = 0.04`.

Density resolve — `crates/level-compiler/src/main.rs::resolve_lightmap_density`
(CLI > worldspawn `_lightmap_density` > default). Region brush entities parse to
world AABB + planes in `crates/level-compiler/src/parse.rs` (the existing
`fog_volume` resolve is the pattern to mirror); `FaceMeta` carries `leaf_index` +
`texture_index`.

Runtime — `crates/renderer/src/lighting/lightmap.rs` (direction-texture upload + BGL
binding 1) branches texture format on the section's direction-format tag;
`crates/renderer/src/shaders/forward.wgsl::decode_lightmap_direction` reads `.r`/`.g`
and the static direction sample never reads `.a`. The animated direction atlas
(forward binding 5) stays `Rgba8Unorm` for its `.a` coverage flag.

Logging discipline: any footprint summary is a one-line `log::info` (like the
per-section `pack.rs` lines and the existing lightmap summary); per-surface /
per-region breakdowns go behind the `--verbose` gate that already wraps
`lightmap_bake::log_stats`. Non-verbose bakes stay quiet.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| Per-surface scale | per-chart effective density in chart planning | n/a (affects chart dims, not a new section field) | `_lightmap_scale` (float, default 1.0) on a scale-region `@SolidClass` |
| Direction format tag | new `DIRECTION_FORMAT_OCT_RG8` const | `dir_format` header u32 (new accepted value; no version bump) | n/a |
| Direction scale factor | `DIRECTION_TEXEL_SCALE` (pow2, default 2) | reflected only in the header's `dir_width`/`dir_height` | optional CLI flag |

## Wire format

No new section and no version bump. Within the existing `LightmapSection` (id 22,
version 2) header: `dir_width`/`dir_height` now carry the reduced direction
dimensions (irr dims / scale), `dir_texel_density` carries the reduced density, and
`dir_format` gains one accepted value for the `Rg8` octahedral layout (direction blob
becomes `dir_width × dir_height × 2` bytes per layer, layer-major, versus `× 4`).
Unknown `dir_format` values keep rejecting as before.
