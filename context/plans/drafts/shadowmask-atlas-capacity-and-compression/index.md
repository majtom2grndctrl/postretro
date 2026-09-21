# Shadowmask Atlas — Capacity and Compression (Epic)

> **Status:** Draft epic. Slice 1 is specified and buildable today. Slice 2 is
> blocked behind `large-map-spatial-residency` stage 5 and is scoped, not
> specified. Not promoted; not ready for `/build-spec`.
> **Supporting research:** `research.md` (sibling).

## Goal

PRL id 42 (`ShadowmaskAtlasSection`) is the only baked atlas still stored raw,
and it caps a texel at four overlapping static-light masks. Close both gaps
under one format evolution, with a hard floor: no layout this epic ships may
carry fewer masks per texel, or more bytes per mask, than today's raw RGBA
packing at any lightmap layer count.

## Scope

### In scope

- A codec-tagged, group-indexed id-42 wire format replacing the fixed RGBA-channel
  packing.
- BC5 `.rg` mask pairs at rest (≈2:1 on disk, CPU RAM, and VRAM together).
- Raw `Rgba8Unorm` retained as a selectable codec, and used as the capacity floor
  when the array-layer budget cannot seat four masks.
- Deterministic greedy slot assignment, replacing the exact-search 4-colouring and
  its node budget.
- Releasing the CPU-side payload after upload.
- Scoping — not building — the two-texture layout that removes the residual
  capacity ceiling, and the binding-slot work it depends on.

### Out of scope

- **Streaming or visibility-driven residency of the atlas.** Owned by
  `large-map-spatial-residency`. Compression composes with it: bytes per resident
  texel is an orthogonal lever to which texels are resident.
- **The lightmap array-consolidation refactor itself.** Named here as Slice 2's
  prerequisite, specified by neither this epic nor a current draft.
- **A BC7 encoder, or BC7 on colour textures.** `bc7-color-textures` owns BC7.
- **The lightmap, irradiance, and direction atlases.** `lightmap_layer` keeps its
  meaning as the receiver's spatial array layer. The "shadowmask dims == lightmap
  dims" invariant is unchanged — BC preserves dimensions.
- **Selection eligibility and ranking.** Which lights are selected is unchanged.
- **The shadowmask bake's memory, parallelism, and progress.** Owned by
  `shadowmask-bake-scaling` (landed). This epic changes what the bake emits.
- **Old-`.prl` migration.** Fixtures re-bake; the section version advances and
  stale caches regenerate.

## Direction

**Problem.** A texel packs per-light masks into the four RGBA channels of one raw
`Rgba8Unorm` texel. A fifth overlapping selected light is dropped lowest-intensity
first and its runtime shadow disappears; and the payload is stored, loaded, and
held uncompressed while ids 22 and 35 both ship BC-compressed. One wire surface
carries both defects, so one format evolution closes both.

**Prior commitments.** `rendering_pipeline.md` §4 commits that absent, rejected,
or dropped shadowmask data reads fully lit, independent of pool-shadow promotion
and its crossfade — preserved and extended here to a new cause (an over-budget
layer product). The static→static double-count dead-zone is untouched: this epic
changes where a mask lives and how it is encoded, never the union subtraction.
`build_pipeline.md` §Build Cache keys the shadowmask memo on inputs and exempts
lossy compressed output from byte-identity; `sh-base-atlas-at-rest-slimming` set
that posture, and Slice 1 inherits it — the pre-compression assignment must be
byte-identical, the compressed section only length-stable.
`static-light-shadowmask-world-receipt` banked a lightmap array-consolidation
refactor as the fallback for a feature needing array-layer headroom. Slice 2 is
that feature; this epic names the coupling rather than triggering it.

**Alternatives rejected.**

