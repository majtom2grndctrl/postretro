# Shadowmask Atlas — BC5 Compress-at-Rest

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4,
`context/lib/build_pipeline.md` §PRL · §Build Cache, `context/lib/development_guide.md`
§1.4, `context/lib/testing_guide.md` §Resource bounds · read at 8ce91e682

This brief changes bytes, never masks. Per-texel mask capacity is
`shadowmask-atlas-mask-capacity`, gated on the overlap measurement this brief ships.

## Problem

A developer-raised anticipated need, not an observed defect. PRL id 42
(`ShadowmaskAtlasSection`) is the only baked atlas still stored raw: no bake or emit
stage compresses it, so its `Rgba8Unorm` payload sits uncompressed on disk, in CPU RAM
for the whole level lifetime, and in VRAM. The masks are independent, spatially smooth
`[0,1]` visibility scalars — BC4's home case, and BC5 is two BC4 blocks in one. Two BC5
groups spend two array layers per lightmap layer against a 256-layer device budget, and
the lightmap packer today lets an ordinary mid-size level pass 128 layers: it sizes
layers to the largest BVH leaf and spills the rest, and authors cannot steer the count.
When this lands, id 42 is stored and uploaded as BC5 at half the payload bytes, the CPU
copy is released once uploaded, the packer holds every level within the budget two
groups need, and every shadow reads as it did on every level. No id-42 byte magnitude
is recorded anywhere in the repo; the landing note records one, before and after.

## Decisions

- **BC5 `.rg` in exactly two array-layer groups — four masks per texel, today's
  number.** No capacity claim, so no capacity floor, no widened slot index, and no
  dependence on the unmeasured overlap frequency. Capacity is the sibling brief's.
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
  today's shape — `(g0.r, g0.g, g1.r, g1.g)` — from two samples, so the per-light
  select and both decode paths (promoted-union subtraction, world specular) keep their
  shape. Both group layers derive from `lightmap_layer` alone, so the samples stay
  hoisted outside every light loop in uniform control flow. Cost: at most one extra
  sample per active decode path per fragment, independent of light count.
- **The lightmap packer bounds the layer count at half the device array budget.** When
  a pack would exceed 128 layers, the packer doubles the shared layer dimension and
  repacks rather than adding layers, keeping leaf cohesion. Levels already within the
  budget pack exactly as today. The budget holds whether or not the level selects any
  shadowmask lights, so the atlas layout never depends on light selection. This also
  retires today's hard layer-overflow failure short of the 8192² dimension cap. It
  changes atlas preparation (`build_pipeline.md` §Compiler pipeline, atlas
  preparation), and downstream memos keyed on the prepared layout miss on repacked
  levels.
- **Over budget or misaligned data degrades to fully lit, never raw.** The compiler
  never emits either; the runtime still guards against corrupt or hand-built data. Two
  groups need `2 × layer_count` within the pinned array-layer maximum; a BC5 atlas
  needs 4-aligned dimensions. Either failure resolves to the all-visible placeholder, as
  `rendering_pipeline.md` §4 commits for rejected data. The placeholder clamp covers
  the second group's layer.
- **The upload owns the payload.** Level install moves the payload out of the loaded
  world into the atlas upload, which drops it once the texture exists. The world keeps
  the slot table and atlas dimensions — spec-light channel mapping keys on their
  presence — and has no representation of a header without its payload, so nothing can
  re-validate or re-upload an emptied section. An install with no renderer moves
  nothing. It rides with BC5 because both answer the same double residency of one
  section, and every load or reload re-reads the level from disk.
- **No new device requirement, binding, or padding.** `TEXTURE_COMPRESSION_BC` is
  already required at device acquisition. Format and layer count change behind the same
  group-4 binding, so the forward texture inventory is unchanged. Lightmap atlas
  dimensions are powers of two ≥ 64, which the shadowmask shares.
- **Compile-time peak is bounded at 1.5× the raw fill plus one layer.** The raw fill,
  its BC5 output and one layer's encode scratch coexist only inside the finish step; the
  raw buffer drops before the section leaves the stage. Runtime residency falls: VRAM
  halves, CPU goes to zero after upload.
- **Report peak per-texel overlap under `--verbose`, on every bake.** Not acted on
  here; it is the capacity brief's missing premise. The count comes from the overlap
  graph, which only a memo miss builds, so the memo carries it beside the section and a
  hit reports it too. The shipped section does not carry it — no runtime reader.

### Non-goals

- **Mask capacity above four.** `shadowmask-atlas-mask-capacity`. Unmeasured and
  unreported, so it must not gate a certain win.
- **A runtime raw decode path**, including as an A/B toggle.
- **BC7.** `bc7-color-textures` owns it; BC7 models cross-channel correlation four
  independent masks lack.
- **Streaming the shadowmask.** `sh-probe-streaming` excludes lightmap residency and its
  cluster directory admits no id 42. Bytes per resident texel are orthogonal to which
  texels are resident.
