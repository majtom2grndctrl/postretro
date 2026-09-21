# Shadowmask Atlas — BC5 Compress-at-Rest

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4,
`context/lib/build_pipeline.md` §PRL · §Build Cache, `context/lib/testing_guide.md`
§Resource bounds

This brief changes bytes, never masks. Per-texel mask capacity is
`shadowmask-atlas-mask-capacity`, a separate brief gated on a measurement this one
takes.

## Problem

PRL id 42 (`ShadowmaskAtlasSection`) is the only baked atlas still stored raw. ids
22 and 35 ship BC-compressed; the pack seam has no compression step for id 42, so
its payload sits uncompressed on disk, in CPU RAM for the level lifetime, and in
VRAM. The masks are independent single-channel `[0,1]` visibility scalars, spatially
smooth — BC4's home case, and BC5's, since BC5 *is* two BC4 blocks in one 16-byte
block.

**No id-42 byte figure exists anywhere in the repo.** `sh-base-atlas-at-rest-slimming`
measured ids 34, 35, 27, 41 and 45 on `campaign-test` and omitted 42. We have a
ratio and no magnitude. Task 1 is that measurement, and it gates nothing — it is one
bake, and it is how the landing note states what this brief actually bought.

## Decisions

- **BC5 `.rg` pairs at a group count fixed at two — exactly four masks per texel.**
  Two array-layer groups, two masks each: today's four, unchanged. This brief makes
  **no capacity claim**. That is what keeps it small — no capacity floor, no raw
  fallback codec, no second shader decode path, no widened slot index, and no
  dependence on the unmeasured overlap frequency.
- **The slot table is unchanged.** Four slots still fit `0..3` with
  `SHADOWMASK_CHANNEL_DROPPED = 0xFF` as the sentinel, exactly as today. Only the
  *meaning* changes: slot `s` addresses group `s / 2` and channel `s % 2` rather than
  an RGBA channel. Widening the element is `shadowmask-atlas-mask-capacity`'s
  problem, not this brief's.
- **The sample stays hoisted, and control flow stays uniform.** With the group count
  fixed at two, both array layers are known from `lightmap_layer` alone, so both
  samples hoist out of the light loops exactly as the single sample does today; the
  four masks arrive as `(g0.r, g0.g, g1.r, g1.g)` and the per-light select is the
  same four-way shape as the current `shadowmask_channel_value`. This costs **two
  hoisted samples per fragment instead of one** — a fixed doubling on fragments that
  use the atlas, independent of light count — and it costs nothing else. It is only
  once groups grow on demand that the sample must move inside the loops, land in
  non-uniform control flow, and force the explicit-level sampler; that whole cluster
  of consequences belongs to the capacity brief and is absent here.
- **A self-describing format tag in the header.** Mirrors
  `LightmapSection::direction_format`, the house pattern for exactly this. Raw
  `Rgba8Unorm` stays representable under its own tag value so the encode is A/B-able
  against the raw baseline. The capacity brief adds tag values additively.
- **Above the array-layer budget, degrade — do not fall back to raw.** Two groups
  need `2 × layer_count ≤ 256`, so a level past 128 lightmap layers cannot seat the
  atlas. It degrades to the all-visible placeholder through
  `filter_usable_shadowmask_section`, the path `rendering_pipeline.md` §4 already
  commits to for rejected data. No raw-fallback codec, and therefore no permanent
  second decode path in the shader. That band is what the capacity brief reopens.
- **Byte-identical output, not merely length-stable.**
  `static-light-shadowmask-cache-addendum` ships a live guarantee that cached warm
  output for this section matches uncached output byte-for-byte, and `bc5.rs` is a
  pure function — per-block min/max endpoints, a fixed integer palette matching the
  hardware ladder, nearest-index selection, no cluster-fit refinement, no
  parallelism. The BC6H lossy exemption in `build_pipeline.md` §Build Cache is
  therefore available but **not taken**: taking it would surrender a satisfied
  guarantee for nothing.
