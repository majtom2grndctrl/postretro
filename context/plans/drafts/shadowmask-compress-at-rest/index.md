# shadowmask-compress-at-rest

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4, `context/lib/build_pipeline.md` §PRL, `context/lib/testing_guide.md` §Resource bounds · read at a097035

## Problem

Sizing a ~2.39 GB baked warren `.prl`, the developer flagged the `ShadowmaskAtlas`
(PRL id 42) as a large, reducible section. It is the lone baked atlas still stored raw:
`ShadowmaskAtlasSection` (`crates/level-format/src/shadowmask_atlas.rs`) is an
uncompressed layer-major `Rgba8Unorm` payload — `width × height × layer_count × 4` bytes,
sized to the shared lightmap irradiance atlas and independent of light count — while its
siblings id 22 (lightmap) and id 35 (direct SH) already ship BC-compressed at rest. That
raw-vs-BC-sibling asymmetry — verified in source, independent of the exact file fraction —
is what justifies the work. It is also double-resident: the full payload is held in `LevelWorld.shadowmask_atlas`
for the level lifetime *and* uploaded whole to VRAM as an `Rgba8Unorm` D2Array, though the
renderer keeps only the 1-byte-per-light `channels` table after upload. This is a
requested capability — cut the atlas's file, RAM, and VRAM footprint. When done, id 42
is stored and uploaded as a GPU-native block-compressed texture (≈4:1), the CPU-side
payload is released once it is on the GPU, and the world-specular signal it drives is
visually unchanged.

## Decisions

- **Block-compress id 42 at rest, sampled compressed.** Store and upload the payload as a
  GPU-native BC texture instead of `Rgba8Unorm`; the sampler decompresses in hardware, so
  the bytes on disk, in the CPU load buffer, and in VRAM all shrink together. This mirrors
  the at-rest BC discipline already established for id 22 and id 35 (`DirectShVolume = 35`
  is doc-noted "Stored BC6H-compressed at rest", `crates/level-format/src/lib.rs`). Lossy
  is accepted: the atlas is a soft, specular-only visibility signal whose fallback is
  already "fully lit" (`rendering_pipeline.md` §4 World specular shadowmask), so codec
  error is bounded and affects highlights only.
- **Default codec BC7; per-channel BC4 is the fallback.** BC7 unorm is sampled as `f32`
  in `[0,1]` identically to `Rgba8Unorm`, so it is a *drop-in encoding swap* — the shader
  (`forward.wgsl` `sample_shadowmask_atlas` → `shadowmask_channel_value`) is unchanged and
  only the compiler-write and renderer-upload sides move. BC7 is ≈4:1. The four RGBA
  channels are independent per-light masks, not correlated color, so if BC7's cross-channel
  block error fails the fidelity gate, fall back to per-channel BC4 (≈2:1, high per-channel
  fidelity) — a heavier change that restructures the atlas into per-channel planes and does
  touch the shader/binding. The final pick is delegated (Open questions), gated by the
  fidelity and visual ACs.
- **No new device requirement or alignment constraint.** id 22/35 already require the
  `TEXTURE_COMPRESSION_BC` adapter feature, so BC7 adds none. BC is 4×4-block; the
  shadowmask shares the lightmap atlas's dimensions, which are already BC-aligned because
  id 22 is BC6H — so no padding is introduced.
- **The BC encode must be byte-deterministic.** `build_pipeline.md` §Build Cache keys the
  shadowmask memo on inputs and expects a reproduced output, and the sibling draft carries a
  byte-identical re-bake AC; a lossy BC7 block search is a common source of non-deterministic
  output across threads or encoder versions. The chosen encoder must be an order-deterministic,
  pinned-version function of the raw atlas so re-baking a fixture yields byte-identical
  compressed output and the cache stays valid.
