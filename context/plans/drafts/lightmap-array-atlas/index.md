# Lightmap Array Atlas

## Goal

Replace the single-layer lightmap atlas with `texture_2d_array` atlases for irradiance and
direction, removing the hard 8192² overflow ceiling. Carry the atlas layer index per-vertex —
the only portable channel through the GPU-driven `multi_draw_indexed_indirect` hot path. Replace
the shelf packer with a leaf-aware MaxRects multi-bin packer that keeps all charts of one BVH
leaf on a single layer and achieves 85–95% packing density vs the shelf packer's ~60%.

## Scope

### In scope

- Widen `Vertex` from 32 to 36 bytes: add `lightmap_layer: u16` + 2 bytes explicit padding.
- Restructure `LightmapSection` (PRL section id 22): versioned header, decoupled irradiance and
  direction dimensions, layer-major blob layout. Old sections (version ≠ 2) rejected at parse time.
- Replace `shelf_pack` with a leaf-aware MaxRects multi-bin packer. Hard invariant: all charts for
  one BVH leaf land on a single layer (bake-time assertion in unit tests). Add `MAX_ATLAS_LAYERS`
  constant (= 256). Add `LightmapBakeError::LayerOverflow` for when charts exceed this cap.
- Update `assign_lightmap_uvs` to write `lightmap_layer` per vertex from packer output.
- Runtime: `texture_2d_array` texture creation for irradiance (binding 0) and direction (binding 1);
  `D2Array` view dimension in BGL entries for those two bindings. Animated atlas bindings (3, 5)
  stay `D2` — animated multi-layer is out of scope.
- Extend `filter_usable_section` to reject sections where `layer_count` exceeds
  `max_texture_array_layers`. Add `max_texture_array_layers` floor to the adapter pre-check.
- Update `forward.wgsl`: `texture_2d_array` for bindings 0 and 1; add `@location(5)
  @interpolate(flat) lightmap_layer: u32` to `VertexInput` and `@location(6)
  @interpolate(flat) lightmap_layer: u32` to `VertexOutput`; thread it through `vs_main`;
  update `textureSample` calls for irradiance and direction to pass the layer.
- Update `prl_loader.rs` to parse the new `LightmapSection` format.
- Update `context/lib/build_pipeline.md` to document the new section layout.

### Out of scope

- Animated atlas multi-layer (sections 24/25 stay single-layer; animated overflow is a separate plan).
- Per-chart density coarsening (Strategy C Tier-1); deferred as a follow-on compiler QoL pass.
- Hardware raytracing, denoising, or BC6H pipeline changes.
- Migration tooling for old `.prl` files — all test maps re-bake from source.

## Acceptance criteria

- [ ] `cargo build -p postretro && cargo build -p postretro-level-compiler` compiles clean with no warnings.
- [ ] `cargo test -p postretro-level-format` passes. Includes: a test asserting `VERTEX_SIZE == 36`; a
  round-trip test for the v2 `LightmapSection` wire format covering single-layer and multi-layer cases;
  a test asserting old v1 sections are rejected with an `InvalidData` error.
- [ ] `cargo test -p postretro-level-compiler` passes. Includes a test that forces a two-layer bake and
  asserts that every BVH leaf's charts are placed on a single layer.
- [ ] `cargo run -p postretro -- content/dev/maps/campaign-test.prl` loads and renders without a
  `[Renderer]` lightmap error or degraded placeholder.
- [ ] A two-layer bake (forced via a reduced `MAX_ATLAS_DIMENSION` in a test build, or a dedicated
  integration test) renders both layers correctly in the forward pass.
- [ ] The adapter pre-check logs a `[Renderer]` error and aborts when `max_texture_array_layers` is
  below the required floor.

## Tasks

### Task 1: Widen Vertex to 36 bytes