- **Free the CPU payload after upload.** Drop the loaded level's shadowmask payload
  once the upload has run, retaining the small slot table the renderer clones. No
  consumer reads it post-upload; a level reload re-reads a fresh world, so reuse is
  not a consumer. Removes the RAM half of the double residency at zero quality cost.
- **No new device requirement, no new alignment.** `TEXTURE_COMPRESSION_BC` is
  already a hard init requirement — the renderer bails at device acquisition without
  it — so BC5 adds none, and an adapter-lacking-BC acceptance row would be
  unreachable. `round_atlas_dim` guarantees the lightmap atlas is power-of-two and at
  least 64 per axis and names BC block alignment as the reason; the shadowmask shares
  those dimensions, so **no padding is introduced**, unlike the direct-SH path.
- **Measure peak per-texel overlap under `--verbose`.** This brief does not act on
  it. It is the input the capacity brief needs and currently lacks, and taking it
  here costs a counter in a loop the bake already runs.

### Non-goals

- **Per-texel mask capacity above four.** `shadowmask-atlas-mask-capacity`. Greater-than-four
  overlap is rare and unmeasured on today's content — a build-ahead lift, not a
  reported defect — so it must not gate a win that is certain.
- **A BC7 encoder.** `bc7-color-textures` owns BC7. BC7 models a per-block
  cross-channel correlation four independent masks do not have.
- **Streaming or visibility-driven residency.** `large-map-spatial-residency`.
  Compression composes with it — bytes per resident texel is orthogonal to which
  texels are resident.
- **The lightmap array-consolidation refactor.** Only the capacity brief needs a
  binding slot.
- **Selection eligibility and ranking.** Unchanged.
- **Old-`.prl` migration.** Fixtures re-bake; the section version advances and stale
  caches regenerate.

## Acceptance

### Automated

- [ ] `to_bytes` → `from_bytes` round-trips the header, format tag, slot table, and
      payload under both tags, at the format edges: an empty selection, a single
      selected light, and a fully-populated four-slot table.
- [ ] `from_bytes` rejects a payload whose length disagrees with the tagged format's
      arithmetic — raw at 4 bytes per texel over `layer_count` layers, BC5 at 16
      bytes per 4×4 block over `2 × layer_count` layers — and rejects a slot index
      that is neither `0..3` nor the sentinel.
- [ ] A dropped-sentinel slot reads fully lit, and a baked slot in the second group
      reads fully lit rather than sampling out of range when the bound texture is the
      one-layer all-visible placeholder.
- [ ] A section whose `2 × layer_count` exceeds the engine's pinned array-layer
      maximum is rejected to the all-visible placeholder with a `[Renderer]` error
      and no panic. The filter compares the product, not `layer_count` alone.
- [ ] Boundary: `2 × layer_count` exactly equal to that maximum is retained; one
      layer greater degrades to the placeholder.
- [ ] Four overlapping selected lights carry every mask with no drop — the capacity
      this brief must not regress.
- [ ] Both decode paths resolve a light in either group to the correct mask, and the
      shadowmask sample remains hoisted outside both light loops — a source-inspection
      gate, since a sample that migrated into the loops is the regression this brief's
      shape exists to avoid.
- [ ] Static→static world shadowing stays exactly zero: moving a light from the first
      group to the second does not change world-specular output for surfaces already
      covered by first-group lights.
- [ ] An all-visible (255) atlas round-trips to fully lit through encode and decode.
- [ ] On a fixture carrying a populated atlas, the id-42 on-disk section byte count
      drops ≈2:1 against the raw baseline, measured by the per-section byte accounting
      at the pack seam.
- [ ] The renderer uploads the section in its compressed format and the computed
      resident byte count drops ≈2:1 against the raw baseline.
- [ ] After upload, no `width × height × layer_count × 2` shadowmask buffer remains
      resident on the CPU, and a subsequent level reload still installs a correct
      atlas.
