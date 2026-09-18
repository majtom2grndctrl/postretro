# Shadowmask Atlas — Stacked Mask Planes and BC4 Compress-at-Rest

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
compression step for id 42), while id 42 alone stays raw. The four masks at a texel are
**independent single-channel visibility scalars**, not correlated colour — so the format
that fits them is a stack of single-channel **planes** (one mask per plane), and the codec
that fits them is per-channel **BC4** (its home case). Evolving the header to carry both a
**plane count** (more mask slots per texel) and a **codec tag** (raw `R8` or BC4) closes
both gaps under one version bump.

## Decisions

- **Per-channel BC4 at rest, ≈2:1, decided — not a spike.** The masks are four independent
  single-channel scalars used as a specular multiplier, spatially smooth, with a fully-lit
  fallback (`rendering_pipeline.md` §4 World specular shadowmask). BC4 (two min/max endpoints
  + 3-bit indices per 4×4 block, single channel) is built for exactly that — it is the
  correct codec by the data's structure, not a compromise. BC7 was rejected here: it models a
  cross-channel block correlation four independent masks do not have (its weak case) and would
  spend ~2 bits/channel vs BC4's 4, so it is both structurally wrong and lower per-channel
  precision — its only edge, ≈4:1, comes from that mismatch. The GPU decompresses BC4 in
  hardware, so disk, CPU-load RAM, and VRAM all shrink ≈2:1 together. No fidelity gate is
  needed to *choose* the codec; a measured error report plus a visual regression check confirm
  the result (Acceptance). BC7 belongs to `bc7-color-textures` (correlated sRGB colour, where
  BC7 is the right tool) — this brief neither builds nor blocks on a BC7 encoder.
- **Independent masks are single-channel planes; the `(block, channel)` slot collapses to one
  plane index.** With BC4 there is no RGBA texel to pack into, so a texel's k-th mask is simply
  plane *k*. The atlas becomes a single-channel (`R8`/BC4) `texture_2d_array`; the mask for
  plane *p* lives at array layer `lightmap_layer + p × layer_count`. The per-selection table
  stores a flat **plane index** (dropped sentinel preserved), and the shader samples one array
  layer's `.r` — no RGBA channel select (the old `shadowmask_channel_value` disappears). A
  texel carries `plane_count` masks; >4 overlap is carried by more planes.
- **Drop ceiling: `plane_count × layer_count ≤ max_texture_array_layers` (portable 256).**
  Single-channel planes spend array layers faster than RGBA packing did — one mask per layer,
  not four — so the ceiling is `floor(256 / layer_count)` masks/texel, roughly a quarter of
  what an RGBA-packed block layout would allow. Accepted: it is still materially more than four
  on realistic layer counts, >4 overlap is rare and unmeasured on today's content (a
  build-ahead lift, not an observed defect), and the single-index single-sample layout is
  simpler than four bindings. A mask is dropped (lowest-intensity, as today) only when adding a
  plane would exceed the budget. If a real map ever pushes the ceiling, a channel-grouped
  layout (four BC4 planes sampled per group, preserving density at the cost of bindings) is the
  escape hatch — noted in Path, not built.
- **Reuse the in-tree `bc5.rs` unorm encoder; no new encoder.** BC4 is one channel of the
  existing deterministic BC5 path (`crates/level-compiler/src/bc5.rs`), with the pad-to-4×4 +
  per-layer-concat emit shape of `encode_direct_section_bc6h`
  (`crates/level-compiler/src/direct_sh_bake.rs`). This lands self-contained — no cross-plan
  dependency, no ISPC-texcomp-class encoder to build or vendor.
- **No new device requirement or alignment constraint.** id 22/35 already require the
  `TEXTURE_COMPRESSION_BC` adapter feature, so BC4 adds none. BC is 4×4-block; the shadowmask
  shares the lightmap atlas dimensions, already BC-aligned because id 22 is BC6H — no padding
  introduced.
