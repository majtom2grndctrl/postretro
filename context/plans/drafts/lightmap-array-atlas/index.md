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
- [ ] `cargo test -p postretro-level-compiler` includes a test that calls `pack_layers` directly
  with a small per-layer dimension cap and two BVH leaves, producing a two-layer result. The test
  asserts correct layer assignments, valid per-vertex `lightmap_layer` values, and the leaf-cohesion
  invariant. (GPU rendering of both layers is verified manually via `cargo run` with a map that
  triggers overflow.)
- [ ] The adapter pre-check logs a `[Renderer]` error and aborts when `max_texture_array_layers` is
  below the required floor. `filter_usable_section` returns `None` (and logs a `[Renderer]` error)
  when a `LightmapSection` has `layer_count > max_texture_array_layers` (verified by a unit test
  constructing an oversize section).
- [ ] `prl_loader` logs `[PRL] Lightmap: …x… atlas, N layer(s) …` at `info` level when a
  lightmap section is present (prefix stays `[PRL]`, not `[Renderer]`).
- [ ] A unit test in `crates/level-format/src/geometry.rs` asserts that a `Vertex` with a non-zero `lightmap_layer` round-trips through `to_bytes`/`from_bytes` with the layer value preserved. A unit test in `crates/postretro/src/render/renderer_geometry.rs` (or `prl_loader.rs`) asserts that `WorldVertex.lightmap_layer` is serialized at byte offset 32 (i.e. the 4-byte `u32` starting at byte 32 of the serialized `WorldVertex` matches `lightmap_layer`).

## Tasks

### Task 1: Widen Vertex to 36 bytes

In `crates/level-format/src/geometry.rs`: add `pub lightmap_layer: u16` and 2 bytes of explicit
padding to `Vertex` after `lightmap_uv`. Update `VERTEX_SIZE` from 32 to 36. Update `to_bytes` to
serialize `lightmap_layer` as `u16` LE then two zero padding bytes. Update `from_bytes` to read and
discard padding. Update `Vertex::new` to accept `lightmap_layer: u16`; all current call sites in the
compiler pass `0` (real layer values come in Task 3). Rename the test
`vertex_is_32_bytes_face_is_8_bytes` and fix its expected byte count to 36.

Also in `crates/postretro/src/geometry.rs`: add `pub lightmap_layer: u32` (4 bytes) to
`WorldVertex`, the GPU-side runtime struct, and bump `WorldVertex::STRIDE` from 32 to 36.
In `crates/postretro/src/prl_loader.rs` in the `WorldVertex { … }` construction (around
line 1036): add `lightmap_layer: v.lightmap_layer as u32` (widening from u16). In
`crates/postretro/src/render/renderer_geometry.rs`: extend the vertex serializer to write
the new field. The on-disk `Vertex` stores `lightmap_layer: u16`; the GPU struct stores it
as `u32` so the wgpu vertex format is `VertexFormat::Uint32`.

Note: `VERTEX_SIZE` in `geometry.rs` is a private `const`. The AC test asserting
`VERTEX_SIZE == 36` must live in `geometry.rs`'s own `#[cfg(test)] mod tests`.

### Task 2: Restructure LightmapSection for multi-layer

In `crates/level-format/src/lightmap.rs`: replace the 28-byte header with the v2 header defined in
Wire Format below. Decouple irradiance and direction: each has its own `(width, height,
texel_density, format, total_bytes)`. Add `pub layer_count: u32` to `LightmapSection`. Change the
`irradiance` and `direction` `Vec<u8>` fields to hold layer-major blobs (all layers concatenated,
each layer `width × height` texels in the declared format). Update `to_bytes`, `from_bytes`,
`placeholder`, `encode_section` (lightmap_bake.rs:193, which constructs `LightmapSection { width, height, … }`), `log_stats` (lightmap_bake.rs:470, which reads `section.width`/`section.height`), `fake_section` (lightmap.rs test helper), and all existing tests. `from_bytes` must return `InvalidData` when `version ≠ 2`, naming the received value. Note: pre-v2 sections had no version field — their first u32 was `width`. The rejection test should feed a realistic pre-v2 blob (first u32 value ≠ 2, e.g. a typical width like 1024) rather than a synthetic `version=1` header that never existed on disk. Update `context/lib/build_pipeline.md` with the new layout description.

### Task 3: Leaf-aware MaxRects multi-bin packer