- [ ] Re-baking a fixture twice yields a byte-identical section — assignment and
      compressed payload alike — across differing worker-thread counts, so the
      existing cached-warm-equals-uncached guarantee for this section survives.
- [ ] Frame time on the world specular path does not regress measurably against a
      pre-change baseline, on a scene whose fragments carry several selected static
      specular lights. The expected cost is one extra hoisted sample per fragment,
      fixed and independent of light count.
- [ ] Fidelity, measured and reported rather than gated: max and mean per-channel
      absolute error of the BC5 encode against the raw masks, recorded in the landing
      note.

### Manual

- [ ] The id-42 section byte count on a representative map is recorded in the landing
      note, both before and after — the magnitude no plan in this repo currently has.
- [ ] In a scene with selected non-SDF static world specular lights, specular
      highlights and their shadowmask-occluded regions read unchanged against a
      raw-atlas capture through the offscreen capture path. Look at both images: a
      distribution check passes on an image that has lost its contrast.
- [ ] The bake reports peak observed per-texel overlap under `--verbose`, alongside
      layer count and selected format. A non-verbose bake gains no new line.

## Wire format

`ShadowmaskAtlasSection` keeps little-endian encoding, a 16-byte-aligned header, the
per-selection table padded to 4, then the payload, and section id 42, under an
advanced per-section version.

