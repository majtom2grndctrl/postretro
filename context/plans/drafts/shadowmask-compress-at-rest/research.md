# shadowmask-compress-at-rest — Research

Grounding for `index.md`. Read at `a097035`. Facts cited by symbol; the brief holds the
decisions, this file holds the investigation. No line numbers (they stale).

## Basis: is id 42 really the large, reducible section?

The developer's premise — the shadowmask atlas is ~half of a 2.39 GB warren `.prl` — was
not measured against a shipped file this session (no `.prl` inspected), but the mechanism
makes "large, comparable to the lightmap footprint" the expected outcome, and confirms it
is reducible:

- **Format is raw `Rgba8Unorm`, 4 bytes/texel.** `ShadowmaskAtlasSection` in
  `crates/level-format/src/shadowmask_atlas.rs`: header (width, height, layer_count,
  selected_light_count) + `channels: Vec<u8>` (1 byte/light, pad to 4) + `data` = layer-major
  `Rgba8Unorm` payload; `SHADOWMASK_TEXEL_BYTES = 4`; `from_bytes` validates
  `width × height × layer_count × 4`.
- **Stored uncompressed.** `pack.rs` plans the section as `PlannedSection::new(...,
  section.byte_len(), || Ok(section.to_bytes()))` and writes the bytes with a
  `bytes.len() == descriptor.byte_len` assertion — no compression step. `to_bytes` just
  concatenates header + channels + pad + data.