In `crates/level-format/src/geometry.rs`: add `pub lightmap_layer: u16` and 2 bytes of explicit
padding to `Vertex` after `lightmap_uv`. Update `VERTEX_SIZE` from 32 to 36. Update `to_bytes` to
serialize `lightmap_layer` as `u16` LE then two zero padding bytes. Update `from_bytes` to read and
discard padding. Update `Vertex::new` to accept `lightmap_layer: u16`; all current call sites in the
compiler pass `0` (real layer values come in Task 3). Rename the test
`vertex_is_32_bytes_face_is_8_bytes` and fix its expected byte count to 36.

### Task 2: Restructure LightmapSection for multi-layer

In `crates/level-format/src/lightmap.rs`: replace the 28-byte header with the v2 header defined in
Wire Format below. Decouple irradiance and direction: each has its own `(width, height,
texel_density, format, total_bytes)`. Add `pub layer_count: u32` to `LightmapSection`. Change the
`irradiance` and `direction` `Vec<u8>` fields to hold layer-major blobs (all layers concatenated,
each layer `width × height` texels in the declared format). Update `to_bytes`, `from_bytes`,
`placeholder`, and all existing tests. `from_bytes` must return `InvalidData` when `version ≠ 2`,
naming the received value. Update `context/lib/build_pipeline.md` with the new layout description.

### Task 3: Leaf-aware MaxRects multi-bin packer

In `crates/level-compiler/src/lightmap_bake.rs`: replace `shelf_pack` with `pack_layers`,
implementing MaxRects multi-bin packing. Each chart must carry (or resolve to) its BVH leaf index
— confirm whether `Chart` already carries this field; add it if absent. The packer's hard invariant:
all charts belonging to one BVH leaf are placed on a single layer; when a leaf's charts don't fit
on the current layer, open a new one. Add `const MAX_ATLAS_LAYERS: u32 = 256` and a new error
variant `LightmapBakeError::LayerOverflow { layer_count: u32, max: u32 }`. Per-layer dimensions stay
power-of-two and multiples of 4 (BC6H block alignment). Direction charts pack separately at their
own density but share the same `layer_count` and use the same layer index per face as irradiance.

Update `assign_lightmap_uvs` to also write `lightmap_layer` into each vertex from the `pack_layers`
output. Add a unit test asserting the leaf-cohesion invariant.

### Task 4: Runtime array texture pipeline

In `crates/postretro/src/lighting/lightmap.rs`:
- Update `upload_irradiance_texture` and `upload_direction_texture` to set `depth_or_array_layers =
  section.layer_count`, `dimension = D2`, and `view_dimension = D2Array`.
- Change `bind_group_layout_entries` bindings 0 (irradiance) and 1 (direction) from
  `TextureViewDimension::D2` to `TextureViewDimension::D2Array`. Leave bindings 3 and 5 as `D2`.
- Extend `filter_usable_section` to also reject sections where `section.layer_count` exceeds
  `max_texture_array_layers`, logging a `[Renderer]` error consistent with the existing message.
- Update or extend `usable_atlas_dimensions` if callers need `layer_count` alongside `(width, height)`.

In `crates/postretro/src/render/renderer_init_resources.rs`: add
`const REQUIRED_MAX_TEXTURE_ARRAY_LAYERS: u32 = 256` and the adapter pre-check that bails with a
named `[Renderer]` error when `adapter_limits.max_texture_array_layers < REQUIRED_MAX_TEXTURE_ARRAY_LAYERS`.

In `crates/postretro/src/prl_loader.rs`: update the `LightmapSection::from_bytes` call site (around
the lightmap section read) to handle the new fields; extend the `info!` log to include `layer_count`.

### Task 5: Shader array texture sampling

In `crates/postretro/src/shaders/forward.wgsl`:
- Change bindings 0 (`lightmap_irradiance`) and 1 (`lightmap_direction`) from `texture_2d<f32>` to
  `texture_2d_array<f32>`. Bindings 3 and 5 stay `texture_2d<f32>`.
