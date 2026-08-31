# Shadowmask No-Drop Atlas — research notes

Investigation record. Decisions live in `index.md`.

## The 4-mask ceiling is a coupled contract across three layers

The atlas array **layer is the receiver's `lightmap_layer`** (shared with the
irradiance/direction atlases), **not** a per-light axis. The only per-light axis today
is the 4 RGBA channels of one `Rgba8Unorm` texel. So a texel can carry ≤4 masks; a
5th overlapping selected light is dropped (`assign_channels_with_drops`, lowest
intensity, logs `[ShadowmaskAtlas] dropped N ... after >4-way overlap`).

The ceiling lives in three coupled places:

1. **On-disk** — `crates/level-format/src/shadowmask_atlas.rs` (153 lines).
   `ShadowmaskAtlasSection { width, height, layer_count, channels: Vec<u8>, data:
   Vec<u8> }`. `SHADOWMASK_CHANNEL_DROPPED = 0xFF`, `SHADOWMASK_TEXEL_BYTES = 4`.
   `to_bytes`/`from_bytes`: little-endian, 16-byte header (`width`, `height`,
   `layer_count`, `selected_light_count`), then `channels` bytes, pad to 4, then
   `data` (layer-major `Rgba8Unorm`, `width*height*layer_count*4`). `from_bytes`
   validates every channel `<= 3 || == 0xFF` and cross-checks `expected_payload =
   width*height*layer_count*4`. Section id **42**.
2. **Runtime metadata** — a per-selection `Vec<u8>` channel flows into two f32 fields,
   both sentinel-capped at 4.0:
   - `SpecLight.shadowmask_channel` (spec buffer byte 56, `crates/lighting/src/spec_buffer.rs`;
     `SPEC_LIGHT_SHADOWMASK_NONE = 4.0`), read in-shader as `sl.cone_cos.z`.
   - Promoted-light record `meta1.z` (byte 24 of the 2-vec4 record;
     `FORWARD_SHADOWMASK_META_VEC4S_PER_RECORD = 2`), packed by
     `pack_forward_shadowmask_metadata` in `crates/renderer/src/render/shadowmask.rs`
     (also `build_spec_light_shadowmask_channels`). `shadowmask_present == false`
     forces the dropped sentinel for every record.
   The renderer stores `shadowmask_channels: Vec<u8>` (`render/renderer_types.rs`),
   threaded from `section.channels` at `renderer_full_init.rs`,
   `renderer_resources.rs`, `renderer_init_resources.rs`, `renderer_light_slots.rs`.
3. **Shader** — `crates/renderer/src/shaders/forward.wgsl` (1368 lines).
   `@group(4) @binding(6) shadowmask_atlas: texture_2d_array<f32>` (L227).
   `shadowmask_channel_value(mask: vec4<f32>, channel: u32)` switches 0/1/2/3 (L638).
   `sample_shadowmask_atlas(lightmap_uv, lightmap_layer)` samples the array layer =
   receiver's `lightmap_layer` (L651). Two decode paths cap at
   `SHADOWMASK_CHANNEL_DROPPED = 4.0` (L611): static world specular
   (`shadowmask_visibility_for_spec_light`, reads `cone_cos.z`) and promoted-light
   union subtraction (`shadowmask_union_subtraction`, reads `meta1.z`). Shader tests:
   `render/tests/shader_tests.rs` L429/433/447/600.

## GPU upload — `crates/renderer/src/lighting/lightmap.rs` (966 lines)

`upload_shadowmask_texture` (L567): `Extent3d { width, height, depth_or_array_layers:
layer_count }`, `format: Rgba8Unorm` (L584), `D2`, `TextureDataOrder::LayerMajor`.
`filter_usable_shadowmask_section` (L431): drops when any of `width|height|layer_count
== 0`, `width/height > max_texture_dimension_2d`, `layer_count >
max_texture_array_layers` → `[Renderer]` error + 1×1×1 all-visible placeholder
(`upload_placeholder_shadowmask`), `shadowmask_present = false`. Binding
`BIND_SHADOWMASK_ATLAS = 6`, group 4, BGL `view_dimension: D2Array` (L353). Device
limit `REQUIRED_MAX_TEXTURE_ARRAY_LAYERS = 256`.

## Chosen expansion: stacked array-layer channel blocks

The natural expansion for the existing `texture_2d_array` shape: add a **block**
dimension stacked in the array layers. Atlas array layers = `layer_count ×
block_count`; a light's mask lives at `(block, channel)`, sampled at array layer
`lightmap_layer + block * layer_count`. The per-light metadata carries a slot
`s = block*4 + channel` instead of a bare channel; the shader decodes `block = s/4`,
`channel = s%4`. One texture, one binding, one sample per light (as today, at a
computed layer). Capacity = `4 × block_count` masks per texel, bounded by
`layer_count × block_count ≤ max_texture_array_layers = 256`.

Rejected alternatives:
- **Second shadowmask texture/binding** — adds a `BIND_*` + BGL + `@binding`, and
  does not scale past 2 groups (each new group is a new binding + sample). Stacked
  layers scale to `256/layer_count` blocks with one binding.
- **Variable-length per-texel light list + indirection** — large shader / data-model
  departure, poor GPU fit.
- **Rank/drop better** — still drops; violates the directive.

## Contracts to preserve (rendering_pipeline.md §4)

- Absent / rejected / dropped shadowmask → fully lit (graceful degradation).
- Static→static world shadowing stays **exactly zero**: the pool-shadow union
  subtraction dead-zones on world surfaces (double-count invariant). The format change
  alters mask *lookup*, not the dead-zone logic — must be verified unaffected.
- World-specular multiply and promoted-union crossfade are independent; no receiver
  sums a light twice.

## build_pipeline.md id 42 (line 240) — statement to revise at promotion

"When usable `EntityShadowLights` are emitted; per-selected-light baked
world-visibility masks packed into RGBA channels, with `0xFF` channel entries for
globally dropped masks." Becomes: masks packed into `(block, channel)` slots across
`layer_count × block_count` array layers; drop only when the device array-layer budget
is exceeded.