- The header gains one **format tag**, mirroring `LightmapSection::direction_format`.
  Two values: raw `Rgba8Unorm` (today's layout, retained for the A/B baseline) and
  BC5 `.rg`. No group-count field — the group count is two under the BC5 tag and one
  under raw, both implied by the tag. The capacity brief adds a group count when it
  needs one.
- The per-selection table is **unchanged**: one byte per selected light, `0..3`, with
  `0xFF` as the dropped sentinel. Under the raw tag slot `s` is the RGBA channel at
  array layer `lightmap_layer`, as today. Under the BC5 tag slot `s` addresses group
  `s / 2` and channel `s % 2`, at array layer `lightmap_layer + (s / 2) × layer_count`.
- `data` is the tagged format's stream, layer-major over `layer_count` array layers
  under raw and `2 × layer_count` under BC5. `from_bytes` computes expected length
  from the tag — 4 bytes per texel, or 16 bytes per 4×4 block — and rejects a
  disagreement.

Field widths and exact header slots are implementation choices. The binding
constraints are that the tag round-trips, that the payload cross-check matches the
tagged format, and that the slot table's existing sentinel semantics are preserved.

## Tasks

**Task 1 — measure the baseline.** Bake a representative map and record the id-42
on-disk section byte count, alongside its lightmap `layer_count` and atlas
dimensions, through the per-section byte accounting that already exists at the pack
seam. This is the magnitude no plan in this repo has. It gates nothing; it is what
makes the landing note say something true about what was bought.

**Task 2 — format tag and BC5 encode, thin vertical slice.** Add the header format
tag and the tagged payload cross-check; reinterpret the slot table as
`(group, channel)` under the BC5 tag; BC5-encode each group's two mask channels at
the pack seam through `encode_bc5_rg`; branch the upload texture format on the tag
and size the array dimension to `2 × layer_count`; in `forward.wgsl`, sample both
groups' array layers hoisted outside the light loops and select the mask by
`(group, channel)` in both the world-specular and promoted-union paths. Preserve the
hoist and the uniform control flow it rests on — the whole reason this brief fixes
the group count at two is that both layers are known from `lightmap_layer` alone, and
a sample that migrates into the loops silently reintroduces the cost this shape
avoids. Extend `filter_usable_shadowmask_section` to compare `2 × layer_count`
against the array-layer maximum rather than `layer_count` alone. Proven when a
fixture texel overlapped by four selected lights spread across both groups shows
every shadow at runtime, which falsifies the wire, runtime, shader, and codec
boundaries together.

**Task 3 — free the CPU payload after upload.** Drop the loaded level's shadowmask
payload once `upload_shadowmask_texture` has run, retaining the slot-table clone.
Establish that no consumer reads it post-upload before freeing it.

**Task 4 — overlap instrumentation.** Track peak observed per-texel overlap during
the bake and report it under `--verbose` with the layer count and selected format.
Do not act on it and do not warn without `--verbose`. This is the capacity brief's
missing premise.

**Task 5 — coverage and durable capture.** Lock the round-trip, rejection, boundary,
degradation, determinism, double-count, reload, byte-delta and frame-time rows above,
plus the shader test that a second-group light resolves correctly in both decode
paths. Revise the `build_pipeline.md` id-42 line and the `rendering_pipeline.md` §4
world-specular statement for the tagged format and group addressing at a fixed group
count of two.

## Sequencing

**Phase 1 (sequential):** Task 1 — the baseline measurement, before the thing it
measures changes.
**Phase 2 (sequential):** Task 2 — thin slice; falsifies wire ↔ runtime ↔ shader ↔
codec together.
**Phase 3 (concurrent):** Task 3, Task 4 — independent; different files, no shared
contract.
**Phase 4 (sequential):** Task 5 — consumes every prior surface.

## Invariants

| Invariant | Established by | Threatened at | Verified by |
|---|---|---|---|
| Per-texel mask capacity stays exactly four — this brief trades bytes, never masks | Task 2 fixed group count | a group count that varies, or a slot table reinterpreted wider | AC 6 |
| The shadowmask sample stays hoisted, control flow stays uniform | Task 2 | a sample moved inside either light loop, which also makes `textureSample` illegal there | AC 7, 14 |
| Absent, rejected, or over-budget data resolves to fully lit, never a panic | existing filter, extended by Task 2 | a filter comparing `layer_count` alone; an unclamped layer against the placeholder | AC 3, 4, 5 |
| Static→static world shadowing stays exactly zero (pool-shadow union dead-zone) | existing promoted-union path | a group or codec decode that alters the union term | AC 8 |
| Section byte-identical across re-bakes — the shipped cached-warm-equals-uncached guarantee, not the BC6H lossy exemption | Task 2 encoder | any encoder change introducing parallelism or cluster-fit refinement | AC 13 |
| CPU payload released after upload; reload reinstalls | Task 3 | a post-upload payload reader; a reload that fails to re-read | AC 12 |

## Path

- **Encoder.** `crates/level-compiler/src/bc5.rs` emits, per 4×4 block, a BC4 R block
  then a BC4 G block from an `Rgba8Unorm` input, ignoring B and A — dependency-free
  and deterministic. A group's two mask channels map onto its R and G inputs
  directly. The per-layer pad-and-concat emit shape to mirror is
  `encode_direct_section_bc6h`, minus its padding step, which the power-of-two atlas
  dimensions make unnecessary.
- **Pack seam.** The shadowmask section is planned with no compression step today;
  the encode phase lands there, after assignment.
- **Upload and filter seam.** `crates/renderer/src/lighting/lightmap.rs` holds both
  the usability filter and the upload. The D2Array view, layer-major upload, and
  group-4 binding are reused unchanged.
- **Runtime linchpin.** `crates/renderer/src/render/shadowmask.rs` builds the
  per-spec-light slot values and packs the promoted-light metadata. The slot values
  and their sentinel are unchanged by this brief.
- **CPU release seam.** The atlas lives on the loaded level as an optional section,
  stashed at startup; the renderer borrows it at install and clones only the slot
  table.
- **Fixtures.** A focused fixture with selected specular lights suffices — the ratio
  is codec-intrinsic and the error texel-local. A capture fixture for the specular A/B
  already exists. Do not bake a stress map for proof.

## Re-anchor before building

`lighting-scale--shadowmask-cold-working-set` (landed) restructured the assignment
seam: it deletes the per-(light, texel) membership record and derives the overlap
graph analytically. This brief does not change assignment — the exact-search
4-colouring and its node budget stay exactly as they are, because at four slots the
scarce-colour problem they solve is still real. Retiring them is the capacity brief's
job, and only becomes correct once slots are abundant.