- **Lightmap array consolidation.** `shadowmask-atlas-mask-capacity` owns it; only a
  second shadowmask binding needs it.
- **Selection eligibility and ranking.** Unchanged.

## Acceptance

### Automated

**Wire**
- [ ] Header, format tag, slot table and payload round-trip byte-exact for an empty
      selection, a single selected light, and a full four-slot table.
- [ ] `from_bytes` rejects: a payload length that disagrees with 16 bytes per 4×4
      block over `2 × layer_count` layers; dimensions not multiples of 4; an unknown
      tag; a slot neither `0..3` nor the sentinel.
- [ ] A pre-change raw id-42 payload loads with a warning naming the format mismatch,
      renders fully lit, and does not panic — including a payload whose bytes at the
      tag's position read as a valid tag, which must not fall through to a length
      mismatch. (pin: stale-payload, stale-payload-tag-collision)
- [ ] A warm cache holding a pre-change shadowmask entry never serves it; the rebuilt
      section equals the uncached section byte-for-byte. (pin: stale-memo)
- [ ] A level whose selected lights are all dropped ships the section at half the raw
      bytes, loads it, and renders fully lit. (pin: all-sentinel)

**Layer budget**
- [ ] A level whose pack fits within 128 layers yields a prepared atlas byte-identical
      to the pre-change packer's. (pin: budget-permit)
- [ ] A pack of exactly 128 layers keeps its layer dimension; one that would need 129
      packs at a larger power-of-two dimension into at most 128 layers, with every
      leaf's charts on one layer. (pin: budget-boundary)
- [ ] A level that fails today with a layer-overflow error above 256 layers now packs.
- [ ] The same geometry packs to the same layout with and without selected shadowmask
      lights. (pin: layout-selection-independent)
- [ ] A repacked level's shadowmask loads and resolves its masks rather than falling to
      the placeholder. (pin: repacked-end-to-end)

**Degradation**
- [ ] A sentinel slot reads fully lit. A second-group slot against the one-layer
      placeholder reads fully lit without addressing outside the bound texture.
- [ ] `2 × layer_count` equal to the pinned array-layer maximum is kept; one greater
      degrades to the placeholder with a `[Renderer]` error and no panic.
- [ ] A bake given atlas dimensions that are not multiples of 4 fails with an error
      naming them, in release as in debug; it never emits a truncated payload.
      (pin: bake-misaligned)

**Masks**
- [ ] Four overlapping selected lights, spread across both groups, each resolve to their
      own mask in both decode paths — no drop, no cross-talk between groups. Proven by
      offscreen render on an adapter; a skipped run does not count.
- [ ] Moving a light from the first group to the second leaves world-specular output
      unchanged for surfaces covered only by first-group lights; static→static world
      shadowing stays exactly zero. Proven by offscreen render on an adapter; a skipped
      run does not count.
- [ ] An all-visible (255) atlas decodes fully lit after encode.
- [ ] Grep gate over the forward shader: no shadowmask sample sits inside a light loop,
      each decode path issues its samples once per fragment, and the second group's
      layer derives only from the base lightmap layer and the bound texture's layer
      count.
- [ ] The forward pass binds no new texture or sampler; its per-group sampled-texture
      inventory is unchanged.

**Bytes and determinism**
- [ ] On a populated fixture, the id-42 payload is exactly half the raw arithmetic
      (`W × H × layer_count × 2` against `× 4`) in the per-section footprint report.
- [ ] The texture description built for the atlas — BC5, `2 × layer_count` layers —
      and its byte size, half the raw description's, are checked without a GPU.
- [ ] On a cache miss the bake holds one raw fill, one compressed output and at most one
      layer of encode scratch, and the raw fill is gone before the section is cached or
      returned.
- [ ] Re-baking twice yields a byte-identical section across differing worker-thread
      counts, and a cached-warm section equals the uncached one byte-for-byte.
- [ ] Measured and reported, not gated: max and mean per-channel absolute error of the
      encode against the raw masks, recorded in the landing note.

**Lifecycle**
- [ ] After install, the loaded world holds the slot table and atlas dimensions and no
      shadowmask payload; the payload's allocation is freed once the texture exists.
- [ ] The payload is released only after the atlas upload; an install that uploads
      nothing keeps it. (pin: no-upload-install)

**Overlap report**
- [ ] A cold bake under `--verbose` reports peak per-texel overlap with layer count and
      format; a warm memo hit reports the same value; a non-verbose bake gains no line.
      (pin: overlap-memo-hit)

### Manual

- [ ] The id-42 section byte count, layer count and atlas dimensions on
      `campaign-test`, before (pre-change commit) and after, in the landing note, with
      the bake's peak RSS alongside — the compile-time bound's evidence.
- [ ] Offscreen capture of a scene with selected non-SDF static specular lights, against
      the same capture from the pre-change commit: highlights and their occluded regions
      read unchanged. Look at both images; a distribution check passes on an image that
      lost its contrast.
