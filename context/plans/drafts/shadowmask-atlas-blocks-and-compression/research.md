# Shadowmask Atlas — Blocks and Compression — Research

Grounding for `index.md`. Read at `a097035`. Facts cited by symbol; the brief holds the
decisions, this file holds the investigation. No line numbers (they stale). Merges the
two source drafts' research (`shadowmask-no-drop-atlas`, `shadowmask-compress-at-rest`);
the two-drafts-coordination question is resolved — this is one id-42 format change.

## The id-42 surface today

`ShadowmaskAtlasSection` (`crates/level-format/src/shadowmask_atlas.rs`):

- Header (width, height, layer_count, selected_light_count) + `channels: Vec<u8>` (1
  byte/selected light, 0..3 = RGBA channel, `SHADOWMASK_CHANNEL_DROPPED = 0xFF` = globally
  dropped, pad to 4) + `data` = layer-major `Rgba8Unorm` payload. `SHADOWMASK_TEXEL_BYTES
  = 4`, 255 = fully visible.
- `to_bytes` concatenates header + channels + pad + data; `from_bytes` validates
  `width × height × layer_count × 4` and gates channels implicitly. `byte_len` mirrors it.
- **Stored uncompressed.** `pack.rs` plans the section as `PlannedSection::new(...,
  section.byte_len(), || Ok(section.to_bytes()))` with a `bytes.len() == byte_len`
  assertion — **no compression step**, the sole raw baked atlas.
