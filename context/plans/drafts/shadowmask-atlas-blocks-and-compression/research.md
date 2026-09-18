# Shadowmask Atlas — Planes and BC4 Compression — Research

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

## Plane dimension (the >4-overlap axis)

- Today's per-texel ceiling is exactly four (the RGBA channels of one texel); the array
  layer is the receiver's `lightmap_layer`, a spatial axis, not a per-light one. >4
  overlapping selected lights force a mask drop and a missing runtime shadow.
- The masks are **independent single-channel scalars**, so the natural representation is a
  stack of single-channel **planes** — one mask per plane — not RGBA packing. The plane
  dimension stacks in the same `texture_2d_array`
  (`depth_or_array_layers = layer_count × plane_count`); a texel carries `plane_count` masks
  addressed by a flat plane index, and the mask for plane *p* is at array layer
  `lightmap_layer + p × layer_count`. This collapses the old `(block, channel)` two-level
  slot to one plane index and deletes the shader's RGBA channel select.
- **Plane headroom is `floor(256 / layer_count)` — a quarter of an RGBA-packed block
  layout's.** Single-channel planes spend the `max_texture_array_layers = 256` budget four
  times faster than RGBA (one mask per layer, not four). Accepted: still materially more
  than four on realistic layer counts, >4 overlap is rare/unmeasured build-ahead, and the
  single-index single-sample layout is simpler than four bindings. Escape hatch if a real
  map binds it: a channel-grouped layout (four BC4 planes per group) preserves RGBA-era
  density at four bindings — noted in the brief Path, not built.
- **Budget coupling.** Planes stack into the same `max_texture_array_layers = 256` pool the
  lightmap array atlas occupies. `static-light-shadowmask-world-receipt` banked a "lightmap
  array-consolidation refactor" as the fallback if a feature needs array-layer headroom;
  this is that feature (and it spends the budget faster than the RGBA design it replaces). A
  later consolidation that shrinks `layer_count` frees plane headroom; a finer
  `_lightmap_density` that grows `layer_count` spends it. This brief does not trigger
  consolidation but names the coupling so whoever lands it weighs shadowmask planes in the
  layer budget.
- **Frequency is a decision input, not a measured fact.** Per-texel >4 overlap on today's
  content is unmeasured; `static-light-shadowmask-world-receipt` judged it rare enough for a
  compiler warning + global drop, and `stress-warren-lit`'s 157 lights is a map-wide count.
  The plane ceiling is a deliberate build-ahead owner decision (materially more headroom than
  four), stated plainly. Task 2 still emits a per-texel overlap histogram under `--verbose`.
- **Assignment simplifies under abundant planes.** Today's `assign_channels_with_drops` is a
  two-phase exact-search + greedy-fallback: `color_graph_exact_bounded` searches for a 4-colouring
  bounded by `SHADOWMASK_COLOR_SEARCH_NODE_BUDGET` (`assignment.rs:322`, `while frame.next_channel
  < 4`), and on `BudgetExhausted` (`:163-185`) falls to `color_graph_priority_greedy` (`:381`,
  `used = [false; 4]`), which can **drop a mask on search-budget grounds** (`:391-395`), not the
  device ceiling — the search-budget drop gap. That machinery exists because 4 colours is scarce.
  Under planes grown on demand, colours are abundant, so a deterministic greedy first-fit (stable
  selection order, lowest free plane, Δ+1 planes max) is complete and drops only at
  `plane_count × layer_count = max_texture_array_layers`. The brief retires the exact search and
  the fallback: the no-drop-below-budget invariant becomes true by construction, the two
  4-hardcoded loops (`:322`, `:381`) collapse to one, and a search-budget nondeterminism source is
  removed. Cost: greedy uses ≥ χ planes (a few more than optimal near the ceiling) — accepted.

## Compression (the bytes-per-texel axis) — why BC4, not BC7

- **Basis is the raw-vs-BC-sibling asymmetry, verified in source** — not an unverified
  "~half the file" magnitude (no `.prl` was measured this session). `crates/level-format/
  src/lib.rs` doc-notes `DirectShVolume = 35` "Stored BC6H-compressed at rest"; id 22
  lightmap likewise (`build_pipeline.md` §PRL, id 22: irradiance BC6H at rest). id 42 is the
  lone raw atlas — the asymmetry the brief closes.