- **Determinism, aligned to the cache invariant.** The plane assignment must be a pure,
  order-deterministic function of the selection and per-light layer inputs, so the logical
  (pre-compression) atlas re-bakes byte-identically — required and precedent-aligned. The BC4
  encode is deterministic by construction (the `bc5.rs` min/max-endpoint path), so the section
  re-bakes byte-identically in practice; but per `build_pipeline.md` §Build Cache (the BC6H
  irradiance exemption — the cache keys on inputs, not outputs) only **section-length
  stability** is a hard requirement, not byte-identity of the lossy bytes.
- **Free the CPU payload after upload.** Drop `LevelWorld.shadowmask_atlas.data` (retaining
  the tiny plane table the renderer clones) once `upload_shadowmask_texture` has run, removing
  the RAM half of the double residency. The executor confirms no other consumer reads `.data`
  post-upload before freeing it; level reload re-reads from a fresh world, so reuse is not a
  consumer.
- **Preserve graceful-degradation and double-count unchanged, extended.** Absent, rejected, or
  over-budget shadowmask data still resolves to fully lit via `filter_usable_shadowmask_section`
  → the placeholder; the static→static union dead-zone (`rendering_pipeline.md` §4 pool-shadow
  bias) is untouched. Extend the placeholder path to two new causes: `plane_count × layer_count`
  over the device budget, and an adapter lacking `TEXTURE_COMPRESSION_BC` — both degrade to the
  all-visible placeholder, never a panic.
- **Placement.** A runtime-format capacity-and-encoding change spanning the at-rest wire
  format, the compiler's plane assignment and BC4 encode, the renderer upload/metadata, and the
  shader — each edited at its own boundary, with the plane index and codec tag pinned once
  (Boundary inventory). Planes stack in the existing `texture_2d_array`; the GPU stays
  renderer-owned; the whole atlas is resident (no residency or streaming machinery here).

### Non-goals

- **Streaming / visibility-driven residency of the atlas.** Deferred to the in-progress
  `sh-probe-streaming` cell-cluster substrate, which builds a resource-agnostic
  cluster-of-cells residency layer on the visible-cell signal and names lightmap-layer /
  shadowmask as future subscribers but wires only SH (id 49 cluster directory reserved,
  unemitted). Compression composes with, and does not foreclose, that work (re-bakeable into
  cluster-addressable compressed form).
- **Material / texture residency.** A separate material+mip domain, explicitly excluded from
  the spatial substrate by `sh-probe-streaming`.
- **A BC7 encoder / BC7 on colour textures.** `bc7-color-textures` owns BC7 — the right tool
  for correlated sRGB colour, with its own mip chain, magnification aesthetic gate, and
  `emissive-surfaces-bloom` dependency. This brief does not build a BC7 encoder; the shadowmask
  is not BC7's data.
- **Sparse / empty-region footprint trim.** Subsumed by the streaming epic above.
- **The shadowmask bake's memory / parallelism / progress.** Owned by `shadowmask-bake-scaling`
  (landed); this brief changes what the bake emits and how masks are assigned to planes and
  encoded, not the bake's allocation lifecycle.
- **The lightmap / irradiance / direction atlases.** Untouched; `lightmap_layer` (the
  receiver's spatial array layer) keeps its meaning. Only the shadowmask atlas gains the plane
  dimension and the codec tag. The "shadowmask dims == lightmap dims" invariant is unchanged
  (BC preserves dimensions).
- **Old-`.prl` migration.** All fixtures re-bake from source; the format version advances and
  stale caches regenerate.
- **Raising the per-texel cap above the device array-layer budget.** Past
  `plane_count × layer_count = 256` the existing lowest-intensity drop is retained as the
  graceful fallback — a chosen, owner-visible ceiling.
- **Selection eligibility / ranking.** Which lights are selected (`entity_shadow_select`) is
  unchanged.

## Acceptance

### Automated
- [ ] On a fixture with a lightmap texel overlapped by more than four selected static lights
  (6–8), every selected light receives a non-dropped plane and its shadow is present at
  runtime — no mask dropped below the device array-layer budget.
- [ ] A mask is dropped only when adding its plane would push `plane_count × layer_count` past
  `max_texture_array_layers`; at or below the budget, no drop. The drop, when it happens, is
  the lowest-intensity mask, matching the pre-change policy.
- [ ] The plane assignment is collision-free: for every overlap edge in the selection graph,
  the two lights receive different planes (or one is the dropped sentinel) — a unit assertion
  over the assignment output, independent of any rendered texel.
- [ ] The no-drop-below-budget property is proven on a fixture large enough to exercise the
  budget-exhausted greedy fallback (not only the exact-search path): the greedy path also
  grows to `plane_count` planes and drops only where the Decisions' drop rule permits (see the
  search-budget carve-out, if the owner keeps that fallback).
