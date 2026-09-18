# Shadowmask Atlas — Stacked Channel Blocks and Compress-at-Rest

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4, `context/lib/build_pipeline.md` §PRL · §Build Cache, `context/lib/testing_guide.md` §Resource bounds · read at a097035

## Problem

The `ShadowmaskAtlas` (PRL id 42, `ShadowmaskAtlasSection` in
`crates/level-format/src/shadowmask_atlas.rs`) must evolve once to do two things it
cannot do today: (a) carry masks for more than four overlapping selected static lights,
and (b) store the atlas compressed at rest, cutting its file, RAM, and VRAM footprint.
Both changes rewrite the same wire surface — the id-42 header, the payload, and
`from_bytes`' cross-check — so they are one format evolution, not two. Today a texel
packs per-light masks into the four RGBA channels of one raw `Rgba8Unorm` texel: a fifth
overlapping light's mask is dropped (lowest-intensity first) and its runtime shadow
disappears, and the whole payload is stored and held uncompressed. That raw encoding is
also the lone at-rest asymmetry among the baked atlases: id 22 (lightmap) and id 35
(`DirectShVolume`) already ship BC-compressed (`crates/level-format/src/lib.rs` notes id
35 "Stored BC6H-compressed at rest"; `crates/level-compiler/src/pack.rs` has no
compression step for id 42), while id 42 alone stays raw. Evolving the header to carry
both a **block dimension** (more mask slots per texel) and a **codec tag** (compressed
texel bytes) closes both gaps under one version bump.

## Decisions

- **Block-slot expansion.** Add a block dimension stacked in the atlas's array layers so
  a texel carries `4 × block_count` masks. The per-selection channel table encodes a
  `(block, channel)` **slot** `s = block * 4 + channel`; the payload is layer-major over
  `layer_count × block_count` array layers. A mask is dropped only when assigning it
  would push `layer_count × block_count` past the device array-layer budget
  (`max_texture_array_layers`, the portable baseline 256) — a far higher ceiling than
  four; past it the existing lowest-intensity drop is the graceful fallback. The
  `<= 3 || 0xFF` channel gate relaxes to the slot range plus the dropped sentinel
  (`SHADOWMASK_CHANNEL_DROPPED`).
- **Block-compress id 42 at rest, sampled compressed.** Store and upload the payload as
  a GPU-native BC block-compressed texture; the sampler decompresses in hardware, so the
  bytes on disk, in the CPU load buffer, and in VRAM shrink together. This mirrors the
  at-rest BC discipline already set for id 22 and id 35. Lossy is accepted: the atlas is
  a soft specular-only visibility signal whose fallback is already "fully lit"
  (`rendering_pipeline.md` §4 World specular shadowmask), so codec error is bounded and
  affects highlights only.
- **Codec: BC7 and per-channel BC4 trade on opposite axes; the default is an owner call
  (Open questions).** BC7 gives ≈4:1 and a drop-in **shader** — BC7 unorm samples as `f32`
  in `[0,1]` identically to `Rgba8Unorm`, so `forward.wgsl`'s
  `sample_shadowmask_atlas`/`shadowmask_channel_value` are unchanged for the encoding — but
  its *encoder* is the expensive side: no BC7 **unorm** encoder exists in-tree (the in-tree
  BC encoders are HDR-RGB BC6H, `crates/level-compiler/src/bc6h.rs`, and BC5, `bc5.rs`;
  neither is a BC7-unorm), and `bc7-color-textures` already flags a BC7
  encoder as unresolved heavy work (building or vendoring one, or sequencing after that
  draft). Per-channel BC4 inverts it: ≈2:1 and it *does* touch the shader (channels become
  per-channel planes; the `channel` selector becomes a plane index that composes with the
  block decode), but its encoder is largely in-tree — BC4 is one channel of the existing
  `bc5.rs` unorm path — and it is the semantic fit for four *independent* per-light masks,
  where BC7 models a cross-channel correlation the masks do not have. So BC4 lands
  self-contained with no cross-plan dependency. The fidelity dimension stays delegated (fall
  back off the chosen codec if it fails the fidelity/visual ACs); the effort/ratio/dependency
  dimension is the owner question below. (Determinism is not a discriminator — neither codec
  is held to byte-identical output; see the determinism decision.)