- **The data selects the codec.** `forward.wgsl` binds
  `@group(4) @binding(6) var shadowmask_atlas: texture_2d_array<f32>;`, sampled by
  `sample_shadowmask_atlas`; `shadowmask_channel_value` selects `mask.r/g/b/a` by channel
  index (≥ sentinel → 1.0 fully lit). Each channel is an **independent** per-light `[0,1]`
  visibility scalar, spatially smooth, never a correlated colour tuple. That is exactly BC4's
  home case (two min/max endpoints + 3-bit indices per 4×4 block, single channel). BC7 was
  rejected: it models a cross-channel block correlation the masks do not have (its weak
  case), and at 8 bpp for four channels it gives ~2 bits/channel vs BC4's 4 — structurally
  wrong *and* lower per-channel precision. Its only edge (≈4:1 vs BC4's ≈2:1) is bought with
  that mismatch, and the only full-res 4:1 alternative (halving spatial resolution) is a
  different, out-of-scope lever. So ≈2:1 via BC4 is the correct ceiling for the compression
  lever, banked without a fidelity gate. The single-channel plane restructure makes the
  shader read one array layer's `.r` (the `shadowmask_channel_value` RGBA select disappears).
- **No new device feature or alignment.** BC6H at rest for id 22/35 means
  `TEXTURE_COMPRESSION_BC` is already a required adapter feature; BC4 is in the same wgpu
  feature. BC 4×4 alignment already holds — the shadowmask shares the lightmap dims and id 22
  is BC6H, so those dims are BC-aligned.

## BC4 encode seam (reuse, no new encoder)

- **In-tree encoders:** `bc5.rs` (BC5 normals — two BC4 channels) and
  `encode_bc6h_rgb_from_f32_rgba` (`crates/level-compiler/src/bc6h.rs`, BC6H Mode 11). Both
  dependency-free, min/max-endpoint, order-deterministic. The emit-side wrapper
  `encode_direct_section_bc6h` (`crates/level-compiler/src/direct_sh_bake.rs`) pads each
  atlas axis up to a 4×4 multiple (`bc6h_padded_atlas_dimensions`), encodes each layer, and
  concatenates per-layer blocks — the pad-and-concat emit shape a compressed id 42 mirrors.
- **BC4 = one channel of the existing BC5 path**, unorm, deterministic — so the shadowmask
  reuses in-tree machinery and lands self-contained, with **no encoder to build**. Path
  grounds that the `bc5.rs` path exposes (or trivially yields) a single-channel BC4 block and
  that its precision holds a smooth `[0,1]` mask within the fidelity bound.
- **BC7 is not this brief's to build.** No BC7-unorm encoder exists in-tree, and
  `bc7-color-textures` (draft, stub) owns BC7 — the right tool for correlated sRGB colour,
  with its own mip chain, magnification aesthetic gate, and `emissive-surfaces-bloom`
  dependency. Forcing BC7 onto the shadowmask (BC7's weak, uncorrelated-channel case, no
  mips) to serve as a proving ground for the colour path was the rejected over-reach: the
  proof would not transfer where it matters, and it would make the shadowmask worse to
  benefit a different, blocked feature.
- **Determinism.** `build_pipeline.md` §Build Cache keys the `"shadowmask_atlas"` memo on
  inputs, not outputs, and its determinism invariant exempts lossy compressed output (BC6H
  irradiance) from byte-identity; `sh-base-atlas-at-rest-slimming` sets the posture (exact/raw
  stage byte-identical, BC path section-length-stable only). The plane assignment must be
  order-deterministic so the logical pre-compression atlas re-bakes byte-identically; the BC4
  encode is deterministic by construction (the `bc5.rs` min/max path), so the section re-bakes
  byte-identically in practice, but only section-length stability is a hard requirement.

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
- **Renderer keeps only the plane/channel table.** Renderer init clones `section.channels`;
  `.data` is never copied there — but the `LevelWorld` source is not freed. Freeing `.data`
  after upload removes the RAM half at zero quality cost; the executor confirms no post-upload
  `.data` reader; level reload re-reads a fresh world (not a reuse consumer).

## Shader consumption and the runtime linchpin

- The slot crosses into the runtime via the `SpecLight` shadowmask field and the promoted
  record's `meta1.z`, written by `build_spec_light_shadowmask_channels` /
  `pack_forward_shadowmask_metadata` as a float (sentinel preserved). Today `forward.wgsl`
  decodes an RGBA channel index and selects `mask.r/g/b/a` via `shadowmask_channel_value`.
- Under the plane restructure the field carries a **plane index**; `forward.wgsl` samples
  array layer `lightmap_layer + plane × layer_count` via `sample_shadowmask_atlas` and reads
  `.r`, in both the world-specular and promoted-union paths — `shadowmask_channel_value` is
  deleted. BC4 decodes to `.r` under the same `texture_2d_array<f32>` binding, so the sample
  is unchanged by the codec once the atlas is single-channel.

## Prior commitments preserved

- `rendering_pipeline.md` §4 World specular shadowmask: "absent, rejected, or dropped
  shadowmask data is fully lit, and this world-only signal remains independent of pool-shadow
  promotion and its crossfade." Preserved and extended to over-budget and no-BC-adapter causes.
- Static→static world shadowing stays exactly zero via the pool-shadow union-subtraction
  dead-zone (double-count invariant). The plane/codec generalization changes mask *location*
  and *encoding*, not the union term.
- `build_pipeline.md` id-42 line ("packed into RGBA channels, with 0xFF ... for globally
  dropped masks") is revised at promotion: masks in single-channel planes addressed by a plane
  index, drop only past the device layer budget, payload BC4-compressed at rest.

## Streaming — why a separate epic, not this brief

- **Nothing streams today.** No sparse/virtual-texture/tile-pool/LRU/mip-streaming
  machinery; lightmap, direct-SH, and shadowmask all upload whole and stay resident. In this
  portal engine "occlusion culling" of resident data and "streaming" are the same lever, both
  riding the per-frame visible-cell signal (`determine_visible_cells` → `VisibleCells::Culled`
  → `ComputeCull::write_bitmask_from_cells`).
- **The substrate is in-flight and general.** `sh-probe-streaming` (in-progress) builds a
  cluster-of-cells residency substrate (resource-agnostic key = cell adjacency + byte budget,
  LRU eviction, prefetch/hysteresis), names lightmap-layer / shadowmask as future subscribers
  but wires only SH; id 49 (cluster directory) reserved, unemitted.
- **The shadowmask is co-keyed with the lightmap** (same `lightmap_uv` + `lightmap_layer`,
  baked per-vertex `lightmap_layer: u16` in `crates/level-format/src/geometry.rs`), so its
  streamability couples to the lightmap's; the natural streamed unit is "cell-keyed baked
  atlas data (lightmap + shadowmask) together." No code assembles the visible-layer set today.
  Material/texture residency (material+mip key) is a different domain, excluded from the
  spatial substrate.
- Compression composes with streaming (bytes-per-resident-texel vs which texels resident) and
  does not foreclose it: re-baking into cluster-addressable compressed form is cheap
  (pre-stable, no external `.prl` consumers per `development_guide.md` §1.6).

## Prior-art / collision map

- **No plan owns id-42 size/RAM/VRAM reduction** — open gap this brief closes.
- The two source drafts (`shadowmask-no-drop-atlas`, `shadowmask-compress-at-rest`) are
  **merged here** into one id-42 format change; the coordination open question they each
  carried is resolved by the merge and removed.
- `lighting-scale--shadowmask-cold-working-set` (landed, `done/`) restructured the assignment
  seam (deletes the per-(light,texel) membership record, derives the overlap graph
  analytically); compile-time RAM only, byte-identical output, with the explicit non-goal
  "bounding the output below its on-disk size — a format question." Confirms the gap is open,
  not owned; **re-anchor against it before building** (brief §Re-anchor).
- `lighting-scale--sh-base-atlas-at-rest-slimming` (done) — BC-at-rest precedent for id 34/35;
  the BC-at-rest discipline and section-length-stable posture generalize.
- `shadowmask-array-atlas` (done) — closed "no action"; id 42 is already a `texture_2d_array`
  within device limits.
- `bc7-color-textures` (draft, stub) — owns BC7 for `.prm` colour slots; blocked on
  `emissive-surfaces-bloom` and an aesthetic A/B veto. Out of this brief's scope; not a
  dependency in either direction.

## Proof shape (resource-bound + capacity; `testing_guide.md` §Resource bounds)

Proof covers both axes across the full lifetime (`development_guide.md` §1.4): production
(plane assignment + BC4 encode), serialization (`to_bytes`/`from_bytes` round-trip + codec
length check + plane-range rejection), persistence (on-disk byte delta ≈2:1), the GPU return
(VRAM estimate delta, upload format, >4-overlap renders all masks), cleanup (CPU `.data` freed
+ reload). BC4 needs no codec-selection gate; a measured error report plus a visual regression
check confirm it. The graceful-degradation pair (over-budget, no-BC-adapter) and the
deterministic-plane / length-stable re-bake close the invariants. Focused fixtures only — no
`stress-warren*` bake (ratio is codec-intrinsic, error texel-local, and a small fixture can
force >4 overlap). Tests are `cargo test`, no GPU context; the BC4 upload and visual A/B are the
thin GPU layer verified by running the engine / offscreen capture (`capture_frame_indirect`).

## Acceptance pin table

Orderings/edges pinned by `/review-brief`, each cited by an Acceptance row.

| id | scenario | ordering / mechanism | expected outcome |
|---|---|---|---|
| collision-free-planes | ≥2 selected lights share a lightmap texel | assignment writes each light's plane index into the per-selection table before payload fill | no overlap edge has both endpoints on the same non-sentinel plane |
| budget-boundary | product hits the array-layer ceiling exactly | renderer filter evaluates `layer_count × plane_count` vs max before texture creation | `==256` kept, `==257` degrades to placeholder; no off-by-one device breach |
| format-edges | single-plane / empty-selection / each codec tag | header encodes `plane_count` and codec tag; `from_bytes` recomputes payload length from the tagged block size | `plane_count ∈ {0,1}` and both tags round-trip byte-exact |
| dropped-and-placeholder-fully-lit | sentinel plane index, or `plane>0` index against the 1-layer placeholder | shader clamps computed array layer, then applies the per-path sentinel guard | both paths return fully-lit; no sample outside the bound texture in either |
