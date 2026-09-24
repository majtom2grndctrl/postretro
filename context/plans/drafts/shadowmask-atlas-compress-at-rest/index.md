# Shadowmask Atlas — BC5 Compress-at-Rest

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4,
`context/lib/build_pipeline.md` §PRL · §Build Cache, `context/lib/development_guide.md`
§1.4, `context/lib/testing_guide.md` §Resource bounds · read at 8e4753ce1

This brief changes bytes, never masks. Per-texel mask capacity is
`shadowmask-atlas-mask-capacity`, gated on the overlap measurement this brief ships.

## Problem

An observed cost on the performance yardstick. On `stress-warren-hallway-inspection`
(84 lightmap layers at 512²), PRL id 42 (`ShadowmaskAtlasSection`) is 88 MB: the
largest section in the level and the only baked atlas still stored raw. No bake or emit
stage compresses it, so the `Rgba8Unorm` payload sits uncompressed on disk, in VRAM,
and in CPU RAM for the whole level lifetime. Id 22's irradiance and direction payloads
(≈66 MB fresh) share that last residency: nothing reads them after upload, yet the
loaded world holds them until unload. `lighting-scale--shadowmask-cold-working-set`
already recorded id 42 at 37.7 MB on the warren, and 1.29 GB at density 0.04. The
masks are independent, spatially smooth `[0,1]` visibility scalars — BC4's home case,
and BC5 is two BC4 blocks in one. When this lands, id 42 ships and uploads as BC5 at
half the payload bytes, no GPU-only lightmap or shadowmask payload stays on the CPU
after upload, and every shadow reads as it did.

## Decisions

- **BC5 `.rg`, two groups side by side in each layer — four masks per texel, today's
  number.** The texture is `2W × H × L`: group 0 fills the left half of every layer and
  group 1 the right. Layer count stays the lightmap's `L`, so the array-layer budget,
  the lightmap packer and every other lightmap-shaped section are untouched. No
  capacity claim, so no capacity floor and no widened slot index. Stacking the groups
  in layers `L..2L` is rejected: it halves the levels a two-group atlas can seat and
  spends the layer budget the capacity brief and lightmap-layer streaming both want.
- **The slot table is unchanged.** One byte per selected light, `0..3` or
  `SHADOWMASK_CHANNEL_DROPPED`. Slot `s` now addresses group `s / 2`, channel `s % 2`.
- **BC5 is the only format.** The payload gains a format tag, mirroring
  `LightmapSection::direction_format` / `irradiance_format`; it has one value today and
  the capacity brief adds values additively. Raw `Rgba8Unorm` is retired from the
  wire and the renderer, so there is exactly one decode path. The raw baseline for
  bytes, fidelity and the visual A/B comes from a pre-change build, not a runtime tag.
- **Pre-change payloads are rejected by name, not migrated.** They fail `from_bytes`
  with an error naming the format mismatch and load fully lit through the loader's
  existing malformed-section path. Fixtures re-bake (`development_guide.md` §1.6). The
  container-entry version for id 42 advances, but the in-payload tag is the reject —
  the loader does not check id 42's entry version.
- **Encode in the bake, before the pack seam.** The whole-section memo then stores
  compressed blocks, so a warm build neither re-encodes nor caches raw, and the pack seam
  keeps receiving a finished section. The shadowmask stage version advances.
- **Byte-identical output; the BC6H lossy exemption is not taken.**
  `static-light-shadowmask-cache-addendum` guarantees cached-warm equals uncached for
  this section, and `bc5.rs` is a pure serial function, so the guarantee survives.
- **One sampling helper absorbs the layout.** The atlas read returns the four masks in
  today's shape — `(g0.r, g0.g, g1.r, g1.g)` — from two samples at the same layer, so
  the per-light select and both decode paths (promoted-union subtraction, world
  specular) keep their shape. Each sample clamps the lightmap `u` half a texel inside
  its group's width before mapping it into that half, which reproduces today's
  clamp-to-edge exactly, so bilinear filtering never blends one group into the other.
  Both coordinates derive from the fragment's lightmap UV and layer alone, so the
  samples stay hoisted outside every light loop in uniform control flow. Cost: at most
  one extra sample per active decode path per fragment, independent of light count.
- **A level whose lightmap layers are already 8192 wide gets no shadowmask, loudly.**
  `2W` would exceed the pinned 8192 texture dimension. The bake names the level and the
  width in a warning and omits id 42; the level renders fully lit, as
  `rendering_pipeline.md` §4 commits for absent data. Reaching it takes a single BVH
  leaf of roughly 26,800 m² of lit surface; no observed level is wider than 4096. The
  capacity brief inherits this band.