- [ ] `ShadowmaskAtlasSection::to_bytes` → `from_bytes` round-trips the plane table, the codec
  tag, and the layer-major payload for `plane_count > 1`; `from_bytes` rejects an out-of-range
  plane index and a payload whose length disagrees with the codec's per-plane block size ×
  `width × height × layer_count × plane_count`.
- [ ] Round-trip holds at the format edges: `plane_count == 1`; an empty selection
  (`selected_light_count == 0`, `plane_count == 0`); and both codec tags (raw `R8` and BC4) —
  each `to_bytes` → `from_bytes` reproduces the header, plane table, and payload.
- [ ] A section whose `plane_count × layer_count` exceeds the device budget is rejected by
  `filter_usable_shadowmask_section` with a `[Renderer]` error and the all-visible placeholder
  (fully lit), no panic.
- [ ] Boundary: a section with `plane_count × layer_count == max_texture_array_layers` is
  retained (no drop, no degrade); `== max_texture_array_layers + 1` is rejected to the
  all-visible placeholder. `filter_usable_shadowmask_section` compares the
  `layer_count × plane_count` product, not `layer_count` alone.
- [ ] A sentinel (dropped) plane index reads fully lit in both decode paths (world-specular
  and promoted-union); and a baked plane `> 0` index samples fully lit (not out-of-range) when
  the atlas is the 1-layer all-visible placeholder — the layer `lightmap_layer + plane × layer_count`
  is clamped to the bound texture's last layer.
- [ ] Static→static world shadowing stays exactly zero: adding a light that lands on a plane
  `> 0` does not change world-specular output for surfaces already covered by plane-0 lights
  (the pool-shadow union dead-zone is unaffected).
- [ ] Both shader decode paths (world-specular, promoted-union) sample the correct array layer
  `lightmap_layer + plane × layer_count` for a light on any plane; shader tests covering
  `plane > 0` pass.
- [ ] Source-inspection gate: `forward.wgsl` contains no RGBA channel-select for the shadowmask
  (`shadowmask_channel_value` and the `mask.r/.g/.b/.a` switch are gone) and does contain the
  single-layer formula `lightmap_layer + plane × layer_count`, in both decode paths — proving
  the 4-channel path was removed, not shadowed by a new one.
- [ ] (Control, not a fix gate) ≤4 overlap carries every mask with no drop — a regression guard
  for the common case; the defect-catching rows are the 5–8-overlap and collision-free rows
  above.
- [ ] On a focused fixture carrying a populated atlas, the id-42 on-disk section byte count
  drops ≈2:1 versus the raw single-channel-plane (`R8`) baseline, measured by the per-section
  byte accounting in `pack.rs`.