- **Release the CPU payload after GPU upload.** Drop `LevelWorld.shadowmask_atlas.data`
  (retaining the tiny `channels` table the renderer clones) once `upload_shadowmask_texture`
  has run, removing the RAM half of the double residency. The executor confirms no other
  consumer reads `.data` post-upload before freeing it; level reload re-reads from a fresh
  world, so reuse is not a consumer.
- **Preserve the graceful-degradation and double-count contracts unchanged.** Absent,
  rejected, or over-limit shadowmask data still resolves to fully lit via
  `filter_usable_shadowmask_section` → the 1×1 placeholder; the static→static union
  dead-zone (`rendering_pipeline.md` §4 Pool-shadow receiver bias) is untouched. An
  adapter lacking BC support takes the existing placeholder path, never a panic.
- **Version id 42 jointly with `shadowmask-no-drop-atlas`.** That draft adds a `block_count`
  header field and extends the payload to `layer_count × block_count` array layers; this
  brief adds a codec tag and changes the per-texel encoding. The two are orthogonal
  (block-stacking adds layers, compression changes texel bytes) but share the id-42 header,
  `from_bytes` payload cross-check, and upload/filter path. Whichever lands second composes
  onto the first in one header carrying both a codec tag and `block_count`, with the payload
  a BC-block stream over `layer_count × block_count` layers. Undo cost is a re-bake
  (pre-stable, no external `.prl` consumers per `development_guide.md` §1.6).
- **Placement:** load-time/at-rest format and renderer upload; the GPU stays renderer-owned.
  Whole-atlas resident — no residency or streaming machinery here.

### Non-goals

- **Streaming / visibility-driven residency (occlusion culling) of the atlas.** A later epic
  over the cell-keyed atlases owns it: `sh-probe-streaming` (in-progress) builds a general
  cluster-of-cells residency substrate on the visible-cell signal and names lightmap-layer /
  shadowmask as future subscribers but wires only SH. Compression composes with, and does
  not foreclose, that later work (re-bakeable into cluster-addressable compressed form).
- **Material / texture residency.** A separate domain keyed on material+mip, explicitly
  excluded from the spatial substrate by `sh-probe-streaming`.
- **Sparse / empty-region footprint trim.** Not pursued; the resident-set reduction it would
  give is subsumed by the streaming epic above.
- **The id-42 block dimension for >4 overlap.** `shadowmask-no-drop-atlas` owns it; this
  brief only coordinates the shared version bump (above).
- **Changing the "shadowmask dims == lightmap dims" invariant.** BC7 preserves dimensions.

## Acceptance

### Automated
- [ ] `ShadowmaskAtlasSection::to_bytes` → `from_bytes` round-trips a compressed section
  (codec tag preserved, payload length cross-check uses the compressed block size for the
  chosen codec); a payload whose length disagrees with the codec's block math is rejected.
- [ ] On a focused fixture map carrying a populated shadowmask atlas, the id-42 on-disk
  section byte count drops by the chosen codec's ratio versus the raw `Rgba8Unorm` baseline
  (≈4:1 for BC7), measured by the per-section byte accounting in `pack.rs`.
- [ ] The renderer uploads id 42 as the BC texture and the startup per-atlas VRAM estimate
  for the shadowmask drops by the same ratio; an all-visible (255) atlas round-trips to
  fully lit after encode→decode.
- [ ] After upload, the shadowmask CPU payload is released — the level holds no
  `width × height × layer_count × 4` shadowmask buffer resident — and a subsequent level
  reload still installs a correct atlas.
- [ ] An adapter without `TEXTURE_COMPRESSION_BC`, and a section rejected by
  `filter_usable_shadowmask_section`, both fall back to the all-visible placeholder (fully
  lit), no panic.
- [ ] Re-baking the same fixture twice yields byte-identical compressed id-42 output
  (deterministic encoder), so the build cache and any concurrent sibling re-bake AC hold.
- [ ] Fidelity, measure-and-report: the max and mean per-channel absolute error of the
  chosen codec versus the raw masks, on the fixture, recorded in the landing note.