- **Malformed data degrades to fully lit, never raw.** A `2W` over the device's
  texture dimension or dimensions not 4-aligned resolve to the all-visible placeholder
  with a `[Renderer]` error. The compiler emits neither; the runtime guards corrupt or
  hand-built data. Against the one-texel placeholder both samples read the same white
  texel.
- **The upload owns GPU-only lightmap payloads.** Every level install — the game's and
  the capture harness's — moves id 42's mask payload and id 22's irradiance and
  direction payloads out of the loaded world into the lightmap upload, which drops them once the textures exist. The world keeps each
  section's header — dimensions, formats, the slot table — because atlas-dimension
  resolution and spec-light channel mapping read them, and it has no representation of
  a header whose payload was taken, so nothing can re-validate or re-upload an emptied
  section. An install with no renderer moves nothing. Every load or reload re-reads the
  level from disk.
- **No new device requirement, binding, or padding.** `TEXTURE_COMPRESSION_BC` is
  already required at device acquisition, and `Bc5RgUnorm` is filterable under it.
  Format and width change behind the same group-4 binding, so the forward texture
  inventory is unchanged. Lightmap atlas dimensions are powers of two ≥ 64, so `2W` is
  block-aligned and the seam falls on a block boundary.
- **Compile-time peak is bounded at 1.5× the raw fill plus one layer.** The raw fill,
  its BC5 output and one layer's encode scratch coexist only inside the finish step; the
  raw buffer drops before the section leaves the stage. Runtime residency falls: id 42's
  VRAM halves, and GPU-only payloads leave the CPU after upload.
- **Report peak per-texel overlap under `--verbose`, on every bake.** Not acted on
  here; it is the capacity brief's missing premise. The count comes from the overlap
  graph, which only a memo miss builds, so the memo carries it beside the section and a
  hit reports it too. The shipped section does not carry it — no runtime reader.

### Non-goals

- **Mask capacity above four.** `shadowmask-atlas-mask-capacity`. Unmeasured and
  unreported, so it must not gate a certain win.
- **A runtime raw decode path**, including as an A/B toggle.
- **Compressing id 22's direction plane.** Octahedral RG8 is BC5's other home case, but
  whether direction survives block compression is a shading-fidelity question this brief
  does not answer. `research.md` records the door.
- **BC7.** `bc7-color-textures` owns it; BC7 models cross-channel correlation four
  independent masks lack.
- **Streaming the shadowmask.** `sh-probe-streaming` excludes lightmap residency and its
  cluster directory admits no id 42. Side-by-side groups keep the shadowmask on the
  lightmap's layer grid, the granularity `context/research/spatial-streaming.md` §6
  names for lightmap residency.
- **Lightmap array consolidation.** `shadowmask-atlas-mask-capacity` owns it; only a
  second shadowmask binding needs it.
- **Selection eligibility and ranking.** Unchanged.

## Acceptance

### Automated

**Wire**
- [ ] Header, format tag, slot table and payload round-trip byte-exact for an empty
      selection, a single selected light, and a full four-slot table.
- [ ] `from_bytes` rejects: a payload length that disagrees with 16 bytes per 4×4
      block over a `2W × H` plane per layer across `layer_count` layers; dimensions not
      multiples of 4; an unknown tag; a slot neither `0..3` nor the sentinel.
- [ ] A pre-change raw id-42 payload loads with a warning naming the format mismatch,
      renders fully lit, and does not panic — including a payload whose bytes at the
      tag's position read as a valid tag, which must not fall through to a length
      mismatch. (pin: stale-payload, stale-payload-tag-collision)
- [ ] A warm cache holding a pre-change shadowmask entry never serves it; the rebuilt
      section equals the uncached section byte-for-byte. (pin: stale-memo)
- [ ] A second build with no edits reports a shadowmask memo hit, not a re-bake — so a
      streamed header that drifted from the section's own encoding cannot hide behind
      cached-equals-uncached. (pin: warm-equals-cold)
- [ ] A level whose selected lights are all dropped ships the section at half the raw
      bytes, loads it, and renders fully lit. (pin: all-sentinel)

**Degradation**
- [ ] A sentinel slot reads fully lit. A second-group slot against the one-texel
      placeholder reads fully lit without addressing outside the bound texture.
- [ ] `2W` equal to the device's pinned texture dimension is kept; one block wider
      degrades to the placeholder with a `[Renderer]` error and no panic.
      (pin: width-boundary)
- [ ] A section whose lightmap width is 8192 — texture width 16384 — degrades to the
      placeholder with a `[Renderer]` error and no panic against the pinned 8192 limit;
      4096 is kept. (pin: width-boundary)
- [ ] A bake whose lightmap layers are 8192 wide warns naming the width and emits no
      id 42; the level loads fully lit. A bake at 4096 emits the section. (pin: wide-layer)
- [ ] A warm rebuild of the 8192-wide bake warns again and still emits no id 42.
      (pin: wide-layer-warm)