- **Size = the lightmap atlas footprint, independent of light count.** Dims come from the
  shared lightmap atlas (`SharedAtlas` + the lightmap chart packer's `layer_count`);
  `validate_cached_shadowmask_section` rejects a section disagreeing with the shared
  lightmap atlas. Up to 4 overlapping lights pack into one texel's RGBA; >4 drops the
  dimmest (`assign_channels_with_drops` in `shadowmask_bake/assignment.rs`), and a dropped
  light frees **zero** bytes — the buffer stays `vec![255; data_len]`.

## Block dimension (the >4-overlap axis)

- Today's per-texel ceiling is exactly four (the RGBA channels of one texel); the array
  layer is the receiver's `lightmap_layer`, a spatial axis, not a per-light one. >4
  overlapping selected lights force a mask drop and a missing runtime shadow.
- The block dimension stacks in the same `texture_2d_array`
  (`depth_or_array_layers = layer_count × block_count`), so a texel carries `4 ×
  block_count` masks addressed by slot `s = block * 4 + channel`; block headroom is
  `floor(256 / layer_count)`.
- **Budget coupling.** Blocks stack into the same `max_texture_array_layers = 256` pool
  the lightmap array atlas occupies. `static-light-shadowmask-world-receipt` banked a
  "lightmap array-consolidation refactor" as the fallback if a feature needs array-layer
  headroom; this is that feature. A later consolidation that shrinks `layer_count` frees
  block headroom; a finer `_lightmap_density` that grows `layer_count` spends it. This
  brief does not trigger consolidation but names the coupling so whoever lands it weighs
  shadowmask blocks in the layer budget.
- **Frequency is a decision input, not a measured fact.** Per-texel >4 overlap on today's
  content is unmeasured; `static-light-shadowmask-world-receipt` judged it rare enough for
  a compiler warning + global drop, and `stress-warren-lit`'s 157 lights is a map-wide
  count. The block ceiling is a deliberate build-ahead owner decision (materially more
  headroom than four), stated plainly. Task 2 still emits a per-texel overlap histogram
  under `--verbose` — the number exists, it is just not a gate.

## Compression (the bytes-per-texel axis)

- **Basis is the raw-vs-BC-sibling asymmetry, verified in source** — not an unverified
  "~half the file" magnitude (no `.prl` was measured this session). `crates/level-format/
  src/lib.rs` doc-notes `DirectShVolume = 35` "Stored BC6H-compressed at rest"; id 22
  lightmap likewise (`build_pipeline.md` §PRL, id 22: irradiance BC6H at rest). id 42 is
  the lone raw atlas — the asymmetry the brief closes.
- **BC7 is a drop-in encoding swap; BC4 is not.** `forward.wgsl` binds
  `@group(4) @binding(6) var shadowmask_atlas: texture_2d_array<f32>;`, sampled by
  `sample_shadowmask_atlas` via `textureSample`; `shadowmask_channel_value` selects
  `mask.r/g/b/a` by the channel index (≥ sentinel → 1.0 fully lit). Each RGBA channel is an
  **independent** per-light `[0,1]` visibility scalar, never a correlated color tuple. BC7
  unorm decodes to `f32 [0,1]` under the same binding and `textureSample`, so the shader is
  unchanged for BC7. Per-channel BC4 (single-channel, high fidelity) splits the atlas into
  4 planes, changing the binding/layout and the channel selection — hence the fallback.
  Both lossy; specular-only + fully-lit-fallback semantics bound the visible cost.
- **No new device feature or alignment.** BC6H at rest for id 22/35 means
  `TEXTURE_COMPRESSION_BC` is already a required adapter feature; BC7 is in the same wgpu
  feature. BC 4×4 alignment already holds — the shadowmask shares the lightmap dims and id
  22 is BC6H, so those dims are BC-aligned.

## BC encode seam (grounded: pattern candidate, not a drop-in)

- **In-tree encoders:** `encode_bc6h_rgb_from_f32_rgba` (`crates/level-compiler/src/
  bc6h.rs`, BC6H Mode 11, single-subset non-delta) and `bc5.rs` (BC5 normals). Both are
  dependency-free, min/max-endpoint, order-deterministic (per the lean northstar). The
  emit-side wrapper `encode_direct_section_bc6h` (`crates/level-compiler/src/
  direct_sh_bake.rs`) pads each atlas axis up to a 4×4 multiple (`bc6h_padded_atlas_
  dimensions`), decodes the lossless RGBA16F section into the padded buffer, encodes each
  layer, and concatenates per-layer blocks — the pad-and-concat emit shape a compressed id
  42 mirrors.
- **Why the brief BUILDS the encoder rather than retreating:** BC6H is an **HDR RGB,
  f16-internal** codec; decode is `output_f16 = (interp * 31) >> 6`, and it drops alpha — it
  does **not** produce unorm `[0,1]`, so it is a *pattern to mirror*, not the function. No
  BC7/BC4-unorm encoder is in-tree today. Rather than fall back to a weaker codec to dodge
  that, the brief builds a deterministic BC7-unorm encoder as a shared foundation (mirroring
  the dependency-free, pad-and-concat `bc5.rs`/`bc6h.rs` pattern) with the shadowmask as first
  consumer. This is the "lay the foundation, ship its first consumer in the same unit" doctrine:
  `bc7-color-textures` (draft, stub) names the *same* deterministic BC7 encoder as its heaviest
  task (Task 2) and top risk, and is itself blocked on `emissive-surfaces-bloom` — so the
  shadowmask (no such dependency, graceful fallback, GPU-free testable) is the ideal proving
  ground, and landing the encoder here retires that draft's top risk. BC7 is heavier than BC5
  (8 modes, partition search); a mode subset meeting the fidelity bound is acceptable for v1,
  the hard requirement being cross-platform reproducibility. Per-channel BC4 (one channel of
  the `bc5.rs` unorm path) is the measured fidelity floor if BC7's cross-channel error on
  independent masks fails the visual gate.
- **Determinism (split to match the invariant).** `build_pipeline.md` §Build Cache keys the
  `"shadowmask_atlas"` memo on inputs, not outputs, and its **Determinism invariant** exempts
  lossy compressed output (BC6H irradiance) from byte-identity; `sh-base-atlas-at-rest-slimming`
  sets the posture (exact/raw stage byte-identical, BC path section-length-stable only). id 42
  takes that exemption: the `(block, channel)` slot assignment must be order-deterministic so
  the *logical* pre-compression atlas re-bakes byte-identically (what the source briefs' "byte
  stable" AC actually required), but the lossy BC bytes need only be section-length-stable — no
  runtime or cache path compares two independently produced id-42 blobs for equality. Byte-
  identity of the BC bytes was an over-tight constraint in the first merged draft that
  manufactured a false determinism objection to BC7; corrected. The encoder is still a pinned
  version for hygiene.

## Runtime lifecycle (the double residency, the CPU-free)

- **Whole-atlas resident in RAM for the level lifetime.** `from_bytes` copies the payload
  into an owned `Vec<u8>`; `load_prl` (`crates/level-loader/src/prl_loader.rs`) stores it in
  `LevelWorld.shadowmask_atlas` (`crates/level-loader/src/prl.rs`), accepted only if dims
  match the lightmap irradiance atlas. `startup/lifecycle.rs` stashes the world via
  `self.level = Some(world)`, so `.data` stays resident.
- **Whole-atlas resident in VRAM.** `upload_shadowmask_texture` (`crates/renderer/src/
  lighting/lightmap.rs`) does one `create_texture_with_data`: `Rgba8Unorm`,
  `depth_or_array_layers = layer_count`, `mip_level_count: 1`, LayerMajor,
  `TEXTURE_BINDING | COPY_DST`; view D2Array, bound at group 4. No mips, no streaming.
- **Renderer keeps only `channels`.** Renderer init clones `section.channels`; `.data` is
  never copied there — but the `LevelWorld` source is not freed. Freeing `.data` after
  upload removes the RAM half at zero quality cost; the executor confirms no post-upload
  `.data` reader; level reload re-reads a fresh world (not a reuse consumer).

## Shader consumption and the runtime linchpin

- The slot crosses into the runtime via the `SpecLight` shadowmask field and the promoted
  record's `meta1.z`, written by `build_spec_light_shadowmask_channels` /
  `pack_forward_shadowmask_metadata` as `slot as f32` (sentinel preserved). `forward.wgsl`
  decodes `block = slot / 4`, `channel = slot % 4`, samples array layer `lightmap_layer +
  block × layer_count` via a generalized `sample_shadowmask_atlas`, selects the channel via
  `shadowmask_channel_value`, in both the world-specular and promoted-union paths.
- BC7 leaves this untouched (same `texture_2d_array<f32>` sample). BC4-per-channel would
  make `channel` a plane index rather than an RGBA component — this is where the codec axis
  and the block axis interact (see brief Task 3 / Boundary).

## Prior commitments preserved

- `rendering_pipeline.md` §4 World specular shadowmask: "absent, rejected, or dropped
  shadowmask data is fully lit, and this world-only signal remains independent of
  pool-shadow promotion and its crossfade." Preserved and extended to over-budget and
  no-BC-adapter causes.
- Static→static world shadowing stays exactly zero via the pool-shadow union-subtraction
  dead-zone (double-count invariant). The slot/codec generalization changes mask *location*
  and *encoding*, not the union term.
- `build_pipeline.md` id-42 line ("packed into RGBA channels, with 0xFF ... for globally
  dropped masks") is revised at promotion: masks in `(block, channel)` slots, drop only
  past the device layer budget, payload BC-compressed at rest.

## Streaming — why a separate epic, not this brief

- **Nothing streams today.** No sparse/virtual-texture/tile-pool/LRU/mip-streaming
  machinery; lightmap, direct-SH, and shadowmask all upload whole and stay resident. In this
  portal engine "occlusion culling" of resident data and "streaming" are the same lever, both
  riding the per-frame visible-cell signal (`determine_visible_cells` → `VisibleCells::Culled`
  → `ComputeCull::write_bitmask_from_cells`).
- **The substrate is in-flight and general.** `sh-probe-streaming` (in-progress) builds a
  cluster-of-cells residency substrate (resource-agnostic key = cell adjacency + byte
  budget, LRU eviction, prefetch/hysteresis), names lightmap-layer / shadowmask as future
  subscribers but wires only SH; id 49 (cluster directory) reserved, unemitted.
- **The shadowmask is co-keyed with the lightmap** (same `lightmap_uv` + `lightmap_layer`,
  baked per-vertex `lightmap_layer: u16` in `crates/level-format/src/geometry.rs`), so its
  streamability couples to the lightmap's; the natural streamed unit is "cell-keyed baked
  atlas data (lightmap + shadowmask) together." No code assembles the visible-layer set
  today. Material/texture residency (material+mip key) is a different domain, excluded from
  the spatial substrate.
- Compression composes with streaming (bytes-per-resident-texel vs which texels resident)
  and does not foreclose it: re-baking into cluster-addressable compressed form is cheap
  (pre-stable, no external `.prl` consumers per `development_guide.md` §1.6).

## Prior-art / collision map

- **No plan owns id-42 size/RAM/VRAM reduction** — open gap this brief closes.
- The two source drafts (`shadowmask-no-drop-atlas`, `shadowmask-compress-at-rest`) are
  **merged here** into one id-42 format change; the coordination open question they each
  carried ("how do the two share the header") is resolved by this merge and removed. The
  two axes are orthogonal (blocks add layers, compression changes texel bytes) and share the
  header, `from_bytes` cross-check, and upload/filter path.
- `lighting-scale--shadowmask-cold-working-set` (`ready/`) restructures the assignment seam
  (deletes the per-(light,texel) membership record, derives the overlap graph analytically);
  its output is byte-identical, compile-time RAM only, with the explicit non-goal "bounding
  the output below its on-disk size — a format question." Confirms the gap is open, not
  owned; **re-anchor against it before building** (brief §Re-anchor).
- `lighting-scale--sh-base-atlas-at-rest-slimming` (done) — BC6H-at-rest precedent for id
  34/35; the BC-at-rest discipline generalizes.
- `shadowmask-array-atlas` (done) — closed "no action"; id 42 is already a
  `texture_2d_array` within device limits.
- `bc7-color-textures` (draft, stub) — the only prior mention of BC7 in the compiler; it
  targets `.prm` color slots and leaves the BC7 encoder unresolved. Confirms no in-tree BC7
  encoder exists yet.

## Proof shape (resource-bound + capacity; `testing_guide.md` §Resource bounds)

Proof covers both axes across the full lifetime (`development_guide.md` §1.4): production
(slot assignment + BC encode), serialization (`to_bytes`/`from_bytes` round-trip + codec
length check + slot-range rejection), persistence (on-disk byte delta by codec ratio), the
GPU return (VRAM estimate delta, upload format, >4-overlap renders all masks), cleanup (CPU
`.data` freed + reload). The fidelity/visual pair is the codec gate; the graceful-degradation
pair (over-budget, no-BC-adapter) and the deterministic-slot / length-stable re-bake close the
invariants. Focused fixtures only — no `stress-warren*` bake (ratio is codec-intrinsic, error
texel-local, and a small fixture can force >4 overlap). Tests are `cargo test`, no GPU
context; the BC upload and visual A/B are the thin GPU layer verified by running the engine /
offscreen capture (`capture_frame_indirect`).