*Single-channel BC4 planes* (the shape this epic's predecessor brief proposed):
one mask per array layer, `floor(256 / layer_count)` masks per texel. Rejected
because it regresses capacity against today at 65+ lightmap layers while
delivering the same ≈2:1 ratio that BC5 pairs deliver with twice the capacity, at
the same one binding, reusing `encode_bc5_rg` rather than needing a single-channel
BC4 wrapper. BC5 *is* two BC4 blocks in one 16-byte block — the plane layout was
paying a capacity penalty for nothing.

*BC7* would keep the RGBA packing, need no array-layer growth at all, and reach
≈4:1. Rejected on data structure: BC7 models a per-block cross-channel
correlation, and four independent visibility scalars have none, so its ratio is
bought with error concentrated on exactly this data. No BC7-unorm encoder exists
in-tree, and `bc7-color-textures` owns that codec.

*Two BC5 textures now* removes the capacity ceiling outright but needs a
sampled-texture slot the forward pass does not have. See Slice 2.

## Capacity model

The array-layer budget is the binding constraint. `REQUIRED_MAX_TEXTURE_ARRAY_LAYERS`
is 256, requested in `required_limits` and pre-checked against the adapter, so the
granted limit is 256. `MAX_ATLAS_LAYERS` caps the lightmap packer at 256 layers.

Masks per texel, as a function of lightmap `layer_count` (L), where the group
count P is bounded by `floor(256 / L)`:

| Layout | Masks/texel | Bytes/texel/mask | Falls below today at |
|---|---|---|---|
| Today — raw `Rgba8Unorm` | 4 | 1.0 | n/a |
| Rejected — BC4 planes | `floor(256/L)` | 0.5 | L ≥ 65 |
| **Slice 1 — BC5 `.rg` pairs** | `2 × floor(256/L)` | 0.5 | L ≥ 129 |
| Slice 1 raw fallback | 4 | 1.0 | never |
| Slice 2 — two BC5 textures | `4 × floor(256/L)` | 0.5 | never |

Slice 1's guarantee comes from the last two rows together: BC5 pairs while
`2 × floor(256/L) ≥ 4` (that is, L ≤ 128), raw `Rgba8Unorm` above it. Compression
is lost in the fallback band, capacity never is.

Why the fallback band is not merely theoretical: `layer_count` tracks BVH leaf
structure, not map size or authored density. `choose_layer_dim` sizes the shared
per-layer dimension to host the single largest leaf, then spills the rest into
further layers under leaf cohesion, so a map of many small leaves gets a small
dimension and multiplies layers. `_lightmap_density` is not a usable dial against
this: finer density scales charts and the largest leaf together, so the dimension
grows in step and the layer count stays roughly flat until the 8192 cap, past
which finer density does spill. Coarsening density can shrink the dimension and
*raise* the layer count. Authors have no reliable control over L; what they control
is per-texel light pileup, through placement and the
`entity_shadow_min_intensity_ratio` / `entity_shadow_min_range` selection floors.
This is why the floor is a format guarantee and not an authoring warning.

## Slices

**Slice 1 — one-binding BC5 pairs with a raw capacity floor.** Buildable today.
Changes no binding count, so it cannot collide with the in-flight
`sh-probe-streaming` inventory guard. Delivers the full ≈2:1 on every map with
L ≤ 128, and 2× to 128× today's per-texel mask capacity over the same band. The
Tasks, Wire format, Boundary inventory, and Invariants sections below specify it.

**Slice 2 — two BC5 textures.** Raises capacity to `4 × floor(256/L)`, which never
falls below today at any layer count, retiring the raw fallback band. Blocked: the
forward pass already requests exactly 16 sampled textures per stage, which
`renderer_init_resources.rs` documents as the WebGPU spec floor, so a 17th breaks
the stated portability guarantee. The count is 15 without `CUBE_ARRAY`, so the
cube-array case sets the ceiling and its slack cannot be borrowed.

The only viable slot comes from merging the two direction atlases — forward.wgsl
group 4 bindings 1 (static) and 5 (animated). They share the octahedral encoding,
`decode_lightmap_direction`, and the Nearest sampler at binding 2, and
`direction_texture_format` already bridges the format gap by supporting both
`DIRECTION_FORMAT_OCT_RG8` and `DIRECTION_FORMAT_OCT_RGBA8`. Two blockers: the
animated atlas is compute-written while the static one is upload-once, so the
merged texture needs storage usage with the compute pass confined to its slice
range; and the two index array slices differently — static by lightmap layer,
animated by dense animated slot via the group-4 binding-7 lookup — so the merge
needs a unified slice scheme.

That merge is the lightmap array-consolidation refactor, and it must sequence
behind `sh-probe-streaming`, not beside it. That epic's acceptance pins the forward
fragment texture inventory as unchanged and names a runnable guard for it
(`forward_pipeline_sampled_texture_request_matches_bgl_definitions`); a
consolidation changes that inventory from 16 to 15, so one epic would be pinning
the number the other rewrites. The two also overlap in substance — consolidation's
hard part is a unified array-slice index scheme, streaming's is dynamic array-slice
residency — and `large-map-spatial-residency` stage 5 is exactly "generalize the
same cluster state to lightmap layers," where the consolidation belongs.
Consolidation would additionally make the static direction atlas compute-written
for the first time, adding a writer to the compose passes streaming must mark
dirty on a mid-level cluster install.

Order: `sh-probe-streaming` → lightmap array-consolidation → Slice 2. Slice 1
enters none of that chain, and the codec tag makes Slice 2 additive — a new tag
value and a second upload, leaving the wire format, slot indexing, and assignment
logic intact.

## Acceptance criteria

Slice 1 only. Slice 2 inherits these and adds its own when specified.

- [ ] On a fixture with a lightmap texel overlapped by 5–8 selected static lights,
      every selected light receives a non-sentinel slot and its shadow is present
      at runtime.
- [ ] A mask is dropped only when the next group would push `layer_count × group_count`
      past the device array-layer maximum. At or below the maximum, nothing is
      dropped. The drop, when it occurs, is the lowest-intensity mask, matching the
      pre-change policy.
- [ ] Slot assignment is collision-free: for every overlap edge in the selection
      graph, the two lights hold different slots, or one holds the sentinel. Asserted
      over the assignment output, independent of any rendered texel.
- [ ] No drop arises from search-budget exhaustion — that path and its node budget
      no longer exist.
- [ ] Codec selection follows the capacity floor: a bake whose layer count admits
      four or more masks under BC5 pairs emits the BC5 codec; one that does not emits
      the raw codec and its four-mask capacity.
- [ ] Round-trip holds at every format edge: `to_bytes` → `from_bytes` reproduces
      header, slot table, and payload for a multi-group BC5 section, a single-group
      section, a raw section, and an empty selection. `from_bytes` rejects an
      out-of-range non-sentinel slot, and a payload whose length disagrees with the
      tagged codec's block arithmetic.
- [ ] A section whose `layer_count × group_count` exceeds the device maximum is
      rejected to the all-visible placeholder with a `[Renderer]` error, no panic.
      The check compares the product, not `layer_count` alone.
- [ ] Boundary: a product exactly equal to the device maximum is retained; one
      greater by a single layer degrades to the placeholder.
- [ ] A sentinel slot reads fully lit in both decode paths; and a baked non-zero
      group index samples fully lit rather than out of range when the bound texture
      is the one-layer all-visible placeholder.
- [ ] Static→static world shadowing stays exactly zero: adding a light that lands in
      a non-zero group does not change world-specular output for surfaces already
      covered by group-zero lights.
- [ ] Both decode paths sample the array layer derived from the light's group for a
      light in any group, under every codec tag.
- [ ] Source-inspection gate: neither decode path in `forward.wgsl` samples the
      shadowmask through a call requiring uniform control flow, and both derive the
      array layer from the light's group rather than from `lightmap_layer` alone.
- [ ] Control, not a fix gate: four-way overlap carries every mask with no drop
      under both codecs.
- [ ] On a fixture carrying a populated atlas, the id-42 on-disk section byte count
      drops ≈2:1 against the raw baseline of the same mask count, measured by the
      per-section byte accounting at the pack seam.
- [ ] The renderer uploads the BC5 section in its compressed format and the computed
      resident byte count drops ≈2:1 against the raw baseline. An all-visible (255)
      atlas round-trips to fully lit through encode and decode.
- [ ] After upload, no `width × height × layer_count × group_count` shadowmask buffer
      remains resident on the CPU, and a subsequent level reload still installs a
      correct atlas.
- [ ] Re-baking a fixture twice yields a byte-identical pre-compression assignment
      and a length-stable compressed section, so the build cache stays valid.
      Byte-identity holds across differing worker-thread counts, and slot-open order
      is a pure function of a stable ordering key rather than of chart-worker or
      iteration order.
- [ ] Fidelity, measured and reported rather than gated: max and mean per-channel
      absolute error of the BC5 encode against the raw masks, recorded in the landing
      note. The visual A/B below is the fidelity gate.
- [ ] Manual: in a scene with selected non-SDF static world specular lights,
      specular highlights and their shadowmask-occluded regions read unchanged
      against a raw-atlas capture taken through the offscreen capture path. A
      heavily-masked region retains its specular contrast.
- [ ] Manual: the bake reports layer count, group count, resulting mask capacity,
      peak observed per-texel overlap, and the selected codec under `--verbose`, and
      warns without `--verbose` only when a mask is actually dropped, naming the
      lights that lost masks. A bake with headroom gains no new non-verbose line.

## Tasks

### Task 1: Codec-tagged format and group addressing — thin slice

Restructure `ShadowmaskAtlasSection` per the Wire format section, and carry one
BC5 section end to end before any of the capacity or fallback work exists. The
header gains a codec tag and a group count; the per-selection table becomes a
widened slot index with its own sentinel; the payload becomes the tagged codec's
block stream over `layer_count × group_count` array layers; `from_bytes`
recomputes expected payload length from the tagged codec's block arithmetic and
validates slots against the full representable range. Advance the id-42 section
version — sections carry a per-section version at the pack seam, independent of
the file header's version.

Emit BC5 only in this task, with the group count fixed at two, and assign
overlapping lights to slots by the simplest rule that fills both groups — the
real policy is Task 2. Encode each group's two mask channels through the in-tree
BC5 encoder. Branch the upload texture format on the codec tag and size the
texture's array dimension to the layer-group product. Thread the slot index
through the runtime linchpin into the `SpecLight` shadowmask field and the
promoted record's metadata, preserving the sentinel. In `forward.wgsl`, derive
the array layer from the light's group and select the channel from its parity, in
both the world-specular and promoted-union paths.

Two constraints bind this task, both of which the natural implementation
violates. The shadowmask sample is currently hoisted out of both light loops
because every light shared one texel's four channels; per-light array layers
make that impossible, and both loops contain `continue` and `break`, so the
sample lands in non-uniform control flow where WGSL forbids the implicit-derivative
sampling function. Use the explicit-level form. The atlas carries a single mip
level, so this changes no sampled value. Separately, the slot index and its
sentinel must not collide: the current 8-bit table is safe only because channels
occupy 0..3, and a group-addressed slot can reach the full 8-bit range including
the current sentinel value.

Proven when a fixture texel overlapped by four selected lights spread across two
groups shows every shadow at runtime — which falsifies the wire, runtime, shader,
and codec boundaries together.

### Task 2: Greedy slot assignment, codec selection, and degradation

Replace the exact-search colouring, its deterministic node budget, and the
priority-greedy fallback with one deterministic greedy first-fit: each selected
light, in stable selection order, takes the lowest slot no overlap neighbour
holds, opening groups on demand. Under groups grown on demand the scarce-four-colour
problem that machinery solved no longer exists; first-fit uses at most Δ+1 slots
and drops only at the device ceiling, which makes the no-drop-below-budget
invariant true by construction and removes a search-budget-dependent
nondeterminism source. Do not reintroduce search for compactness near the ceiling
— a greedy assignment using a few more groups than optimal is accepted, and the
drop there is graceful.

Select the codec from the capacity floor in the Invariants table: BC5 pairs when
the layer count admits at least four masks, raw `Rgba8Unorm` otherwise, in which
case the assignment reverts to four channel slots in one group and the section is
uncompressed. Thread the same array-layer bound the renderer enforces into the
bake so the cap binds at bake time, and extend the renderer's usability filter so
an over-budget layer-group product degrades to the all-visible placeholder with a
`[Renderer]` error rather than reaching texture creation.

Assignment must be a pure function of the selection and per-light layer inputs so
the pre-compression section is byte-stable across worker-thread counts. Track peak
observed per-texel overlap and report it with the layer count, group count,
capacity, and selected codec under `--verbose`; warn without `--verbose` only when
a mask is actually dropped, naming the lights that lost masks.

### Task 3: Release the CPU payload after upload

Drop the shadowmask payload from the loaded level once the upload has run,
retaining the small slot table the renderer clones. Establish that no consumer
reads the payload after upload before freeing it; a level reload re-reads from a
fresh world, so reuse is not a consumer. This removes the RAM half of the double
residency at no quality cost.

### Task 4: Byte accounting and fidelity measurement

Report the id-42 on-disk section byte count through the existing per-section
accounting at the pack seam, and the computed resident byte count at upload, each
against the raw baseline for the same mask count. Measure and record max and mean
per-channel absolute error of the BC5 encode against the raw masks on the fixture.

The error figure is reported, not gated. This task also owns the fidelity gate
itself: capture the specular A/B through the offscreen capture path against a
raw-atlas baseline, on a scene with selected non-SDF static world specular lights,
and look at both images rather than only their statistics — a distribution check
passes on an image that has lost its contrast.

### Task 5: Coverage and durable capture

Lock the contract with the round-trip, rejection, boundary, degradation,
collision-free, determinism, double-count, and reload coverage the acceptance
criteria name, plus a shader test that a light in a non-zero group samples the
right layer in both decode paths under both codecs. Revise the `build_pipeline.md`
id-42 line and the `rendering_pipeline.md` §4 world-specular statement to describe
codec-tagged group addressing, the capacity floor and its raw fallback, and the
device-budget drop. Route any new concept through `index.md`.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the wire ↔ runtime ↔
shader ↔ codec boundaries before any capacity work is built on them.
**Phase 2 (sequential):** Task 2 — consumes Task 1's codec tag and group
addressing; its codec selection changes what Task 1 hardcoded.
**Phase 3 (concurrent):** Task 3, Task 4 — independent; different files, no shared
contract.
**Phase 4 (sequential):** Task 5 — consumes every prior task's surface.

## Wire format

`ShadowmaskAtlasSection` keeps little-endian encoding, a 16-byte-aligned header,
the per-selection table padded to 4, then the payload, and section id 42, under an
advanced per-section version.

- The header carries the existing width, height, layer count, and selected-light
  count, plus a **codec tag** and a **group count**. Two codec tags are defined:
  raw `Rgba8Unorm` and BC5 `.rg`. The raw tag's group count is one.
- The per-selection table stores one **slot index** per selection index, or a
  dropped sentinel. The element must represent every slot the group count admits
  *and* a sentinel that no valid slot can equal — the current 8-bit element cannot,
  since a group-addressed slot reaches the full 8-bit range. Widen it and pick the
  sentinel from outside the representable slot range. An empty selection encodes as
  today.
- Slot semantics per codec. Under the raw tag, slot `s` in `0..3` is the RGBA
  channel of the texel at array layer `lightmap_layer`. Under the BC5 tag, slot `s`
  addresses group `s / 2` and channel `s % 2`, at array layer
  `lightmap_layer + (s / 2) × layer_count`.
- `data` is the tagged codec's block or texel stream, layer-major over
  `layer_count × group_count` array layers. `from_bytes` computes expected length
  from the tagged codec's per-texel or per-block size — raw is 4 bytes per texel,
  BC5 is 16 bytes per 4×4 block — times `width × height × layer_count × group_count`,
  and rejects a disagreement.

Field widths and exact header slots are implementation choices. The binding
constraints are that codec tag, group count, and every representable slot
round-trip; that the sentinel cannot collide with a valid slot; and that the
payload cross-check matches the tagged codec.

No block-alignment padding is introduced. `round_atlas_dim` guarantees the
lightmap atlas is power-of-two and at least 64 on both axes and is commented as
BC-block-aligned, and the shadowmask shares those dimensions. This is unlike the
direct-SH path, which pads because its axes are multiples of the tile dimension.

## Boundary inventory

Two encodings cross module boundaries and are each pinned once.

| Crossing | Rust (compiler) | Wire / serde | Runtime | Shader |
|---|---|---|---|---|
| Slot index | per-selection table entry = slot; sentinel = dropped | table element wide enough for every slot plus a non-colliding sentinel; header group count; payload layer-major over `layer_count × group_count` | `SpecLight` shadowmask field and the promoted record's metadata carry the slot as a float; the sentinel-or-above comparison value must exceed every representable slot | array layer from the slot's group, channel from its parity |
| Payload encoding | encode each group's two mask channels through the in-tree BC5 encoder; raw path emits today's bytes | codec tag in header; `data` = the tagged stream; cross-check by that codec's block size | upload texture format selected from the codec tag; D2Array view, layer-major upload, and group-4 binding reused | one `texture_2d_array<f32>` binding under both tags |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Capacity floor: masks per texel never below four, bytes per mask never above today's, at any layer count | Task 1 raw codec, Task 2 codec selection | a codec chosen without consulting the layer budget; a raw path that stops round-tripping | AC 5, 6, 13, 14 |
| No selected mask dropped while `layer_count × group_count` fits the device array-layer maximum — true by construction under greedy first-fit, with no search-budget drop path | Task 2 | any residual four-slot cap surviving in assignment, metadata, or shader | AC 1, 2, 3, 4 |
| Absent, rejected, or over-budget shadowmask data resolves to fully lit, never a panic | existing usability filter, extended by Task 2 | a filter comparing `layer_count` alone; an unclamped array layer against the placeholder | AC 7, 8, 9 |
| Static→static world shadowing stays exactly zero (pool-shadow union dead-zone) | existing promoted-union path | a group or codec decode that alters the union term | AC 10 |
| Pre-compression assignment byte-deterministic; compressed section length-stable | Task 2 assignment, Task 1 encoder | non-deterministic slot-open order; an encoder whose output length varies across runs | AC 17 |
| CPU payload released after upload; reload reinstalls | Task 3 | a post-upload payload reader; a reload that fails to re-read | AC 16 |

## Rough sketch

- **Encoder.** `crates/level-compiler/src/bc5.rs` already emits, per 4×4 block, a
  BC4 R block followed by a BC4 G block from an `Rgba8Unorm` input, dependency-free
  and deterministic via min/max endpoints. A group's two mask channels map onto its
  R and G inputs directly; B and A are ignored by that path. The pad-and-concat
  per-layer emit shape to mirror is `encode_direct_section_bc6h`, minus its padding
  step, which the power-of-two atlas dimensions make unnecessary.
- **Assignment seam.** `crates/level-compiler/src/shadowmask_bake/assignment.rs`
  holds the overlap graph and the exact-search plus priority-greedy machinery Task 2
  retires; `shadowmask_bake/fill.rs` holds the fill that consumes the assignment and
  computes per-texel byte offsets. Re-anchor on the landed cold-working-set
  restructure, which derives the overlap graph analytically rather than from a
  per-(light, texel) membership record.
- **Upload and filter seam.** `crates/renderer/src/lighting/lightmap.rs` holds both
  the usability filter and the upload; the texture format branches on the codec tag
  and the array dimension becomes the layer-group product. The D2Array view,
  layer-major upload, and group-4 binding are otherwise reused.
- **Runtime linchpin.** `crates/renderer/src/render/shadowmask.rs` builds the
  per-spec-light slot values and packs the promoted-light metadata.
- **CPU release seam.** The atlas lives on the loaded level as an optional section,
  stashed at startup; the renderer borrows it at install and clones only the slot
  table.
- **Proof fixtures.** A focused fixture with selected specular lights over a
  greater-than-four overlap texel suffices for both axes — the ratio is
  codec-intrinsic and the error is texel-local. A capture fixture for the specular
  A/B already exists. Do not bake a stress map for proof.

## Open questions

- **Whether Slice 1 ships the raw fallback, or degrades to the placeholder above
  128 layers.** Shipping it is what makes the capacity floor a guarantee, and it is
  the reason this shape was chosen over BC4 planes; the cost is that `forward.wgsl`
  retains the RGBA channel-select path alongside the new group addressing, branched
  on a per-level uniform, rather than deleting it. Specified as shipping. Revisit
  only with a measurement showing the fallback band unreachable on real content —
  which would require establishing what layer counts shipping maps actually produce,
  a measurement this epic does not have and stage-5 planning will need anyway.
- **Whether Slice 2 survives its own prerequisite.** If the lightmap
  array-consolidation lands as part of `large-map-spatial-residency` stage 5, the
  slice scheme it introduces may make a second shadowmask texture unnecessary or
  differently shaped. Slice 2 is scoped here, not specified, for that reason.