- [ ] A bake given atlas dimensions that are not multiples of 4 fails with an error
      naming them, in release as in debug; it never emits a truncated payload.
      (pin: bake-misaligned)

**Masks**
- [ ] Four overlapping selected lights, spread across both groups, each resolve to their
      own mask in both decode paths — no drop, no cross-talk between groups. Proven by
      offscreen render on an adapter; a skipped run does not count.
- [ ] A texel on either edge of a group's half reads its own group's mask; a fixture
      whose two groups differ at the seam shows no bleed from the neighbouring group.
      Proven by offscreen render on an adapter; a skipped run does not count.
      (pin: seam-bleed)
- [ ] Samples driven at lightmap `u` = 0 and `u` = 1, for each group, read that group's
      edge column, with neighbouring groups that differ at the seam. Baked fixtures do
      not count: their chart gutter keeps UVs off the edge. (pin: seam-outer-halftexel)
- [ ] Moving a light from the first group to the second leaves world-specular output
      unchanged for surfaces covered only by first-group lights; static→static world
      shadowing stays exactly zero. Proven by offscreen render on an adapter; a skipped
      run does not count.
- [ ] An all-visible (255) atlas decodes fully lit after encode.
- [ ] Grep gate over the forward shader: no shadowmask sample sits inside a light loop,
      each decode path issues its samples once per fragment at one layer, and each
      group's coordinate derives only from the fragment's lightmap UV and the bound
      texture's width.
- [ ] The forward pass binds no new texture or sampler; its per-group sampled-texture
      inventory is unchanged.

**Bytes and determinism**
- [ ] On a populated fixture, the id-42 payload is exactly half the raw arithmetic
      (`2W × H × layer_count × 1` against `W × H × layer_count × 4`) in the per-section
      footprint report.
- [ ] The texture description built for the atlas — BC5, `2W × H`, `layer_count`
      layers — and its byte size, half the raw description's, are checked without a GPU,
      and it is the same description the upload creates.
- [ ] On a cache miss the bake holds one raw fill, one compressed output and at most one
      layer of encode scratch, and the raw fill is gone before the section is cached or
      returned.
- [ ] Re-baking twice yields a byte-identical section across differing worker-thread
      counts, and a cached-warm section equals the uncached one byte-for-byte.
- [ ] Measured and reported, not gated: max and mean per-channel absolute error of the
      encode against the raw masks, recorded in the landing note.

**Lifecycle**
- [ ] After install, the loaded world holds id 42's slot table and dimensions and id 22's
      header, and none of their payloads; each payload's allocation is freed once its
      texture exists.
- [ ] Payloads are released only after their upload; an install that uploads nothing
      keeps them. (pin: no-upload-install)
- [ ] A capture install takes the payloads the same way; no install path borrows them.
      (pin: capture-install)
- [ ] On a level with animated lights, the animated contribution atlas matches the
      static lightmap's dimensions after install, and animated lights render.
      (pin: animated-after-take)
- [ ] A level with a lightmap but no shadowmask, and a level with neither, install
      without panic; the first releases its lightmap payloads. (pin: partial-lighting-install)

**Overlap report**
- [ ] A cold bake under `--verbose` reports peak per-texel overlap with layer count and
      format; a warm memo hit reports the same value; a non-verbose bake gains no line.
      (pin: overlap-memo-hit)

### Manual

On `stress-warren-hallway-inspection`, freshly baked, pre-change commit against
post-change:
- [ ] Id 42 and id 22 section byte counts, layer count and atlas dimensions, and the
      bake's peak RSS — the compile-time bound's evidence — in the landing note.
- [ ] Process RSS after level install, before and after — the CPU release's evidence.
- [ ] Offscreen capture of a view with selected non-SDF static specular lights: highlights
      and their occluded regions read unchanged against the pre-change capture. Look at
      both images; a distribution check passes on an image that lost its contrast.
- [ ] Frame time, measured and reported: capture-harness CPU completion median and p95.
      Pin view, machine class, build profile, worker count and cache mode per
      `testing_guide.md` §Resource bounds. May need an attended run on this Mac.
- [ ] Peak per-texel overlap under `--verbose`, recorded for the capacity brief.
- [ ] After a level reload and a dev level cycle, second-group shadows render on the
      new level. (pin: release-then-reload)
- [ ] A dev level cycle to a level whose lightmap width differs, and back, renders both
      levels' lightmaps and shadowmasks correctly.

## Wire format

`ShadowmaskAtlasSection`, id 42. Little-endian throughout. Section order unchanged.

- **Header** gains a format tag. One value today: BC5 `.rg`, two groups side by side.
  `width` stays the lightmap atlas width `W`; the texture width `2W` is implied by the
  tag. The capacity brief adds a group-count field if it needs one.