- [ ] Frame time, measured and reported: capture-harness CPU completion median and p95,
      before and after, on a scene whose fragments carry several selected static
      specular lights. Pin fixture, machine class, build profile, worker count and
      cache mode per `testing_guide.md` §Resource bounds. May need an attended run on
      this Mac.
- [ ] `campaign-test`'s peak per-texel overlap under `--verbose`, recorded in the
      landing note for the capacity brief.
- [ ] After a level reload and a dev level cycle, second-group shadows render on the
      new level. (pin: release-then-reload)

## Wire format

`ShadowmaskAtlasSection`, id 42. Little-endian throughout. Section order unchanged.

- **Header** gains a format tag. One value today: BC5 `.rg`, two groups. Group count is
  implied by the tag; the capacity brief adds a group-count field if it needs one.
- **Slot table** unchanged: `u8` per selected light, padded to 4. Slot `s` → group
  `s / 2`, channel `s % 2`, array layer `lightmap_layer + (s / 2) × layer_count`.
- **Payload**: BC5 blocks, 16 bytes per 4×4 block, layer-major over `2 × layer_count`
  layers — group 0's layers first, then group 1's.
- **Rejects**: unknown tag, length disagreeing with the tag's arithmetic, dimensions not
  4-aligned, out-of-range slot.

Header slot positions and tag width are implementation choices. Binding constraints:
the tag round-trips, the length check follows the tag, the sentinel keeps its meaning,
and no pre-change payload parses as valid.

## Path

Non-binding.

- **Encode seam.** `ShadowmaskFill::finish` (`shadowmask_bake/fill.rs`) moves raw RGBA
  into the section; `empty_section_for_dimensions` builds sections too and must emit the
  same tagged shape. Transpose each
  lightmap layer's RGBA into two RG planes and run `bc5::encode_bc5_rg` per plane. Put
  the encode in its own submodule — `shadowmask_bake.rs` is past 4,000 lines and gains
  no new responsibility here.
- **Cache writer.** `cache_shadowmask_section_then_complete` streams its own copy of the
  header layout and bypasses `to_bytes`. It must learn the tag, and it is where
  warm/cold drift would hide — prefer one header-writing helper both paths share. Bump
  `SHADOWMASK_ATLAS_STAGE_VERSION`; its test pins the value.
- **Pack seam.** `pack/section_plan.rs` `build_finalized_section_plan` plans id 42; its
  container-entry version argument is the one to advance. `pack_output::report_section_footprint`
  is the byte report (`prl-build -v`).
- **Renderer.** `filter_usable_shadowmask_section` and `upload_shadowmask_texture`
  (`lighting/lightmap.rs`): compare `2 × layer_count`, reject misalignment, upload
  `Bc5RgUnorm`. `upload_placeholder_shadowmask` stays `Rgba8Unorm` 1×1×1 white.
- **Shader.** `sample_shadowmask_atlas` in `forward.wgsl` is the only atlas read; its
  two call sites are the promoted-union subtraction and the hoisted specular sample in
  `fs_main`. Group 1's layer offset can come from `textureNumLayers / 2`, which also
  clamps correctly against the placeholder. Sharing one sample pair across both paths
  is permitted where their gating conditions allow.
- **Guards that change.** `forward_shader_shadowmask_fallback_clamps_multilayer_indices`
  pins today's sample count and clamp text; rewrite it to the new shape.
  `forward_pipeline_sampled_texture_request_matches_bgl_definitions` must stay green
  untouched.
- **Packer.** `lightmap_bake::pack_layers` / `choose_layer_dim`, called from
  `prepare_atlas` on both branches. The overflow check today returns
  `LayerOverflow` at `MAX_ATLAS_LAYERS`; the budget becomes a parameter so fixtures can
  pass a small one alongside their small `max_dim`. `research.md` §Lightmap layer budget.
- **Overlap memo.** The peak count comes from `build_analytic_overlap_graph_in_order`,
  which only a shadowmask memo miss runs; the memo entry carries it (a sibling entry or
  an in-entry field is the executor's call) without adding it to the shipped section.
- **Payload ownership.** `LevelWorld.shadowmask_atlas` (`level-loader/src/prl.rs`,
  accessor via `LevelWorld::lighting()`) splits into a retained slot table plus dims
  and a movable payload. `install_level_payload` (`startup/lifecycle.rs`, past 4,000
  lines — a few-line change) moves the payload into geometry install before stashing
  the world into `App.level`. `level_world_to_geometry` and `LevelGeometry` carry the
  owned payload to `upload_shadowmask_texture`.
- **Proof tooling.** Byte report and frame-time harness: `research.md` §Proof support.
- **First slice.** Tag + encode + upload + shader on a fixture where four selected lights
  overlap across both groups, checked by capture. It falsifies the wire, codec, upload
  and shader boundaries together before the tests fan out.
- **Rival shape.** Keep raw loadable behind the tag for a live A/B — rejected in
  `research.md` §Why BC5 only.

## Open questions

None.