- Add `@location(5) lightmap_layer: u32` to `VertexInput` (reads the new vertex field).
- Add `@location(6) @interpolate(flat) lightmap_layer: u32` to `VertexOutput`.
- In `vs_main`: `out.lightmap_layer = in.lightmap_layer;`.
- Update `sample_lightmap_irradiance` and the `textureSample(lightmap_direction, …)` call to pass
  `in.lightmap_layer` as the array layer argument.
- Animated atlas sample calls (`animated_lm_atlas`, `animated_lm_direction`) are unchanged.

Audit any other passes that bind group 4 (e.g. shadow passes, wireframe) — if they share the same
BGL, they must also declare `texture_2d_array` for bindings 0 and 1, even if they don't sample them.

## Sequencing

**Phase 1 (sequential):** Task 1, then Task 2 — vertex and format contracts that all downstream
tasks compile against. Task 2 depends on Task 1 only via the `Vertex::new` call-site audit.

**Phase 2 (concurrent):** Task 3 and Task 4 — compiler packer and runtime upload path are
independent once Tasks 1 and 2 are done.

**Phase 3 (sequential):** Task 5 — shader consumes the `D2Array` BGL from Task 4 and the
`lightmap_layer` vertex attribute from Task 1.

## Wire format

### LightmapSection (PRL section id 22), version 2

All fields little-endian.

```
Header (52 bytes):
  u32  version              = 2
  u32  layer_count          shared by irradiance and direction; ≥ 1
  u32  irr_width            per-layer texel width (pow2, ≥ 4)
  u32  irr_height           per-layer texel height (pow2, ≥ 4)
  f32  irr_texel_density    m/texel at bake time (informational)
  u32  irr_format           0 = Rgba16Float  1 = Bc6hRgbUfloat
  u32  irr_total_bytes      byte count for all irradiance layers combined
  u32  dir_width            per-layer texel width for direction atlas (pow2, ≥ 4)
  u32  dir_height           per-layer texel height (pow2, ≥ 4)
  f32  dir_texel_density    m/texel for direction (informational; may differ from irr)
  u32  dir_format           = 0  (Rgba8Unorm octahedral; only defined value)
  u32  dir_total_bytes      byte count for all direction layers combined

Irradiance blob (irr_total_bytes bytes):
  Layer-major: layer 0, layer 1 … layer (layer_count - 1).
  Each layer: irr_width × irr_height texels in irr_format layout.
    Rgba16Float:    u16×4 per texel, row-major (y × irr_width + x).
    Bc6hRgbUfloat:  ceil(w/4)·ceil(h/4)·16 bytes, row-major 4×4 blocks.

Direction blob (dir_total_bytes bytes):
  Layer-major: layer 0, layer 1 … layer (layer_count - 1).
  Each layer: dir_width × dir_height × 4 bytes (Rgba8Unorm, octahedral, row-major).

Optional LMOD trailer (8 bytes; omitted when mode = Shadowed):
  u32  = LIGHTMAP_MODE_TRAILER_MAGIC  (0x444f4d4c, ASCII "LMOD")
  u32  mode  (0 = Shadowed, 1 = Unshadowed)
```

Parsers must reject sections where `version ≠ 2` with an `InvalidData` error naming the received
version. Single-layer bakes write `layer_count = 1`; sampling is identical to the old format but
uses the v2 wire layout.

## Open questions

- **Direction atlas density rule**: the spec allows `dir_width`/`dir_height` to differ from
  irradiance dimensions but does not prescribe the ratio. Task 3's implementer should choose a
  concrete rule (e.g. direction always bakes at half irradiance density, or at the same density).
  Either is correct; the choice affects visual quality, not format correctness.

- **Other passes sharing group 4 BGL**: Task 5 calls out an audit of passes beyond `forward.wgsl`
  that bind group 4. If any such passes exist, their BGL entries for bindings 0 and 1 must also
  become `D2Array`. This may surface during Task 4 or Task 5.