- **Slot table** unchanged: `u8` per selected light, padded to 4. Slot `s` → group
  `s / 2` (left or right half), channel `s % 2`, at the texel's own lightmap layer.
- **Payload**: per layer, one `2W × H` BC5 plane in row-major 4×4 blocks, 16 bytes per
  block; layers in order. Group 0's masks occupy block columns `0..W/4`, group 1's
  `W/4..2W/4`.
- **Rejects**: unknown tag, length disagreeing with the tag's arithmetic, dimensions not
  4-aligned, out-of-range slot.

Header slot positions and tag width are implementation choices. Binding constraints:
the tag round-trips, the length check follows the tag, the sentinel keeps its meaning,
and no pre-change payload parses as valid.

## Path

Non-binding.

- **Encode seam.** `ShadowmaskFill::finish` (`shadowmask_bake/fill.rs`) moves raw RGBA
  into the section; `empty_section_for_dimensions` builds sections too and must emit the
  same tagged shape. Per layer, lay RGBA's R,G into the left half and B,A into the right
  half of a `2W × H` RG image and run `bc5::encode_bc5_rg` once at width `2W`. Put the
  encode in its own submodule — `shadowmask_bake.rs` is past 4,000 lines and gains no
  new responsibility here.
- **Cache writer.** `cache_shadowmask_section_then_complete` streams its own copy of the
  header layout and bypasses `to_bytes`. It must learn the tag, and it is where
  warm/cold drift would hide — prefer one header-writing helper both paths share. Bump
  `SHADOWMASK_ATLAS_STAGE_VERSION`; its test pins the value.
- **Wide-layer omit.** The bake knows `W` from the prepared atlas; omit the section
  before the fill runs, so no raw buffer is allocated for a section that will not ship.
- **Pack seam.** `pack/section_plan.rs` `build_finalized_section_plan` plans id 42; its
  container-entry version argument is the one to advance. `pack_output::report_section_footprint`
  is the byte report (`prl-build -v`).
- **Renderer.** `filter_usable_shadowmask_section` and `upload_shadowmask_texture`
  (`lighting/lightmap.rs`): compare `2W` against `max_texture_dimension_2d`, reject
  misalignment, upload `Bc5RgUnorm` at `2W × H × L`. `upload_placeholder_shadowmask`
  stays `Rgba8Unorm` 1×1×1 white.
- **Shader.** `sample_shadowmask_atlas` in `forward.wgsl` is the only atlas read; its
  call sites are the promoted-union subtraction and the hoisted specular sample in
  `fs_main`. Group width in texels is `textureDimensions(shadowmask_atlas).x / 2`, which
  is zero against the 1×1 placeholder — guard it, or make the placeholder two texels
  wide so each half is one white texel. Sharing one sample pair
  across both paths is permitted where their gating conditions allow.
- **Guards that change.** `forward_shader_shadowmask_fallback_clamps_multilayer_indices`
  pins today's sample count and clamp text; rewrite it to the new shape.
  `forward_pipeline_sampled_texture_request_matches_bgl_definitions` must stay green
  untouched.
- **Overlap memo.** The peak count comes from `build_analytic_overlap_graph_in_order`,
  which only a shadowmask memo miss runs; the memo entry carries it (a sibling entry or
  an in-entry field is the executor's call) without adding it to the shipped section.
- **Payload ownership.** `LevelWorld.lightmap` and `LevelWorld.shadowmask_atlas`
  (`level-loader/src/prl.rs`, also exposed via `LevelWorld::lighting()`) split into
  retained headers and movable payloads. `install_level_payload`
  (`startup/lifecycle.rs`, past 4,000 lines — a few-line change) moves the payloads
  into geometry install before stashing the world into `App.level`.
  `level_world_to_geometry` and `LevelGeometry` carry them to `LightmapResources::new`,
  the one constructor that uploads irradiance, direction and shadowmask. The capture
  harness is the second install path: `capture/setup.rs` `capture_level_geometry`
  spreads `level_world_to_geometry` over a borrowed world, then `capture/prepared.rs`
  installs it. The `LevelGeometry { lightmap: None, .. }` builders in `mesh_render.rs`,
  `particle_render.rs` and `startup/lifecycle.rs` follow the type change.
  `usable_atlas_dimensions` reads id 22's header only.
- **Proof tooling.** Byte report and frame-time harness: `research.md` §Proof support.
- **First slice.** Tag + encode + upload + shader on a fixture where four selected lights
  overlap across both groups with a mask edge at the seam, checked by capture. It
  falsifies the wire, codec, seam, upload and shader boundaries together before the
  tests fan out.
- **Rival shapes.** Raw loadable behind the tag, and groups stacked in array layers —
  both rejected in `research.md`.

## Open questions

None.
