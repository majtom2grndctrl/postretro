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
- Make the compiler bake atlas layer-aware: `ChartPlacement.layer`, layer-major `CompositedAtlas`,
  per-layer dilation, and the `lightmap_layer.rs` incremental-cache seam (`LayerTexel`,
  `LightmapLayer`, `composite_layers`, `atlas_layout_fingerprint`) all grow a layer dimension so the
  byte-identity cache-exactness gate still holds. The incremental-cache on-disk format bumps
  (dev-local, regenerated on next bake).
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
- [ ] `cargo test -p postretro-level-compiler` includes a multi-layer byte-identity test asserting
  the per-light composite (`composite_layers` → `CompositedAtlas`) equals the monolithic bake for a
  two-layer atlas, extending the existing single-layer cache-exactness gate to multi-layer.
- [ ] The adapter pre-check logs a `[Renderer]` error and aborts when `max_texture_array_layers` is
  below the required floor. Factor the comparison into a pure helper (e.g.
  `fn array_layers_sufficient(limit: u32) -> bool`) and unit-test it, since no real adapter exposes
  `< 256` to exercise the full path. `filter_usable_section` returns `None` (and logs a `[Renderer]`
  `filter_usable_section` (which gains a `max_texture_array_layers` parameter, see Task 4) returns
  `None` (and logs a `[Renderer]` error) when a `LightmapSection` has
  `layer_count > max_texture_array_layers` (verified by a unit test passing a small layer limit and an
  oversize section).
- [ ] `prl_loader` logs `[PRL] Lightmap: …x… atlas, N layer(s) …` at `info` level when a
  lightmap section is present (prefix stays `[PRL]`, not `[Renderer]`).
- [ ] A unit test in `crates/level-format/src/geometry.rs` asserts that a `Vertex` with a non-zero `lightmap_layer` round-trips through `to_bytes`/`from_bytes` with the layer value preserved. A unit test in `crates/postretro/src/render/renderer_geometry.rs` (or `prl_loader.rs`) asserts that `WorldVertex.lightmap_layer` is serialized at byte offset 32 (i.e. the 4-byte `u32` starting at byte 32 of the serialized `WorldVertex` matches `lightmap_layer`).

## Tasks

### Task 1: Widen Vertex to 36 bytes

In `crates/level-format/src/geometry.rs`: add `pub lightmap_layer: u16` and 2 bytes of explicit
padding to `Vertex` after `lightmap_uv`. Update `VERTEX_SIZE` from 32 to 36. Update `to_bytes` to
serialize `lightmap_layer` as `u16` LE then two zero padding bytes. Update `from_bytes` to read and
discard padding. Update `Vertex::new` to accept `lightmap_layer: u16`; all current call sites — including
`#[cfg(test)]` modules in `geometry.rs`, `lightmap_bake.rs`, and any other crate (grep `Vertex::new`
to enumerate) — pass `0` (real layer values come in Task 3b). Rename the test
`vertex_is_32_bytes_face_is_8_bytes` and fix its expected byte count to 36.

Also in `crates/postretro/src/geometry.rs`: add `pub lightmap_layer: u32` (4 bytes) to
`WorldVertex`, the GPU-side runtime struct, and bump `WorldVertex::STRIDE` from 32 to 36.
In `crates/postretro/src/prl_loader.rs` in the `WorldVertex { … }` construction (the
`.map(|v| WorldVertex { … })` closure, around line 1031): add
`lightmap_layer: v.lightmap_layer as u32` (widening from u16). In
`crates/postretro/src/render/renderer_geometry.rs`: in `cast_world_vertices_to_bytes`, append
`lightmap_layer.to_ne_bytes()` after `lightmap_uv` as the last field (bytes 32–35 of the serialized
layout), matching the `VertexAttribute { offset: 32 }` declared in Task 5. Declare `lightmap_layer`
as the last field in the `WorldVertex` struct. The on-disk `Vertex` stores `lightmap_layer: u16`; the GPU struct stores it
as `u32` so the wgpu vertex format is `VertexFormat::Uint32`.

Note: `VERTEX_SIZE` in `geometry.rs` is a private `const`. The AC test asserting
`VERTEX_SIZE == 36` must live in `geometry.rs`'s own `#[cfg(test)] mod tests`.

### Task 2: Restructure LightmapSection for multi-layer

In `crates/level-format/src/lightmap.rs`: replace the 28-byte header with the v2 header defined in
Wire Format below. Decouple irradiance and direction: each has its own `(width, height,
texel_density, format, total_bytes)`. Add `pub layer_count: u32` to `LightmapSection`. Change the
`irradiance` and `direction` `Vec<u8>` fields to hold layer-major blobs (all layers concatenated,
each layer `width × height` texels in the declared format). Update `to_bytes`, `from_bytes`,
`placeholder`, `log_stats` (lightmap_bake.rs:470, which reads `section.width`/`section.height`), `fake_section` (lightmap.rs test helper), and all existing tests. Do not update `encode_section` (defined at lightmap_bake.rs:~172) in this task — its full v2 rewrite (layer-major blobs, `layer_count` wiring) is owned by Task 3a. After Task 2, `encode_section` will not compile because it still constructs `LightmapSection { width, height, … }` using the removed fields; this is resolved in Phase 2. `from_bytes` must return `InvalidData` when `version ≠ 2`, naming the received value. Note: pre-v2 sections had no version field — their first u32 was `width`. The rejection test should feed a realistic pre-v2 blob (first u32 value ≠ 2, e.g. a typical width like 1024) rather than a synthetic `version=1` header that never existed on disk. Update `context/lib/build_pipeline.md` with the new layout description.

### Task 3a: Layer-aware bake atlas and incremental-cache seam

Multi-layer makes the compiler's bake atlas layer-indexed. This is the largest task: the
`CompositedAtlas` is the byte-identity comparison seam between the monolithic bake and the per-light
incremental composite (`lightmap_layer.rs`), so every structure on that seam grows a layer dimension
in lockstep or the cache exactness gate breaks.

In `crates/level-compiler/src/chart_raster.rs`: add `pub layer: u32` to `ChartPlacement` (currently
`{ x, y }`). Every placement producer and consumer threads the layer through.

In `crates/level-compiler/src/lightmap_bake.rs`:
- `CompositedAtlas` (`{ irradiance: Vec<f32>, direction: Vec<Vec3>, coverage: Vec<bool>,
  atlas_width, atlas_height }`): add `pub layer_count: u32`. The three buffers become layer-major,
  sized `layer_count × atlas_width × atlas_height` (irradiance ×4 floats). `zeroed` takes
  `(atlas_w, atlas_h, layer_count)`. Texel addressing becomes
  `layer × (atlas_width × atlas_height) + y × atlas_width + x`.
- `CompositedAtlas::dilate`: run edge dilation per layer over each layer's slice independently —
  dilation must not bleed across layer boundaries.
- `CompositedAtlas::encode_section`: emit the v2 multi-layer `LightmapSection` from Task 2 —
  layer-major blobs, `layer_count`, `irr_width/irr_height = atlas_width/atlas_height`,
  `dir_width/dir_height = atlas_width/atlas_height`. The method's existing `texel_density` parameter
  writes both `irr_texel_density` and `dir_texel_density` (set them equal; see Decisions). The BC6H
  encoder (`encode_bc6h_rgb_from_f32_rgba`) is single-image and `debug_assert`s its input length is
  `width × height × 4`, so the BC6H path must loop over layers: slice `self.irradiance` into
  per-layer `atlas_width × atlas_height × 4` chunks, encode each, and concatenate into the
  layer-major irradiance blob. The RGBA16F irradiance and Rgba8Unorm direction paths can stay
  whole-buffer (their output is already flat layer-major).
- `bake_monolithic_atlas` / `bake_face_chart`: write each face's texels into its placement's layer
  slice (offset by `placement.layer × atlas_width × atlas_height`).
- `PreparedAtlas` and `LightmapBakeOutput`: add `pub layer_count: u32`. `LightmapBakeOutput` is
  constructed at five sites that all must set the field: `lightmap_bake.rs` ~309, ~325, ~378 (inside
  `bake_lightmap`; placeholder / empty-geometry paths use `layer_count = 1`) and `main.rs` ~534 and
  ~656. `prepare_atlas` propagates `layer_count` onto `PreparedAtlas`. `SharedAtlas` does not need a
  `layer_count` field — the layer is carried on individual `ChartPlacement.layer`.
- `log_stats`: read `section.irr_width` / `section.irr_height` / `section.layer_count` (the v1
  `section.width` / `section.height` fields are gone after Task 2).

In `crates/level-compiler/src/lightmap_layer.rs` (the per-light incremental cache):
- `LayerTexel`: add `pub layer: u32` (keep `idx` as the within-layer `y × atlas_width + x`). A single
  global `u32` index would overflow at large multi-layer sizes, so the layer is carried explicitly.
  Update the `size_of::<LayerTexel>() == 40` static assertion to `== 44`.
- `LightmapLayer`: add `pub layer_count: u32`; its `to_bytes` / `from_bytes` header grows from 12 to
  16 bytes (`atlas_width`, `atlas_height`, `layer_count`, texel `count`). Update `LAYER_HEADER_BYTES`.
- `composite_layers` (current signature `composite_layers(layers: &[LightmapLayer], atlas_w: u32,
  atlas_h: u32)`): keep the signature as-is — read `layer_count` from `layers[0].layer_count` (all
  entries share the same atlas layout), size the output `CompositedAtlas` for `layer_count` layers,
  and index each texel at `t.layer × (atlas_w × atlas_h) + t.idx`. The `atlas_w`/`atlas_h` params
  stay; only `layer_count` is newly read from the slice, so the call site in `main.rs` (~line 643)
  and the test call sites in `lightmap_layer.rs` need no argument change.
- `atlas_layout_fingerprint`: fold `p.layer` into the digest alongside `p.x` / `p.y` so a repack that
  changes layer assignment invalidates the per-light cache.
- `bake_light_layer` (lightmap_layer.rs:~190–243), the sole `LayerTexel` producer: resolve each
  texel's atlas layer from `placements[chart_index].layer` into `LayerTexel.layer`, and compute `idx`
  as the within-layer `atlas_y * atlas_w + atlas_x` (not a global index across all layers).

The incremental-bake cache's on-disk `LayerTexel` / `LightmapLayer` format changes, invalidating
existing cache entries; they regenerate on the next bake (dev-local cache, never shipped).

In `crates/level-compiler/src/animated_light_weight_maps.rs`: the animated-chunk baker consumes
`atlas_width` + placements to resolve chunk rects. Animated stays single-layer (out of scope), so
animated chunks live only on layer 0; assert or filter that the placements it consumes are
`layer == 0`. (Faces on layers ≥ 1 receive no animated lighting — see Task 5's shader guard.)

### Task 3b: Leaf-aware MaxRects packer and per-vertex layer assignment

In `crates/level-compiler/src/lightmap_bake.rs`: replace `shelf_pack` with `pack_layers`,
implementing leaf-aware MaxRects multi-bin packing. `pack_layers` takes
`(charts: &[Chart], max_dim: u32) -> Result<PackOutput, LightmapBakeError>` where `max_dim` is the
per-layer maximum width/height; production callers pass `MAX_ATLAS_DIMENSION` and the unit test
passes a small value directly. `PackOutput` is
`{ layer_count: u32, atlas_width: u32, atlas_height: u32, placements: Vec<ChartPlacement> }` — all
layers share one `(atlas_width, atlas_height)` (a `texture_2d_array` has a single per-layer
dimension for all layers); each `ChartPlacement` carries its `layer` (Task 3a). This replaces
`shelf_pack`'s `(u32, u32, Vec<ChartPlacement>)` return; update `prepare_atlas`'s destructure.
`prepare_atlas` has two packer call sites: the main path and the no-static-lights branch (~247–256)
that currently falls back to `(1, 1, vec![])` on error. Update both; the fallback must construct
`PackOutput { layer_count: 1, atlas_width: 1, atlas_height: 1, placements: vec![] }` and populate
`PreparedAtlas.layer_count` accordingly.

Each chart carries its BVH leaf index. `Chart` does not currently have this field; add
`pub leaf_index: u32`. `plan_charts` populates it from `geom.geometry.faces[face_index].leaf_index`
(`FaceMeta.leaf_index`), which is 1:1 with chart index. `empty_chart()` sets `leaf_index: u32::MAX`.

Note: the compiler module `lightmap_layer.rs` and the build-cache key namespace `"lightmap_layer"`
refer to per-light incremental bake layers — a distinct concept from the atlas array layer index
this task introduces. Do not conflate the two in code comments or cache key strings.

The packer's hard invariant: all charts belonging to one BVH leaf are placed on a single layer. A
leaf's charts never straddle a layer boundary; when a leaf's charts don't fit in the current layer's
free rectangles, open a new layer. All layers share the single `(atlas_width, atlas_height)` chosen
to fit the largest leaf's charts up to `max_dim`. Add `const MAX_ATLAS_LAYERS: u32 = 256` and a new
error variant `LightmapBakeError::LayerOverflow { layer_count: u32, max: u32 }`, returned when the
layer count would exceed `MAX_ATLAS_LAYERS`. Remove the existing `LightmapBakeError::AtlasOverflow` variant — the multi-bin packer never fails on atlas area (it opens more layers instead); `LayerOverflow` is its replacement. `ChartTooLarge` is preserved for charts whose largest side exceeds `max_dim`. Also remove the two retry-on-overflow sites in `crates/level-compiler/src/main.rs` that pattern-matched on `AtlasOverflow`: the warm-path helper `prepare_lightmap_atlas_with_retry` (~253–284) and the cold-path retry loop (~667–710). Both doubled `texel_density` and retried on area overflow — dead code once `pack_layers` opens new layers instead. The warm-path helper returns `(PreparedAtlas, f32)` and its caller (~main.rs:522) destructures `(prepared, density)` then sets `final_lightmap_density = density`; replace that with a direct `prepare_atlas` call returning `PreparedAtlas` and set `final_lightmap_density = lightmap_config.lightmap_density` (the fixed density that was previously the retry seed). The cold-path loop wraps `bake_lightmap` (not `prepare_atlas`); replace it with a single non-retrying `bake_lightmap` call and source `final_lightmap_density` the same way. `ChartTooLarge` is a hard error (per-chart density coarsening is out of scope); propagate it to the caller. Per-layer dimensions stay power-of-two and multiples of 4 (BC6H block alignment).

Direction reuses irradiance's placements and layer indices verbatim — same `(atlas_width,
atlas_height)`, same per-face layer (see Decisions). There is no separate direction pack.

Update `assign_lightmap_uvs` to also write `lightmap_layer` into each vertex from `PackOutput`: each
vertex's `lightmap_layer = placement.layer`, and its `lightmap_uv` normalizes against the shared
per-layer `(atlas_width, atlas_height)`. Its signature reads the layer from the threaded
`placements` (which now carry `layer`).

Add a unit test asserting the leaf-cohesion invariant. The two-layer bake test calls `pack_layers`
directly with a small `max_dim` argument and two BVH leaves (self-contained in `lightmap_bake.rs`'s
test module; does not touch the private `MAX_ATLAS_DIMENSION`). Update every test that references the removed `shelf_pack` / `AtlasOverflow` symbols (grep both to
enumerate): delete the ones that test removed area-overflow behavior —
`shelf_pack_overflows_past_8192_cap` (~lightmap_bake.rs:1988) and
`shelf_pack_uses_new_8192_cap_for_overflowing_set` (~1972) — and the `AtlasOverflow` match test
(~2475); port the ones that test still-valid packer properties —
`shelf_pack_is_deterministic` (~1941) and
`shelf_pack_dims_are_pow2_4aligned_under_cap_across_chart_sets` (~2145) — to call `pack_layers` with
its `(charts, max_dim)` signature and destructure `PackOutput`.

`prepare_atlas` currently runs a pre-pack loop that returns `ChartTooLarge` against the hardcoded
`MAX_ATLAS_DIMENSION` (~lightmap_bake.rs:264–275). Remove that pre-check — `pack_layers` now owns the
`ChartTooLarge` check against its `max_dim` argument, and production callers pass `MAX_ATLAS_DIMENSION`
as `max_dim`, so behavior is unchanged with a single source of truth.

### Task 4: Runtime array texture pipeline

In `crates/postretro/src/lighting/lightmap.rs`:
- Update `upload_irradiance_texture` and `upload_direction_texture` to set `depth_or_array_layers =
  section.layer_count`, `dimension = D2`, and `view_dimension = D2Array`.
- Change `bind_group_layout_entries` bindings 0 (irradiance) and 1 (direction) from
  `TextureViewDimension::D2` to `TextureViewDimension::D2Array`. Leave bindings 3 and 5 as `D2`.
- Extend `filter_usable_section`'s signature to take `max_texture_array_layers: u32` alongside the
  existing `max_texture_dimension_2d`, and reject sections where `section.layer_count` exceeds it,
  logging a `[Renderer]` error consistent with the existing message. Thread the limit from the call
  site (lightmap.rs:~118) via `device.limits().max_texture_array_layers`, through
  `usable_atlas_dimensions` if it forwards to `filter_usable_section`, and update the existing
  `filter_usable_section` tests (~lightmap.rs:456–520) to pass the new argument. This branch guards
  against corrupt or future sections whose `layer_count` exceeds the device limit; on a spec-compliant
  adapter it never fires in normal use (the bake cap `MAX_ATLAS_LAYERS` equals the required runtime
  floor). The AC for this branch is satisfied by a unit test that constructs a `LightmapSection` with
  an inflated `layer_count` and asserts the function returns `None`.
- Repoint the existing `s.width`/`s.height` reads in `filter_usable_section` (lightmap.rs:268–269) and `usable_atlas_dimensions` (lightmap.rs:258) to `s.irr_width`/`s.irr_height`. Update the `fake_section` test helper and any `.width`/`.height` field asserts in lightmap.rs tests to use the new names.
- `usable_atlas_dimensions` needs no signature change — `upload_irradiance_texture` and `upload_direction_texture` already receive `&LightmapSection` and can read `layer_count` directly from it.

In `crates/postretro/src/render/renderer_init_resources.rs`: add
`const REQUIRED_MAX_TEXTURE_ARRAY_LAYERS: u32 = 256`. Implement `fn array_layers_sufficient(limit: u32) -> bool { limit >= REQUIRED_MAX_TEXTURE_ARRAY_LAYERS }` in this file. Add the adapter pre-check that bails with a named `[Renderer]` error when `!array_layers_sufficient(adapter_limits.max_texture_array_layers)` — this produces the friendly diagnostic before `request_device` is called. The `required_limits` entry is the hard backstop that causes `request_device` to fail on non-conformant adapters; both mechanisms coexist intentionally. Also set
`max_texture_array_layers: REQUIRED_MAX_TEXTURE_ARRAY_LAYERS` in the `required_limits`
struct so the device is actually asked to provide this floor. Correct stale doc-comment references to `render::mod.rs`'s adapter pre-check in
`crates/postretro/src/lighting/lightmap.rs` (~lines 112, 344) and
`crates/level-compiler/src/lightmap_bake.rs` (~line 33) to point to `renderer_init_resources.rs`
(grep `render::mod` to catch every stale pointer to the pre-check; ignore hits in files where `mod.rs` is referenced for unrelated reasons — only update references that point to the adapter pre-check).

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

Update every `wgpu::VertexBufferLayout` that uses `WorldVertex::STRIDE` as its `array_stride`. There
are five in `crates/postretro/src/render/renderer_init_pipelines.rs`: `Textured Pipeline` (forward,
~line 65), `Wireframe Cull Status Pipeline` (~line 170), `Wireframe Visible Pipeline` (~line 235),
`Depth Pre-Pass Pipeline` (~line 311), and `Spot Shadow Depth Pipeline` (~line 402). Each must update
`array_stride` to 36. Only the forward-pass `Textured Pipeline` layout also adds
`VertexAttribute { offset: 32, format: VertexFormat::Uint32, shader_location: 5 }` (it feeds
`forward.wgsl`'s `vs_main`; confirm by the `Textured Pipeline` label and its `lightmap_uv`
attribute at shader_location 4, offset 28). The other four need the stride bump only — they do not read
`lightmap_layer`. The dummy-vertex buffer sizing (`vec![0u8; WorldVertex::STRIDE]`, ~line 468) tracks
the constant automatically. Only `forward.wgsl`'s `VertexInput` gains `@location(5)`; the
depth-prepass and spot-shadow shaders keep their `VertexInput` unchanged (they do not read the layer).

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

Note: the existing shader has separate `let lm_anim` (~line 687) and `let anim_dir_sample` (~line 718) declarations 31 lines apart. To apply this guard, convert both to `var`, hoist `var anim_dir_sample` up before `var lm_anim`, and replace both `let` assignments with the guarded block above. Leave the downstream `dom_anim`, `anim_covered`, and `use_correction_anim` lines intact — they now consume the hoisted `var`s; the guard changes only how `lm_anim` / `anim_dir_sample` are assigned, not the correction math that follows.

Update the doc-comment above the animated atlas bindings (group 4 bindings 3 and 5) to
note this limitation. Note: `skinned_mesh.wgsl` also uses `@group(4)` but for the
SH/dynamic-direct BGL — a different layout entirely — so it is out of scope for this change.

## Sequencing

**Phase 1 (sequential):** Task 1, then Task 2 — vertex and format contracts that all downstream
tasks compile against. Task 2 depends on Task 1 only via the `Vertex::new` call-site audit. Note:
after Task 2 neither `postretro` nor `postretro-level-compiler` builds: `postretro` because Task 4
hasn't yet repointed `s.width`/`s.height` to `s.irr_width`/`s.irr_height`; `postretro-level-compiler`
because `encode_section` still references the removed fields until Task 3a rewrites it. Only
`postretro-level-format` builds at the Phase-1 boundary.

**Phase 2 (concurrent):** Task 3a (layer-aware bake atlas + incremental-cache seam) and Task 4
(runtime array-texture upload path) — independent once Tasks 1 and 2 are done.

**Phase 3 (concurrent):** Task 3b (leaf-aware `pack_layers` + per-vertex layer assignment, consuming
Task 3a's `ChartPlacement.layer` and layer-major `CompositedAtlas`) and Task 5 (shader, consuming
Task 4's `D2Array` BGL and Task 1's `lightmap_layer` vertex attribute).

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
Single-layer bakes write `layer_count = 1`; the v2 wire layout is always used. v2 `from_bytes`
keeps the v1 behavior of ignoring bytes past the LMOD trailer (forward-compat slack); it does not
reject trailing bytes.

### LightmapLayer incremental-cache blob (compiler-internal, dev-local)

Native-endian (never shipped, never read cross-architecture). Header grows 12 → 16 bytes:

```
  u32  atlas_width      per-layer texel width
  u32  atlas_height     per-layer texel height
  u32  layer_count      number of atlas layers
  u32  count            number of LayerTexel entries that follow
```

Followed by `count` × `LayerTexel` (44 bytes each: within-layer `idx` = `y × atlas_width + x`,
`layer`, `irradiance[3]`, `weighted_dir[3]`, `fallback_normal[3]`). The format bump invalidates
existing cache entries; they regenerate on the next bake.

## Decisions

- **Direction atlas density**: direction uses the same per-layer `(width, height)` as irradiance.
  Direction is nearest-sampled (octahedral lerp ≠ slerp) and low-frequency; higher resolution adds
  no visual benefit for a retro boomer shooter aesthetic. The wire format retains independent
  `dir_width`/`dir_height` fields so a coarser-direction optimisation can land without a format
  version bump, but `pack_layers` (this plan) sets `dir_width = irr_width` and `dir_height = irr_height`, and writes `dir_texel_density = irr_texel_density`.
- **Layer-aware bake atlas (not tall-buffer slicing)**: the compiler's `CompositedAtlas` and the
  per-light incremental-cache seam (`LayerTexel`, `LightmapLayer`, `composite_layers`,
  `atlas_layout_fingerprint`) become genuinely layer-indexed rather than stacking layers in one tall
  2D buffer. This keeps each structure's layer semantics explicit and the byte-identity cache gate
  intact. Cost: the on-disk incremental-cache format bumps (header +4 bytes, `LayerTexel` 40 → 44
  bytes), invalidating existing cache entries — acceptable because the cache is dev-local and
  regenerates on the next bake.