- **Size = the lightmap atlas footprint, independent of light count.** `ShadowmaskFill::new`
  (`shadowmask_bake/fill.rs`) allocates `plane × layer_count × 4`; dims come from
  `SharedAtlas { atlas_width, atlas_height }` + `layer_count_from_shared` (the lightmap
  chart packer's layer count, `lightmap_layer.rs` / `lightmap_bake::prepare_atlas`), and
  `validate_cached_shadowmask_section` rejects any section disagreeing with the shared
  lightmap atlas. Up to 4 overlapping lights pack into one texel's RGBA; >4 drops the dimmest
  (`assign_channels_with_drops` in `shadowmask_bake/assignment.rs`) but a dropped light frees
  **zero** bytes — the buffer stays `vec![255; data_len]`.
- **Siblings already compressed at rest.** `crates/level-format/src/lib.rs` doc-notes
  `DirectShVolume = 35` "Stored BC6H-compressed at rest"; id 22 lightmap likewise. id 42 is
  the lone raw atlas — the asymmetry the brief closes.

## Runtime lifecycle (the double residency)

- **Whole-atlas resident in RAM for the level lifetime.** `ShadowmaskAtlasSection::from_bytes`
  copies the payload into an owned `Vec<u8>`; `load_prl` (`crates/level-loader/src/prl_loader.rs`)
  stores it in `LevelWorld.shadowmask_atlas` (`crates/level-loader/src/prl.rs`), accepted only
  if dims match the lightmap irradiance atlas. `startup/lifecycle.rs` stashes the world via
  `self.level = Some(world)` after `install_level_geometry`, so `.data` stays resident.
- **Whole-atlas resident in VRAM.** `upload_shadowmask_texture`
  (`crates/renderer/src/lighting/lightmap.rs`) does one `create_texture_with_data`:
  `Rgba8Unorm`, `depth_or_array_layers = layer_count`, `mip_level_count: 1`, `LayerMajor`,
  `TEXTURE_BINDING | COPY_DST`. View is `D2Array`, bound at group 4 `BIND_SHADOWMASK_ATLAS`.
  No mips, no streaming, no partial residency.
- **Renderer keeps only `channels`.** `renderer_full_init.rs` / `renderer_resources.rs` clone
  `section.channels` into renderer state; `.data` is never copied there — but the source in
  `LevelWorld` is not freed. Net: the full payload exists concurrently in RAM and VRAM.
  Freeing `.data` after upload removes the RAM half at zero quality cost.

## Shader consumption (why BC7 is a drop-in, BC4 is not)

- `@group(4) @binding(6) var shadowmask_atlas: texture_2d_array<f32>;` (`forward.wgsl`),
  sampled by `sample_shadowmask_atlas` via `textureSample` (linear-filtered, layer clamped).
- `shadowmask_visibility_for_spec_light` reads a channel index (0..3, or ≥ dropped sentinel →
  1.0 fully lit) from `sl.cone_cos.z`, then `shadowmask_channel_value` selects `mask.r/g/b/a`.
  Each RGBA channel is an **independent** per-light scalar visibility mask in `[0,1]`, used as
  a specular multiplier — never read as a correlated color tuple.
- **BC7 unorm** decodes to `f32` `[0,1]` and is sampled by the same `texture_2d_array<f32>`
  binding and the same `textureSample`, so the shader is unchanged: a pure encoding swap on
  the compiler/upload sides. **Per-channel BC4** (single-channel, high fidelity) would split
  the atlas into 4 planes, changing the binding/layout and the shader's channel selection —
  hence the fallback, not the default. Both are lossy; the specular-only, fully-lit-fallback
  semantics bound the visible cost.
- **Device feature:** BC6H at rest for id 22/35 means `TEXTURE_COMPRESSION_BC` is already a
  required adapter feature; BC7 is in the same wgpu feature, so no new requirement. BC 4×4
  block alignment is already satisfied — the shadowmask shares the lightmap dims and id 22 is
  BC6H, so those dims are BC-aligned.

## Streaming / occlusion culling — why it is a separate epic, not this brief

(Deferred; recorded so the boundary is legible.)

- **Nothing streams today.** No sparse/virtual-texture/tile-pool/LRU/mip-streaming machinery
  exists in the renderer; lightmap, direct-SH, and shadowmask all upload whole and stay
  resident. In this portal engine "occlusion culling" of resident data and "streaming" are the
  same lever — both would ride the per-frame visible-cell signal (`determine_visible_cells` →
  `VisibleCells::Culled` → bitmask via `ComputeCull::write_bitmask_from_cells`).
- **The substrate is in-flight and general.** `sh-probe-streaming` (in-progress) builds a
  cluster-of-cells residency substrate (resource-agnostic key = cell adjacency + byte budget,
  LRU eviction, prefetch/hysteresis) and explicitly names lightmap-layer / shadowmask as future
  subscribers ("Generalization door") but wires only SH this epic; id 49 (cluster directory) is
  reserved, unemitted.
- **The shadowmask is co-keyed with the lightmap** (same `lightmap_uv` + `lightmap_layer`, baked
  per-vertex `lightmap_layer: u16` in `crates/level-format/src/geometry.rs`), so its
  streamability is coupled to the lightmap's — the natural streamed unit is "cell-keyed baked
  atlas data (lightmap + shadowmask) together." The visible-layer set is derivable
  (visible-cell → `CellDrawIndex` CSR id 37 → BVH leaves → per-vertex `lightmap_layer`) but no
  code assembles it today.
- **Material/texture residency is a different domain** (material+mip key), explicitly excluded
  from the spatial substrate. An umbrella streaming epic could cover both but they share neither
  key nor planner.
- Compression composes with streaming (bytes-per-resident-texel vs which texels resident) and
  does not foreclose it: re-baking into cluster-addressable compressed form is cheap
  (pre-stable, no external consumers).

## Prior-art / collision map

- **No plan owns id-42 size/RAM/VRAM reduction** — open gap.
- `shadowmask-no-drop-atlas` (draft) adds a `block_count` header field and extends the payload
  to `layer_count × block_count` layers to lift the >4-overlap drop ceiling. Orthogonal to
  compression but shares the id-42 header, `from_bytes` cross-check, and upload/filter path →
  version jointly (brief Decisions). It also revises the `build_pipeline.md` id-42 doc line at
  promotion; a compression tag note composes there.
- `lighting-scale--shadowmask-cold-working-set` (done) — compile-time RAM only, output
  byte-identical; explicit non-goal "Bounding the output below its on-disk size — a format
  question." Confirms the gap is open, not owned.
- `lighting-scale--sh-base-atlas-at-rest-slimming` (done) — the at-rest BC6H + valid-only
  compaction precedent for id 34/35. The BC-at-rest discipline generalizes; the valid-probe
  drop-at-rest is SH-grid-specific and out of scope here.
- `shadowmask-array-atlas` (done) — closed "no action"; confirms id 42 is already a
  `texture_2d_array` within device limits.

## Proof shape (resource-bound; `testing_guide.md` §Resource bounds)

Resource proof covers the full lifetime (`development_guide.md` §1.4): production (bake encode),
serialization (`to_bytes`/`from_bytes` round-trip + length check), persistence (on-disk byte
delta), the GPU return (VRAM estimate delta, upload format), and cleanup (CPU `.data` freed).
The fidelity/visual pair is the codec gate. Focused fixtures only — no `stress-warren*` bake
(ratio is codec-intrinsic, error texel-local). Tests are `cargo test`, no GPU context; the BC
upload and visual A/B are the thin GPU layer verified by running the engine / offscreen capture.