- **No new device requirement or alignment constraint.** id 22/35 already require the
  `TEXTURE_COMPRESSION_BC` adapter feature, so BC7 adds none. BC is 4×4-block; the
  shadowmask shares the lightmap atlas dimensions, already BC-aligned because id 22 is
  BC6H — no padding introduced.
- **Determinism, split to match the cache invariant.** The `(block, channel)` slot
  assignment must be a pure, order-deterministic function of the selection and per-light
  layer inputs, so the *logical* (pre-compression) atlas re-bakes byte-identically —
  required and precedent-aligned. The lossy BC encode is **not** held to byte-identity:
  `build_pipeline.md` §Build Cache exempts lossy compressed output (the BC6H irradiance
  atlas is exempt; the cache keys on inputs, not outputs), and
  `sh-base-atlas-at-rest-slimming` sets the posture — the exact/raw stage is byte-identical,
  the BC default path need only be **section-length-stable**. The encoder is a pinned
  version for reproducibility hygiene, but byte-identity is not a gate. (Corrects the
  earlier merged stance, which held the BC bytes to byte-identity and thereby manufactured a
  determinism objection to BC7 the engine's own invariant does not require.)
- **Free the CPU payload after upload.** Drop `LevelWorld.shadowmask_atlas.data`
  (retaining the tiny `channels` table the renderer clones) once
  `upload_shadowmask_texture` has run, removing the RAM half of the double residency. The
  executor confirms no other consumer reads `.data` post-upload before freeing it; level
  reload re-reads from a fresh world, so reuse is not a consumer.
- **Preserve graceful-degradation and double-count unchanged, extended.** Absent,
  rejected, or over-budget shadowmask data still resolves to fully lit via
  `filter_usable_shadowmask_section` → the placeholder; the static→static union dead-zone
  (`rendering_pipeline.md` §4 pool-shadow bias) is untouched. Extend the placeholder path
  to two new causes: `layer_count × block_count` over the device budget, and an adapter
  lacking `TEXTURE_COMPRESSION_BC` — both degrade to the all-visible placeholder, never a
  panic.
- **Placement.** A runtime-format capacity-and-encoding change spanning the at-rest wire
  format, the compiler's slot assignment and BC encode, the renderer upload/metadata, and
  the shader — each edited at its own boundary, with the slot and codec encodings pinned
  once (Boundary inventory). Blocks stack in the existing `texture_2d_array`; the GPU
  stays renderer-owned; the whole atlas is resident (no residency or streaming machinery
  here).

### Non-goals

- **Streaming / visibility-driven residency of the atlas.** Deferred to the in-progress
  `sh-probe-streaming` cell-cluster substrate, which builds a resource-agnostic
  cluster-of-cells residency layer on the visible-cell signal and names lightmap-layer /
  shadowmask as future subscribers but wires only SH (id 49 cluster directory reserved,
  unemitted). Compression composes with, and does not foreclose, that work
  (re-bakeable into cluster-addressable compressed form).
- **Material / texture residency.** A separate material+mip domain, explicitly excluded
  from the spatial substrate by `sh-probe-streaming`.
- **Sparse / empty-region footprint trim.** Subsumed by the streaming epic above.
- **The shadowmask bake's memory / parallelism / progress.** Owned by
  `shadowmask-bake-scaling` (sibling); this brief changes what the bake emits and how it
  is assigned to slots and encoded, not the bake's allocation lifecycle.
- **The lightmap / irradiance / direction atlases.** Untouched; `lightmap_layer` (the
  receiver's spatial array layer) keeps its meaning. Only the shadowmask atlas gains the
  block dimension and the codec tag. The "shadowmask dims == lightmap dims" invariant is
  unchanged (BC preserves dimensions).
- **Old-`.prl` migration.** All fixtures re-bake from source; the format version advances
  and stale caches regenerate.
- **Raising the per-texel cap above the device array-layer budget.** Past
  `layer_count × block_count = 256` the existing lowest-intensity drop is retained as the
  graceful fallback — a chosen, owner-visible ceiling.
- **Selection eligibility / ranking.** Which lights are selected (`entity_shadow_select`)
  is unchanged.

## Acceptance

### Automated
- [ ] On a fixture with a lightmap texel overlapped by more than four selected static
  lights (6–8), every selected light receives a non-dropped `(block, channel)` slot and
  its shadow is present at runtime — no mask dropped below the device array-layer budget.
- [ ] A mask is dropped only when assigning it would push `layer_count × block_count`
  past `max_texture_array_layers`; at or below the budget, no drop. The drop, when it
  happens, is the lowest-intensity mask, matching the pre-change policy.
- [ ] `ShadowmaskAtlasSection::to_bytes` → `from_bytes` round-trips the `(block, channel)`
  slot table, the codec tag, and the layer-major payload for `block_count > 1`;
  `from_bytes` rejects an out-of-range slot and a payload whose length disagrees with the
  codec's block size × `width × height × layer_count × block_count`.
- [ ] A section whose `layer_count × block_count` exceeds the device budget is rejected
  by `filter_usable_shadowmask_section` with a `[Renderer]` error and the all-visible
  placeholder (fully lit), no panic.
- [ ] Static→static world shadowing stays exactly zero: adding a light that lands on
  `block > 0` does not change world-specular output for surfaces already covered by
  block-0 lights (the pool-shadow union dead-zone is unaffected).
- [ ] Both shader decode paths (world-specular, promoted-union) sample the correct array
  layer `lightmap_layer + block × layer_count` and channel for a light on any block;
  shader tests covering `block > 0` pass.
- [ ] A single-block map (`block_count == 1`) produces a section semantically equivalent
  to the pre-change 4-channel layout (same masks, drop-nothing when ≤4 overlap).
- [ ] On a focused fixture carrying a populated atlas, the id-42 on-disk section byte
  count drops by the chosen codec's ratio versus the raw `Rgba8Unorm` baseline (≈4:1 for
  BC7), measured by the per-section byte accounting in `pack.rs`.
- [ ] The renderer uploads id 42 as the BC texture and the startup per-atlas VRAM
  estimate for the shadowmask drops by the same ratio; an all-visible (255) atlas
  round-trips to fully lit after encode→decode.
- [ ] After upload, the shadowmask CPU payload is released — the level holds no
  `width × height × layer_count × block_count × (codec bytes)` shadowmask buffer resident
  — and a subsequent level reload still installs a correct atlas.
- [ ] An adapter without `TEXTURE_COMPRESSION_BC` falls back to the all-visible
  placeholder (fully lit), no panic.
- [ ] Re-baking the same fixture twice yields a byte-identical slot-assigned
  (pre-compression) atlas and a section-length-stable compressed id-42 section, so the build
  cache stays valid. The lossy BC bytes need not be identical (matching the BC6H-irradiance
  cache exemption).
- [ ] Fidelity, measure-and-report: the max and mean per-channel absolute error of the
  chosen codec versus the raw masks, on the fixture, recorded in the landing note.

### Manual
- [ ] The authoring signal is preserved: the bake logs a warning when overlap forces the
  block count near the device array-layer budget (a spot over-piled with lights), and
  reports the peak observed per-texel overlap and the resulting `block_count` under
  `--verbose`. A non-verbose bake with comfortable headroom gains no new log spam.
- [ ] In a scene with selected non-SDF static world specular lights, specular highlights
  and their shadowmask-occluded regions read unchanged against a raw-atlas capture (the
  visual gate for the codec pick). Reuse the offscreen capture path
  (`capture_frame_indirect`).
- [ ] A heavily-masked region retains its specular contrast; absent/dropped masks still
  read fully lit.

## Wire format

`ShadowmaskAtlasSection` keeps its little-endian shape (16-byte-aligned header, then
`channels`, pad to 4, then the layer-major payload) and section id 42, extended by both
axes under one advanced format/PRL version:

- A `block_count: u32` (or an equivalent single integer from which
  `layer_count × block_count` is recovered) **and** a codec tag in the header. Raw
  `Rgba8Unorm` is retained as a codec-tag value so an uncompressed section stays
  representable. Empty selection encodes as today (`selected_light_count = 0`).
- `channels` entries range `0 .. (block_count * 4 − 1)` or the dropped sentinel; if the
  slot range can exceed a `u8`, widen the element type. Validation accepts the full slot
  range and rejects out-of-range non-sentinel values, replacing the current `<= 3 || 0xFF`
  gate.
- `data` is the tagged codec's block stream, layer-major over `layer_count × block_count`
  array layers. `from_bytes`' cross-check computes the expected length from the codec's
  block size × `width × height × (layer_count × block_count)` (raw = 4 bytes/texel; BC7 =
  16 bytes / 4×4 block; the BC4-per-channel fallback restructures the plane count too).

Field widths and the exact header slots are implementation choices; the constraints are
that both the block count and the codec tag round-trip and that the payload cross-check
matches the tagged codec. State in code that the layout mirrors the pre-block, raw
shadowmask section plus the block dimension and the codec tag.

## Boundary inventory

Two encodings cross module boundaries and are each pinned once. The **mask slot**
`s = block * 4 + channel` crosses Rust → wire → runtime f32 → shader; the dropped
sentinel is retained per surface, and every field must represent slots
`0 .. (block_count * 4 − 1)` plus the sentinel. The **payload encoding** crosses compiler
BC-encode → wire codec tag → renderer `TextureFormat` → shader sample.

| Crossing | Rust (compiler) | Wire / serde | Runtime | Shader |
|---|---|---|---|---|
| Mask slot | `channels` entry = slot, sentinel = dropped | `channels` bytes; header `block_count`; payload layer-major over `layer_count × block_count` | `SpecLight` shadowmask field and promoted `meta1.z` = `slot as f32`, `≥ sentinel` = none | decode `block = s / 4`, `channel = s % 4`; array layer `lightmap_layer + block * layer_count` |
| Payload encoding | BC-encode the raw atlas to the tagged codec's block stream | codec tag in header; `data` = block stream; cross-check by codec block size | `upload_shadowmask_texture` selects `TextureFormat` from the codec tag; D2Array view, LayerMajor upload, group-4 binding reused | samples the BC texture as `texture_2d_array<f32>` — transparent for BC7; BC4-per-channel changes the channel-select layout |

## Tasks

**Task 1 — combined format header + `(block, channel)` slot, thin vertical slice.**
Extend `ShadowmaskAtlasSection` per the Wire format: header `block_count` and codec tag
(raw retained), slot-encoded `channels`, layer-major payload over
`layer_count × block_count`, relaxed validation, advanced version, `from_bytes`
cross-check parameterized on the codec block size. Assign selected lights to slots so
≤4 overlap maps to block 0 (reproducing today's channels) and a fifth+ light spills to
block 1 rather than dropping — a minimal assignment for this slice; the full policy is
Task 2. Upload with `depth_or_array_layers = layer_count × block_count` and thread the
slot through the runtime linchpin into the `SpecLight` shadowmask field and the promoted
`meta1.z` (preserving the sentinel). In `forward.wgsl`, decode `(block, channel)`, sample
array layer `lightmap_layer + block × layer_count`, and select the channel, in both the
world-specular and promoted-union paths. Proven when a fixture texel overlapped by 5–8
selected lights shows every shadow at runtime — this falsifies the wire ↔ runtime ↔
shader boundary end to end. Encoding stays raw here; compression rides in on Task 3.

**Task 2 — deterministic block assignment + device-budget cap + graceful degradation.**
Replace the 4-color-with-drops assignment with a block-aware slot assignment: pack each
selected light into a `(block, channel)` slot so no two lights sharing a texel share a
slot, opening additional blocks as overlap demands, dropping a mask (lowest intensity)
only when opening another block would push `layer_count × block_count` past
`max_texture_array_layers`. Assignment is a pure, order-deterministic function of the
selection and per-light layer inputs so the section is byte-stable. Thread the same
array-layer bound the renderer enforces into the bake so the cap is enforced at bake time,
and extend `filter_usable_shadowmask_section` so an over-budget product degrades to the
all-visible placeholder with a `[Renderer]` error. Preserve the static→static double-count
dead-zone unchanged — the slot generalization changes mask lookup, not the union
subtraction. Track peak observed per-texel overlap, warn when the forced block count nears
the budget, and report peak overlap and final `block_count` under `--verbose`; a
comfortably-under-budget bake emits no new non-verbose line.

**Task 3 — BC encode at the pack seam + upload format branch + fidelity/determinism.**
At the shadowmask pack seam (after slot assignment, before section emit), BC-encode the
raw atlas to the tagged codec's block stream and write the codec tag; this is the emit-side
analogue of the direct-SH BC6H emit wrapper (see Path). The encoder must be
order-deterministic and pinned so re-bake is byte-identical. In the renderer, branch the
upload `TextureFormat` on the codec tag; the D2Array view, LayerMajor upload, and group-4
binding are otherwise reused, and for BC7 the shader is untouched while per-channel BC4
restructures the channel select into a plane index that composes with the block decode.
Use the codec the owner selects (Open questions; recommended per-channel BC4 via the
in-tree `bc5.rs` path for a self-contained, deterministic landing). Measure and report
per-channel error. First slice within this task: encode one fixture atlas to the chosen
codec, upload it, and A/B the specular capture against the raw baseline before wiring the
CPU-free.

**Task 4 — free the CPU payload after upload.** After `upload_shadowmask_texture` runs,
drop `LevelWorld.shadowmask_atlas.data`, retaining the `channels` clone. Confirm no other
consumer reads `.data` post-upload; a level reload re-reads a fresh world, so reuse is not
a consumer. Removes the RAM half of the double residency at zero quality cost.

**Task 5 — round-trip, shader, invariant, and compression coverage.** Lock the contract:
a `to_bytes`/`from_bytes` round-trip over a `block_count > 1`, compressed section (and
rejection of an out-of-range slot and a mismatched payload length under the codec's block
math); a shader test that a light on `block > 0` samples the correct layer and channel in
both decode paths; a bake test that a >4-overlap texel drops nothing below the budget and
lowest-intensity only past it; a double-count regression; the on-disk and VRAM byte-delta
tests; the CPU-free + reload test; the adapter-without-BC and over-budget placeholder
tests; and the byte-identical-compressed re-bake test. Update the `build_pipeline.md` id-42
line and the `rendering_pipeline.md` §4 world-specular statement at promotion to describe
the `(block, channel)` slots, the device-budget drop, and the at-rest codec.

## Path

- **BC6H encode seam (reuse-shape candidate, not a drop-in).** The compiler's in-tree BC
  encoders are `encode_bc6h_rgb_from_f32_rgba` (`crates/level-compiler/src/bc6h.rs`,
  BC6H Mode 11) and `bc5.rs` (BC5 normals); the emit-side wrapper that pads each axis to a
  4×4 multiple and concatenates per-layer blocks is `encode_direct_section_bc6h`
  (`crates/level-compiler/src/direct_sh_bake.rs`). Ground the domain before mandating reuse
  (`context_style_guide.md` §Spec Completeness): BC6H is an **HDR RGB, f16-internal**
  codec whose decode is `output_f16 = (interp * 31) >> 6` — it does **not** decode to unorm
  `[0,1]`, and the shadowmask needs a BC7/BC4 **unorm** encoder that is **not present
  in-tree** (`bc7-color-textures` draft flags the BC7 encoder as unresolved heavy work).
  So the reuse candidate is the *pattern and the emit seam* — an in-tree, dependency-free,
  min/max-endpoint, order-deterministic encoder plus `encode_direct_section_bc6h`'s
  pad-and-concat emit shape — not the BC6H function itself. The determinism-exemption
  precedent in `build_pipeline.md` §Build Cache (BC6H irradiance) is the analogue this
  brief deliberately does **not** take: id 42's codec is chosen deterministic.
- **Upload seam:** `upload_shadowmask_texture` and `filter_usable_shadowmask_section`
  (`crates/renderer/src/lighting/lightmap.rs`) select the `TextureFormat` from the codec
  tag; the D2Array view, LayerMajor upload, and group-4 binding are otherwise reused.
- **CPU release seam:** the atlas lives in `LevelWorld.shadowmask_atlas` stashed via
  `self.level = Some(world)` (`crates/postretro/src/startup/lifecycle.rs`); the renderer
  borrows it at install and clones only `channels`. Free the payload after upload without
  disturbing the `channels` clone.
- **Assignment seam:** the block-aware slot assignment generalizes the current
  `color_graph` / `assign_channels_with_drops` loop to `4 × block_count` colors,
  `block_count` grown on demand up to `floor(max_texture_array_layers / layer_count)`.
- **First slice:** the Task 1 thin vertical slice (raw encoding) proves the block boundary;
  the Task 3 BC first slice (encode one atlas, A/B the capture) falsifies the fidelity
  assumption before the format/version or CPU-free work hardens.
- Do not bake `stress-warren*` for proof: the compression ratio is codec-intrinsic and
  per-channel error is texel-local, so a small fixture with selected specular lights over a
  >4-overlap texel suffices for both axes.

## Sequencing

**Cross-plan dependency:** land `shadowmask-bake-scaling` first. That plan restructures the
shadowmask composite into a streaming membership → assignment → fill shape; this brief
changes the *assignment* step (slots instead of 4-color drops), adds a BC-encode phase at
the pack seam after slot assignment, and changes the emitted format. Building on the
streamed composite keeps the slot assignment a change to one well-scoped step.

**Phase 1:** Task 1 — combined-header thin slice (raw encoding); falsifies the wire ↔
runtime ↔ shader boundary. **Phase 2:** Task 2 — deterministic assignment, device-budget
cap, graceful degradation. **Phase 3:** Task 3 — BC encode at the pack seam (after slot
assignment) + upload format branch + fidelity/determinism. **Phase 4:** Task 4 — free CPU
payload. **Phase 5:** Task 5 — round-trip/shader/invariant + compression coverage.

## Invariants

| Invariant | Established by | Threatened at | Verified by |
|---|---|---|---|
| No selected mask dropped while `layer_count × block_count ≤ max_texture_array_layers` | Task 2 block-spilling assignment | any residual 4-slot cap in assignment, metadata, or shader | AC 1, 2 |
| Absent / rejected / over-budget / no-BC-adapter shadowmask → fully lit, no panic | existing `filter_usable_shadowmask_section`, extended | a missed layer bound is a device breach; a missing BC-feature check is a panic | AC 4, 11 |
| Static→static world shadowing exactly zero (pool-shadow union dead-zone) | existing promoted-union path | a slot or codec decode that alters the union term | AC 5 |
| Slot-assigned (pre-compression) id-42 bytes deterministic; compressed section length stable | Task 2 assignment (byte-identical) + Task 3 pinned encoder (length-stable) | non-deterministic block-open order; an encoder that changes section length across runs | AC 12 |
| CPU shadowmask payload released after upload; reload reinstalls | Task 4 | a post-upload `.data` reader; a reload that fails to re-read | AC 10 |

## Re-anchor before building

`lighting-scale--shadowmask-cold-working-set` (promoted to `ready/`) restructures the
assignment seam this brief targets: it deletes the per-(light, texel) membership record and
derives the overlap graph analytically, so the adjacency this brief consumes arrives on a
cheaper footing — but `assign_channels_with_drops` and the membership type its Task 2 slots
into both change shape. Re-anchor against the landed restructure before building. One
foreclosure to weigh: the deleted record is the only structure where per-texel visibility
values and cross-light adjacency coexist; intensity-ordered retention (which reads light
parameters only) is unaffected, but a contribution- or coverage-weighted retention priority
would have to re-materialize that term.

## Open questions

- Which codec is the default, given no in-tree BC7 encoder? — owner: developer —
  **blocks build**. BC7 (≈4:1, drop-in shader) requires building or vendoring a
  BC7 **unorm** encoder, or sequencing this brief after `bc7-color-textures` delivers one —
  a heavy addition or a cross-plan dependency. Per-channel BC4 (≈2:1, shader-touching)
  reuses the in-tree `bc5.rs` unorm encoder and lands self-contained. Recommendation:
  **default per-channel BC4** — 2:1 on the largest raw section is a real win, dependency-free
  and the semantic fit for four independent masks, and it ships now; treat BC7-4:1 as a
  follow-on that rides `bc7-color-textures`'s encoder. Owner decides ratio-vs-effort/dependency.
- Fidelity fallback within the chosen codec — **delegated**: the executor confirms the
  chosen codec passes the fidelity/visual ACs and reports the measured per-channel error in
  the plan of record; if the higher-ratio codec fails fidelity, it takes the lower-ratio
  one.