### Manual
- [ ] In a scene with selected non-SDF static world specular lights, specular highlights and
  their shadowmask-occluded regions read unchanged against a raw-atlas capture (the visual
  gate for the codec pick). Reuse the offscreen capture path (`capture_frame_indirect`).
- [ ] A heavily-masked region retains its specular contrast; absent/dropped masks still read
  fully lit.

## Wire format

`ShadowmaskAtlasSection` keeps its little-endian shape (aligned header, then `channels`,
pad to 4, then the layer-major payload) and section id 42, with:

- A codec tag in the header identifying the payload encoding (raw `Rgba8Unorm` retained as
  a value so an uncompressed section stays representable). The tag and any
  `shadowmask-no-drop-atlas` `block_count` field share one advanced format/PRL version.
- `data` is the BC-block stream for the tagged codec, layer-major over the array layers;
  `from_bytes`' cross-check computes the expected length from the codec's block size and the
  atlas dimensions (× `block_count` if that field is present).

Field widths and the exact header slot are implementation choices; the constraint is that
the codec tag round-trips and the payload length check matches the codec.

## Path

- **Reuse the id-35 / id-22 BC-at-rest encode seam.** Find whatever encoder the compiler
  already uses to produce BC6H for `DirectShVolume`/lightmap at rest and route BC7 through
  it; the bake site is the shadowmask pack path (`crates/level-compiler/src/pack.rs`,
  `shadowmask_bake.rs`). Match its input domain and precision against the mask semantics
  before mandating reuse (`context_style_guide.md` §Spec Completeness).
- **Upload seam:** `upload_shadowmask_texture` and `filter_usable_shadowmask_section`
  (`crates/renderer/src/lighting/lightmap.rs`) select the `TextureFormat` from the codec tag;
  the D2Array view, LayerMajor upload, and group-4 binding are otherwise reused.
- **CPU release seam:** the atlas lives in `LevelWorld.shadowmask_atlas` stashed via
  `self.level = Some(world)` (`crates/postretro/src/startup/lifecycle.rs`); the renderer
  borrows it at install and clones only `channels`. Free the payload after upload without
  disturbing the `channels` clone.
- **Shape chosen:** BC7 drop-in encoding swap (shader untouched). Strongest rival: per-channel
  BC4, principled for independent masks but restructures the atlas and touches the shader —
  taken only if BC7 fails fidelity.
- **First slice:** encode one fixture atlas to BC7, upload it, and A/B the specular capture
  against the raw baseline — falsifies the fidelity assumption (that a 4:1 lossy codec on
  binary-ish masks is visually acceptable) before touching the format/version or the CPU-free.
- **Sequencing with `shadowmask-no-drop-atlas`:** both edit the id-42 header, `from_bytes`,
  and the upload/filter path; land aware of the other and compose the header (codec tag +
  `block_count`) under one version rather than two bumps.
- Do not bake `stress-warren*` for proof: the compression ratio is codec-intrinsic and
  per-channel error is texel-local, so a small fixture with selected specular lights suffices.

## Open questions

- How do the two concurrent id-42 drafts share the header? — owner: developer — **blocks build**.
  This brief and `shadowmask-no-drop-atlas` both rewrite the id-42 header, `from_bytes`
  cross-check, and upload/filter path. "Compose onto whichever lands first" leaves no single
  owner verifying the combined `block_count` + codec-tag header. Choose: merge the two into one
  id-42 format change; or have one draft own and land the header first with the other building
  on the landed format; or keep them independent and accept the compose-on-merge risk.
- Final codec — BC7 (≈4:1, drop-in) vs per-channel BC4 (≈2:1, shader-touching) — **delegated**:
  the executor defaults to BC7, falls back to BC4 only if BC7 fails the fidelity/visual ACs,
  and reports the pick and measured error in the plan of record.