In `crates/level-compiler/src/lightmap_bake.rs`: replace `shelf_pack` with `pack_layers`,
implementing MaxRects multi-bin packing. `pack_layers` takes `(charts: &[Chart], max_dim: u32) -> Result<PackOutput, LightmapBakeError>` where `max_dim` is the per-layer maximum width/height; production callers pass `MAX_ATLAS_DIMENSION` and the unit test passes a small value directly. Each chart must carry its BVH leaf index. `Chart` does not currently have this field;
add `pub leaf_index: u32` to `Chart`. The leaf index is populated from
`faces[face_index].leaf_index` (the `FaceMeta.leaf_index` on the BSP cell leaf)
when charts are constructed.

Note: the compiler module `lightmap_layer.rs` and the build-cache key namespace
`"lightmap_layer"` refer to per-light incremental bake layers — a distinct concept from
the atlas array layer index this task introduces. Do not conflate the two in code comments
or cache key strings.

The packer's hard invariant:
all charts belonging to one BVH leaf are placed on a single layer; when a leaf's charts don't fit
on the current layer, open a new one. Add `const MAX_ATLAS_LAYERS: u32 = 256` and a new error
variant `LightmapBakeError::LayerOverflow { layer_count: u32, max: u32 }`. Per-layer dimensions stay
power-of-two and multiples of 4 (BC6H block alignment). Direction charts pack separately at their
own density but share the same `layer_count` and use the same layer index per face as irradiance.

Update `assign_lightmap_uvs` to also write `lightmap_layer` into each vertex from the `pack_layers`
output. Add a unit test asserting the leaf-cohesion invariant.
The two-layer bake test must call `pack_layers` directly with a small max-dimension argument
(not via the private `MAX_ATLAS_DIMENSION` constant) so the test is self-contained within
`lightmap_bake.rs`'s test module.

### Task 4: Runtime array texture pipeline

In `crates/postretro/src/lighting/lightmap.rs`:
- Update `upload_irradiance_texture` and `upload_direction_texture` to set `depth_or_array_layers =
  section.layer_count`, `dimension = D2`, and `view_dimension = D2Array`.
- Change `bind_group_layout_entries` bindings 0 (irradiance) and 1 (direction) from
  `TextureViewDimension::D2` to `TextureViewDimension::D2Array`. Leave bindings 3 and 5 as `D2`.
- Extend `filter_usable_section` to also reject sections where `section.layer_count` exceeds
  `max_texture_array_layers`, logging a `[Renderer]` error consistent with the existing message.
  This branch guards against corrupt or future sections whose `layer_count` exceeds the
  device limit; on a spec-compliant adapter it never fires in normal use (the bake cap
  `MAX_ATLAS_LAYERS` equals the required runtime floor). The AC for this branch is satisfied
  by a unit test that constructs a `LightmapSection` with an inflated `layer_count` and
  asserts the function returns `None`.
- Repoint the existing `s.width`/`s.height` reads in `filter_usable_section` (lightmap.rs:268–269) and `usable_atlas_dimensions` (lightmap.rs:258) to `s.irr_width`/`s.irr_height`. Update the `fake_section` test helper and any `.width`/`.height` field asserts in lightmap.rs tests to use the new names.
- `usable_atlas_dimensions` needs no signature change — `upload_irradiance_texture` and `upload_direction_texture` already receive `&LightmapSection` and can read `layer_count` directly from it.

In `crates/postretro/src/render/renderer_init_resources.rs`: add
`const REQUIRED_MAX_TEXTURE_ARRAY_LAYERS: u32 = 256`. Add the adapter pre-check that bails with a named `[Renderer]` error when `adapter_limits.max_texture_array_layers < REQUIRED_MAX_TEXTURE_ARRAY_LAYERS` — this produces the friendly diagnostic before `request_device` is called. The `required_limits` entry is the hard backstop that causes `request_device` to fail on non-conformant adapters; both mechanisms coexist intentionally. Also set
`max_texture_array_layers: REQUIRED_MAX_TEXTURE_ARRAY_LAYERS` in the `required_limits`
struct so the device is actually asked to provide this floor. Correct stale doc-comment
references to `render::mod.rs` in `crates/postretro/src/lighting/lightmap.rs` and
`crates/level-compiler/src/lightmap_bake.rs` to point to `renderer_init_resources.rs`.

In `crates/postretro/src/prl_loader.rs`: update the `LightmapSection::from_bytes` call site (around
the lightmap section read) to handle the new fields; extend the `info!` log to emit `[PRL] Lightmap: {w}x{h} atlas, {n} layer(s), …` matching the AC's expected prefix and `layer(s)` token.

### Task 5: Shader array texture sampling