- [ ] The renderer uploads id 42 as the BC4 texture and the computed resident byte count
  (`width × height × layer_count × plane_count × bytes_per_block(codec)`) drops ≈2:1 R8→BC4
  (optionally logged like the animated atlas' estimate); an all-visible (255) atlas round-trips
  to fully lit after encode→decode.
- [ ] After upload, the shadowmask CPU payload is released — the level holds no
  `width × height × layer_count × plane_count` shadowmask buffer resident — and a subsequent
  level reload still installs a correct atlas.
- [ ] An adapter without `TEXTURE_COMPRESSION_BC` falls back to the all-visible placeholder
  (fully lit), no panic.
- [ ] Re-baking the same fixture twice yields a byte-identical plane-assigned (pre-compression)
  atlas and a section-length-stable compressed id-42 section, so the build cache stays valid.
  Byte-identity holds across differing worker-thread counts, and the plane-open order is a pure
  function of a stable ordering key (selection index / per-light layer), not chart-worker or
  iteration order.
- [ ] Fidelity, measure-and-report: the max and mean per-channel absolute error of the BC4
  encode versus the raw masks, on the fixture, recorded in the landing note.

### Manual
- [ ] The authoring signal is preserved: the bake logs a warning when overlap forces the plane
  count near the device array-layer budget (a spot over-piled with lights), and reports the peak
  observed per-texel overlap and the resulting `plane_count` under `--verbose`. A non-verbose
  bake with comfortable headroom gains no new log spam.
- [ ] In a scene with selected non-SDF static world specular lights, specular highlights and
  their shadowmask-occluded regions read unchanged against a raw-atlas capture. Reuse the
  offscreen capture path (`capture_frame_indirect`).
- [ ] A heavily-masked region retains its specular contrast; absent/dropped masks still read
  fully lit.

## Wire format

`ShadowmaskAtlasSection` keeps its little-endian shape (16-byte-aligned header, then the
per-selection table, pad to 4, then the layer-major payload) and section id 42, restructured to
single-channel planes under one advanced format/PRL version:

- A `plane_count: u32` (or an equivalent single integer from which `layer_count × plane_count`
  is recovered) **and** a codec tag in the header. The codec tag distinguishes raw `R8Unorm`
  from `Bc4RUnorm`; raw stays representable. Empty selection encodes as today
  (`selected_light_count = 0`).
- The per-selection table stores a flat **plane index** (widen the element type if the plane
  range can exceed a `u8`) or the dropped sentinel. Validation accepts the full plane range and
  rejects out-of-range non-sentinel values, replacing the current `<= 3 || 0xFF` gate.
- `data` is the tagged codec's single-channel block stream, layer-major over
  `layer_count × plane_count` array layers. `from_bytes`' cross-check computes the expected
  length from the codec's per-plane texel/block size × `width × height × (layer_count × plane_count)`
  (raw `R8` = 1 byte/texel; BC4 = 8 bytes / 4×4 block).

Field widths and the exact header slots are implementation choices; the constraints are that
the plane count and codec tag round-trip and that the payload cross-check matches the tagged
codec.

## Boundary inventory

Two encodings cross module boundaries and are each pinned once. The **plane index** crosses
Rust → wire → runtime f32 → shader; the dropped sentinel is retained per surface, and every
field must represent planes `0 .. (plane_count − 1)` plus the sentinel. The **payload encoding**
crosses compiler BC4-encode → wire codec tag → renderer `TextureFormat` → shader sample.

| Crossing | Rust (compiler) | Wire / serde | Runtime | Shader |
|---|---|---|---|---|
| Plane index | per-selection table entry = plane, sentinel = dropped | table bytes; header `plane_count`; payload layer-major over `layer_count × plane_count` | `SpecLight` shadowmask field and promoted `meta1.z` = `plane as f32`, `≥ sentinel` = none | array layer `lightmap_layer + plane × layer_count`; sample `.r` (no channel select) |
| Payload encoding | BC4-encode each single-channel plane via the `bc5.rs` unorm path | codec tag in header (`R8`/BC4); `data` = block stream; cross-check by codec block size | `upload_shadowmask_texture` selects `TextureFormat` (`R8Unorm`/`Bc4RUnorm`) from the codec tag; D2Array view, LayerMajor upload, group-4 binding reused | samples the single-channel BC4 texture as `texture_2d_array<f32>` (BC4 decodes to `.r`) |

## Tasks

**Task 1 — plane-restructured format header + plane index, thin vertical slice.** Extend
`ShadowmaskAtlasSection` per the Wire format: header `plane_count` and codec tag (raw `R8`
retained), plane-indexed per-selection table, single-channel layer-major payload over
`layer_count × plane_count`, relaxed validation, advanced version, `from_bytes` cross-check
parameterized on the codec's per-plane size. Assign selected lights to planes so ≤4 overlap
occupies planes 0–3 and a fifth+ light spills to plane 4 rather than dropping — a minimal
assignment for this slice; the full policy is Task 2. Upload with
`depth_or_array_layers = layer_count × plane_count` and thread the plane index through the
runtime linchpin into the `SpecLight` shadowmask field and the promoted `meta1.z` (preserving
the sentinel). In `forward.wgsl`, sample array layer `lightmap_layer + plane × layer_count` and
read `.r`, in both the world-specular and promoted-union paths, deleting the RGBA channel
select. Proven when a fixture texel overlapped by 5–8 selected lights shows every shadow at
runtime — this falsifies the wire ↔ runtime ↔ shader boundary end to end. Encoding stays raw
`R8` here; BC4 rides in on Task 3.

**Task 2 — deterministic plane assignment + device-budget cap + graceful degradation.** Replace
the 4-color-with-drops assignment with a plane assignment: give each selected light a plane so
no two lights sharing a texel share a plane, opening additional planes as overlap demands,
dropping a mask (lowest intensity) only when adding a plane would push `plane_count × layer_count`
past `max_texture_array_layers`. Assignment is a pure, order-deterministic function of the
selection and per-light layer inputs so the section is byte-stable. Thread the same array-layer
bound the renderer enforces into the bake so the cap is enforced at bake time, and extend
`filter_usable_shadowmask_section` so an over-budget product degrades to the all-visible
placeholder with a `[Renderer]` error. Preserve the static→static double-count dead-zone
unchanged — the plane generalization changes mask lookup, not the union subtraction. Track peak
observed per-texel overlap, warn when the forced plane count nears the budget, and report peak
overlap and final `plane_count` under `--verbose`; a comfortably-under-budget bake emits no new
non-verbose line.

**Task 3 — BC4 encode at the pack seam + upload format branch.** At the shadowmask pack seam
(after plane assignment, before section emit), BC4-encode each single-channel plane via the
`bc5.rs` unorm path (one channel of the deterministic BC5 encoder) with the pad-and-concat emit
shape of `encode_direct_section_bc6h`, and write the codec tag; the section is length-stable
across re-bakes. In the renderer, branch the upload `TextureFormat` (`R8Unorm`/`Bc4RUnorm`) on
the codec tag; the D2Array view, LayerMajor upload, and group-4 binding are otherwise reused;
BC4 decodes to `.r`, which the Task 1 shader already reads. Measure and report per-channel error.

**Task 4 — free the CPU payload after upload.** After `upload_shadowmask_texture` runs, drop
`LevelWorld.shadowmask_atlas.data`, retaining the plane-table clone. Confirm no other consumer
reads `.data` post-upload; a level reload re-reads a fresh world, so reuse is not a consumer.
Removes the RAM half of the double residency at zero quality cost.

**Task 5 — round-trip, shader, invariant, and compression coverage.** Lock the contract: a
`to_bytes`/`from_bytes` round-trip over a `plane_count > 1`, BC4 section (and rejection of an
out-of-range plane and a mismatched payload length under the codec's block math); a shader test
that a light on plane `> 0` samples the correct layer in both decode paths; a bake test that a
>4-overlap texel drops nothing below the budget and lowest-intensity only past it; a double-count
regression; the on-disk and VRAM byte-delta tests; the CPU-free + reload test; the
adapter-without-BC and over-budget placeholder tests; the BC4 round-trip error-bound test; and
the deterministic-plane / length-stable re-bake test. Update the `build_pipeline.md` id-42 line
and the `rendering_pipeline.md` §4 world-specular statement at promotion to describe the
single-channel planes, the device-budget drop, and the at-rest BC4 codec.

## Path

- **BC4 encode — reuse the `bc5.rs` unorm path.** BC4 is one channel of the in-tree BC5 encoder
  (`crates/level-compiler/src/bc5.rs`, dependency-free, deterministic min/max endpoints); the
  pad-to-4×4 + per-layer-concat emit wrapper to mirror is `encode_direct_section_bc6h`
  (`crates/level-compiler/src/direct_sh_bake.rs`). Ground the domain before mandating reuse
  (`context_style_guide.md` §Spec Completeness): confirm the BC5 path exposes (or trivially
  yields) a single-channel BC4 block and that its index/endpoint precision holds a smooth
  `[0,1]` mask within the fidelity bound.
- **Upload seam:** `upload_shadowmask_texture` and `filter_usable_shadowmask_section`
  (`crates/renderer/src/lighting/lightmap.rs`) select the `TextureFormat` from the codec tag;
  the D2Array view, LayerMajor upload, and group-4 binding are otherwise reused.
- **CPU release seam:** the atlas lives in `LevelWorld.shadowmask_atlas` stashed via
  `self.level = Some(world)` (`crates/postretro/src/startup/lifecycle.rs`); the renderer borrows
  it at install and clones only the plane table. Free the payload after upload without disturbing
  that clone.
- **Assignment seam:** the plane assignment generalizes the current `color_graph` /
  `assign_channels_with_drops` loop from 4 colours to `plane_count` planes, grown on demand up to
  `floor(max_texture_array_layers / layer_count)`.
- **Ceiling escape hatch (only if a real map binds it):** the single-channel-plane layout caps
  masks/texel at `floor(256 / layer_count)`. If that ever binds, a channel-grouped layout (four
  BC4 planes per group, `.rgba`-style sampling, preserving RGBA-era density at four bindings) is
  the fallback — do not build it pre-emptively.
- **First slice:** the Task 1 thin vertical slice (raw `R8` encoding) proves the plane boundary;
  Task 3 swaps the encoding to BC4 and A/Bs the specular capture before the CPU-free hardens.
- Do not bake `stress-warren*` for proof: the compression ratio is codec-intrinsic and
  per-channel error is texel-local, so a small fixture with selected specular lights over a
  >4-overlap texel suffices for both axes.

## Sequencing

**Cross-plan dependency (satisfied):** `shadowmask-bake-scaling` has landed (`done/`); it
restructured the shadowmask composite into a streaming membership → assignment → fill shape.
Build on that landed composite: this brief changes the *assignment* step (planes instead of
4-color drops), adds the BC4-encode phase at the pack seam after plane assignment, and changes
the emitted format — the composite restructure it depended on is already in place.

**Phase 1:** Task 1 — plane-restructured header thin slice (raw `R8`); falsifies the wire ↔
runtime ↔ shader boundary. **Phase 2:** Task 2 — deterministic plane assignment, device-budget
cap, graceful degradation. **Phase 3:** Task 3 — BC4 encode at the pack seam + upload format
branch. **Phase 4:** Task 4 — free CPU payload. **Phase 5:** Task 5 — round-trip/shader/invariant
+ compression coverage.

## Invariants

| Invariant | Established by | Threatened at | Verified by |
|---|---|---|---|
| No selected mask dropped while `plane_count × layer_count ≤ max_texture_array_layers` | Task 2 plane-spilling assignment | any residual 4-slot cap in assignment, metadata, or shader | AC 1, 2 |
| Absent / rejected / over-budget / no-BC-adapter shadowmask → fully lit, no panic | existing `filter_usable_shadowmask_section`, extended | a missed layer bound is a device breach; a missing BC-feature check is a panic | AC 4, 11 |
| Static→static world shadowing exactly zero (pool-shadow union dead-zone) | existing promoted-union path | a plane or codec decode that alters the union term | AC 5 |
| Plane-assigned (pre-compression) id-42 bytes deterministic; compressed section length stable | Task 2 assignment (byte-identical) + Task 3 deterministic BC4 encode (length-stable) | non-deterministic plane-open order; an encoder that changes section length across runs | AC 12 |
| CPU shadowmask payload released after upload; reload reinstalls | Task 4 | a post-upload `.data` reader; a reload that fails to re-read | AC 10 |

## Re-anchor before building

`lighting-scale--shadowmask-cold-working-set` has landed (`done/`) and restructured the
assignment seam this brief targets: it deletes the per-(light, texel) membership record and
derives the overlap graph analytically, so the adjacency this brief consumes arrives on a
cheaper footing — but `assign_channels_with_drops` and the membership type its Task 2 planes
into both changed shape. Re-anchor against the landed restructure before building. One
foreclosure to weigh: the deleted record is the only structure where per-texel visibility
values and cross-light adjacency coexist; intensity-ordered retention (which reads light
parameters only) is unaffected, but a contribution- or coverage-weighted retention priority
would have to re-materialize that term.