In `crates/postretro/src/shaders/forward.wgsl`:
- Change bindings 0 (`lightmap_irradiance`) and 1 (`lightmap_direction`) from `texture_2d<f32>` to
  `texture_2d_array<f32>`. Bindings 3 and 5 stay `texture_2d<f32>`.
- Add `@location(5) lightmap_layer: u32` to `VertexInput` (reads the new vertex field).
- Add `@location(6) @interpolate(flat) lightmap_layer: u32` to `VertexOutput`.
- In `vs_main`: `out.lightmap_layer = in.lightmap_layer;`.
- Change `sample_lightmap_irradiance`'s signature to `fn sample_lightmap_irradiance(uv: vec2<f32>, layer: u32)`, update its `textureSample` call to pass `layer` as the array index, and update its call site in `fs_main` to pass `in.lightmap_layer`. Update the `textureSample(lightmap_direction, …)` call similarly.
- Animated atlas sample calls (`animated_lm_atlas`, `animated_lm_direction`) are unchanged.

Update every `wgpu::VertexBufferLayout` that uses `WorldVertex::STRIDE` as its `array_stride`.
This includes the forward pass, the depth prepass, and any shadow-depth pipeline variants in
`crates/postretro/src/render/renderer_init_pipelines.rs`. Each layout must: (a) update
`array_stride` to 36, and (b) add `VertexAttribute { offset: 32, format: VertexFormat::Uint32, shader_location: 5 }` for the forward-pass layout only (the layout that feeds `forward.wgsl`'s `vs_main`; in `renderer_init_pipelines.rs` this is the layout at approx. line 64 — confirm by the pipeline label or the fact it includes the `lightmap_uv_packed` attribute at shader_location 4). The depth prepass and shadow pipelines need
the updated stride but do not need the new attribute (they do not read `lightmap_layer`).

Animated atlas limitation: faces on atlas layers ≥ 1 receive no animated lighting. The
animated atlas is single-layer and covers layer-0 UV space only; sampling it at a layer-1
UV yields incorrect results. Guard the animated atlas sample in the fragment shader:

    var lm_anim = vec3<f32>(0.0);
    var anim_dir_sample = vec4<f32>(0.5, 1.0, 0.5, 0.0);
    if in.lightmap_layer == 0u {
        lm_anim = sample_lightmap_animated(in.lightmap_uv);
        anim_dir_sample = textureSample(animated_lm_direction, lightmap_sampler, in.lightmap_uv);
    }

The default `anim_dir_sample = vec4<f32>(0.5, 1.0, 0.5, 0.0)` sets `.a = 0.0`, which is the zero-coverage sentinel the existing `use_correction_anim` gate keys on — layer ≥ 1 faces take the static-only path.

Note: the existing shader has separate `let lm_anim` (~line 687) and `let anim_dir_sample` (~line 718) declarations 31 lines apart. To apply this guard, convert both to `var`, hoist `var anim_dir_sample` up before `var lm_anim`, and replace both `let` assignments with the guarded block above.

Update the doc-comment above the animated atlas bindings (group 4 bindings 3 and 5) to
note this limitation. Note: `skinned_mesh.wgsl` also uses `@group(4)` but for the
SH/dynamic-direct BGL — a different layout entirely — so it is out of scope for this change.

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
Header (48 bytes):
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

The optional LMOD trailer follows the entire direction blob, at byte offset
`48 + irr_total_bytes + dir_total_bytes`. `placeholder` emits `layer_count = 1` and
positions the trailer at `48 + irr_bytes + dir_bytes` exactly as in single-layer bakes.
The trailer round-trip test in Task 2 must cover a multi-layer section.

Parsers must reject sections where `version ≠ 2` with an `InvalidData` error naming the
received value. Parsers must also continue to reject `irr_format ∉ {0, 1}` and
`dir_format ≠ 0` with `InvalidData` (preserving the existing checks from the v1 parser).
Single-layer bakes write `layer_count = 1`; the v2 wire layout is always used.

## Decisions

- **Direction atlas density**: direction uses the same per-layer `(width, height)` as irradiance.
  Direction is nearest-sampled (octahedral lerp ≠ slerp) and low-frequency; higher resolution adds
  no visual benefit for a retro boomer shooter aesthetic. The wire format retains independent
  `dir_width`/`dir_height` fields so a coarser-direction optimisation can land without a format
  version bump, but `pack_layers` (this plan) sets `dir_width = irr_width` and `dir_height = irr_height`, and writes `dir_texel_density = irr_texel_density`.
