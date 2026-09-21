# Shadowmask Atlas — Capacity and Compression — Research

Grounding for `index.md`. Facts cited by symbol; the spec holds the decisions,
this file holds the investigation. No line numbers — they stale.

Supersedes the research behind the earlier `shadowmask-atlas-blocks-and-compression`
brief, which merged two source drafts (`shadowmask-no-drop-atlas`,
`shadowmask-compress-at-rest`) into one id-42 format change. That merge stands.
Its single-channel BC4 plane layout does not — see *Why BC5 pairs, not BC4 planes*.

## The id-42 surface today

`ShadowmaskAtlasSection` (`crates/level-format/src/shadowmask_atlas.rs`):

- Header (width, height, layer_count, selected_light_count) + `channels: Vec<u8>`
  (1 byte per selected light, 0..3 = RGBA channel, `SHADOWMASK_CHANNEL_DROPPED`
  = 0xFF = globally dropped, padded to 4) + `data` = layer-major `Rgba8Unorm`
  payload. `SHADOWMASK_TEXEL_BYTES` = 4; 255 means fully visible.
- `from_bytes` validates `width × height × layer_count × 4` and gates channels at
  `> 3 && != 0xFF`.
- **Stored uncompressed.** The pack seam plans the section with no compression
  step — the sole raw baked atlas. Sections carry a **per-section version** at that
  seam (id 42 is at 1 today), independent of the file header's version.
- **Size is the lightmap atlas footprint, independent of light count.** Dims come
  from the shared lightmap atlas; a cached section disagreeing with it is rejected.
  A dropped light frees zero bytes — the buffer stays fully allocated.
- Up to four overlapping lights pack into one texel's RGBA; a fifth drops the
  dimmest.

## Why BC5 pairs, not BC4 planes

The predecessor brief proposed single-channel BC4 **planes**: one mask per array
layer, the mask for plane *p* at `lightmap_layer + p × layer_count`. That layout
carries `floor(256 / layer_count)` masks per texel.

**BC5 is two BC4 blocks in one 16-byte block.** `crates/level-compiler/src/bc5.rs`
documents and implements exactly that: per 4×4 block it emits a BC4 R block then a
BC4 G block, from an `Rgba8Unorm` input whose B and A are ignored. So storing masks
as `.rg` **pairs** per array-layer group yields:

- the identical ≈2:1 ratio (0.5 bytes per texel per mask under either),
- the same single texture binding,
- `2 × floor(256 / layer_count)` masks per texel — **double** the plane layout,
- reuse of `encode_bc5_rg` verbatim, with no single-channel BC4 wrapper to expose.

The plane layout was paying a capacity penalty for nothing. Crossover below today's
four masks moves from `layer_count ≥ 65` (planes) to `layer_count ≥ 129` (pairs).

Note the earlier figure of 52 layers was wrong: `floor(256 / L)` holds at 4 through
L = 64 and drops to 3 at L = 65. L ≥ 52 is where the plane ceiling stops being
*better* than four, not where it regresses.

## Why the fallback band matters — layer count is not author-controlled

`choose_layer_dim` (`crates/level-compiler/src/lightmap_bake.rs`) sizes the shared
per-layer dimension to host the **single largest BVH leaf**, growing by doubling
and capped at `MAX_ATLAS_DIMENSION` (8192), then spills remaining leaves into
further layers under leaf cohesion — a leaf that does not fit rolls whole to a
fresh layer. `MAX_ATLAS_LAYERS` is 256.

So layer count tracks leaf structure, not map size. A map of many small leaves gets
a small per-layer dimension and multiplies layers. 65 layers at 128² is ~1.06M
texels, roughly 1,700 m² of lit surface at the default 0.04 m/texel — an ordinary
mid-size level.

`_lightmap_density` is not a usable dial against this. Finer density scales charts
and the largest leaf together, so the dimension grows in step and the layer count
stays roughly flat, until the 8192 cap past which finer density does spill into
more layers. Coarsening density to save memory can shrink the dimension and *raise*
the layer count. The relationship is non-monotonic.

What authors do control is per-texel light pileup — placement, plus the
`entity_shadow_min_intensity_ratio` and `entity_shadow_min_range` selection floors.
That is why the spec makes the capacity floor a format guarantee rather than an
authoring warning: a warning would name a condition authors cannot act on.

## Why not BC7

`forward.wgsl` binds the atlas as `texture_2d_array<f32>` and selects `mask.r/g/b/a`
by channel index. Each channel is an **independent** per-light `[0,1]` visibility
scalar, spatially smooth, never a correlated colour tuple — BC4's home case, and
BC5's by extension.

BC7 would keep the RGBA packing, need no array-layer growth at all, and reach ≈4:1
rather than ≈2:1 — genuinely attractive on capacity grounds. Rejected on data
structure: BC7 models a per-block cross-channel correlation that four independent
masks do not have, so its ratio is bought with error concentrated on exactly this
data. No BC7-unorm encoder exists in-tree, and `bc7-color-textures` owns that codec
for `.prm` colour slots, with its own mip chain and aesthetic gate. Forcing BC7 here
to serve as a proving ground for the colour path would not transfer where it
matters.

## No new device requirement, no new alignment

- `TEXTURE_COMPRESSION_BC` is **already a hard init requirement** — the renderer
  bails at device acquisition on an adapter lacking it. BC5 adds nothing. This also
  makes the predecessor brief's acceptance row "an adapter without
  `TEXTURE_COMPRESSION_BC` falls back to the all-visible placeholder" unreachable:
  the engine never reaches level load on such an adapter. Dropped from the spec.
- `round_atlas_dim` guarantees the lightmap atlas is power-of-two and at least 64 on
  both axes, and its comment names BC6H block alignment as the reason. The
  shadowmask shares those dimensions, so a BC shadowmask needs **no padding** —
  unlike the direct-SH path, whose axes are multiples of the tile dimension (6) and
  which pads via `bc6h_padded_atlas_dimensions`.

## The binding ceiling, and the only slot available

A second shadowmask texture would give `4 × floor(256 / layer_count)` masks per
texel — never below today's four at any layer count up to the 256 cap, retiring the
raw fallback entirely. It is blocked.

`renderer_init_resources.rs` documents that the forward pass requests exactly the
sampled-texture count its BGLs compose — 16 with `CUBE_ARRAY`, 15 without — and
that 16 is the WebGPU spec floor. A 17th breaks the stated portability guarantee.
The cube-array case sets the ceiling, so the no-cube-array slack cannot be borrowed.

Per-group inventory, and why only one pair yields:

- **Group 1 — material (4):** diffuse, emissive, specular, normal. Per-material,
  bound per draw. Not mergeable.
- **Group 3 — SH volume (3):** octahedral atlas, depth-moments, direct static-light
  atlas. The depth-moments entry is a `texture_3d<u32>`. Not mergeable.
- **Group 4 — lightmap (5):** static irradiance, static dominant-direction,
  animated-contribution atlas, animated dominant-direction, shadowmask. **The two
  direction atlases are the candidate.**
- **Group 5 — shadow (4 with `CUBE_ARRAY`, else 3):** spot-shadow depth array, SDF
  shadow factor, scene depth, cube array. Distinct formats and dimensions. Not
  mergeable.

The two direction atlases share the octahedral encoding, `decode_lightmap_direction`,
and the Nearest sampler — linear interpolation of octahedral unit vectors does not
commute with slerp, so both avoid the filtering sampler. The format gap is already
bridged: `direction_texture_format` (`crates/renderer/src/lighting/lightmap.rs`)
already supports both `DIRECTION_FORMAT_OCT_RG8` (`Rg8Unorm`) and
`DIRECTION_FORMAT_OCT_RGBA8` (`Rgba8Unorm`), the animated atlas's format.

Two blockers on the merge:

1. The animated atlas is **compute-written** — created with `STORAGE_BINDING` and
   composed each frame by the animated-lightmap compute pass — while the static one
   is upload-once with `TEXTURE_BINDING` only. A merged texture needs storage usage,
   with the compute pass confined to its slice range.
2. They **index array slices differently**: static by lightmap layer, animated by
   dense animated slot through the group-4 binding-7 lookup. The merge needs a
   unified slice scheme.

This is the "lightmap array-consolidation refactor" that
`static-light-shadowmask-world-receipt` banked as the fallback for a feature needing
array-layer headroom.

## Why consolidation must sequence behind the streaming epic

`context/plans/in-progress/sh-probe-streaming/` is stage 3 of the
`large-map-spatial-residency` epic seed. Three points of contact:

1. **A pinned guard.** Its acceptance includes that the SH sampler gains no binding
   and no per-fragment locate-read, and that the forward fragment texture inventory
   and compose BGL budgets are unchanged — with a runnable guard named,
   `forward_pipeline_sampled_texture_request_matches_bgl_definitions`. A
   consolidation changes that inventory from 16 to 15, so one epic pins the number
   the other rewrites and "unchanged" loses its baseline mid-flight.
2. **Overlapping substance.** Consolidation's hard part is a unified array-slice
   index scheme; streaming's is dynamic array-slice residency, with cluster install
   and eviction rewriting which slice holds what. Different atlases today, but
   `sh-probe-streaming` explicitly names lightmap layers as the next subscriber
   through its generalization door and flags that its directory shape must not
   foreclose a second resource cheaply. `large-map-spatial-residency` stage 5 is
   "generalize the same cluster state to lightmap layers" — where the consolidation
   belongs.
3. **A new compute writer.** Consolidation makes the static direction atlas
   compute-written for the first time, adding a writer to the set of compose passes
   streaming must mark dirty on a mid-level cluster install.

Order: `sh-probe-streaming` → lightmap array-consolidation → the two-texture
shadowmask layout.

## Assignment simplifies under abundant slots

Today's assignment (`shadowmask_bake/assignment.rs`) is two-phase: a bounded exact
search for a 4-colouring under `SHADOWMASK_COLOR_SEARCH_NODE_BUDGET`, falling back
on budget exhaustion to a priority greedy that can **drop a mask on search-budget
grounds** rather than on the device ceiling. That machinery exists because four
colours is scarce.

Under groups grown on demand, colours are abundant. A deterministic greedy
first-fit — stable selection order, lowest free slot, at most Δ+1 slots — is
complete and drops only at the array-layer ceiling. Retiring the search makes the
no-drop-below-budget invariant true by construction, collapses the two
four-hardcoded loops to one, and removes a search-budget-dependent nondeterminism
source. Cost: greedy may use a few more groups than an optimal colouring right at
the ceiling. Accepted; the drop there is graceful.

`lighting-scale--shadowmask-cold-working-set` (landed) restructured this seam —
it deletes the per-(light, texel) membership record and derives the overlap graph
analytically, so adjacency arrives on a cheaper footing. Re-anchor against it
before building. One foreclosure: that deleted record is the only structure where
per-texel visibility values and cross-light adjacency coexist. Intensity-ordered
retention reads light parameters only and is unaffected, but a contribution- or
coverage-weighted retention priority would have to re-materialize that term.

## Shader consumption — the sample cannot stay hoisted

Both decode paths currently sample the atlas **once per fragment**, hoisted out of
their light loops, because every light shared one texel's four channels; each light
then selects its channel by index. The world-specular path hoists with the comment
"undo this if specular gains per-light UVs"; the promoted-union path hoists because
every promoted light shares the fragment's lightmap UV and layer.

Group addressing puts each light's mask on its own array layer, so the sample moves
inside both loops. Both contain `continue` and `break`, which is non-uniform control
flow — where WGSL forbids the implicit-derivative sampling function. The explicit-level
form is required. The atlas carries a single mip level, so this changes no sampled
value. The real cost is one sample per fragment becoming one per contributing static
light, behind the early-outs already gating those loops.

The slot crosses into the runtime via the `SpecLight` shadowmask field and the
promoted record's metadata as a float, with the sentinel preserved. Today the shader's
dropped-sentinel comparison value is 4.0, one past the last RGBA channel; it must move
above every representable slot. On the wire, the current 8-bit table element is safe
only because channels occupy 0..3 — a group-addressed slot reaches the full 8-bit
range including the 0xFF sentinel, so the element must widen and the sentinel move
outside the representable slot range.

## Runtime lifecycle — the double residency

- **RAM.** `from_bytes` copies the payload into an owned `Vec<u8>`; the loader stores
  it on `LevelWorld` as an `Option<ShadowmaskAtlasSection>`, accepted only if dims
  match the lightmap irradiance atlas; startup stashes the world, so the payload stays
  resident for the level lifetime.
- **VRAM.** The upload does one `create_texture_with_data` — `Rgba8Unorm`,
  `depth_or_array_layers = layer_count`, one mip level, layer-major,
  `TEXTURE_BINDING | COPY_DST`; D2Array view, bound at group 4. No mips, no streaming.
- **The renderer keeps only the slot table.** Init clones it; the payload is never
  copied there, but the `LevelWorld` source is not freed. Freeing it after upload
  removes the RAM half at zero quality cost. A level reload re-reads a fresh world, so
  reuse is not a consumer.

## Prior commitments preserved

- `rendering_pipeline.md` §4: absent, rejected, or dropped shadowmask data is fully
  lit, and this world-only signal stays independent of pool-shadow promotion and its
  crossfade. Preserved, extended to an over-budget layer-group product.
- Static→static world shadowing stays exactly zero via the pool-shadow
  union-subtraction dead-zone. Group addressing and the codec change mask *location*
  and *encoding*, not the union term.
- `build_pipeline.md` §Build Cache keys the shadowmask memo on inputs, not outputs,
  and exempts lossy compressed output from byte-identity;
  `sh-base-atlas-at-rest-slimming` set that posture for ids 34/35. **Not inherited
  here.** `static-light-shadowmask-cache-addendum` ships a live AC requiring cached
  warm output for `ShadowmaskAtlasSection` to match uncached output byte-for-byte,
  and `bc5.rs` is a pure function — per-block min/max endpoints, a fixed integer
  palette matching the hardware ladder, nearest-index selection, no cluster-fit
  refinement, no parallelism. Byte-identity is therefore achievable, and taking the
  exemption by analogy to BC6H would surrender a currently-satisfied guarantee for
  nothing. The whole section stays byte-identical across re-bakes.
- The `build_pipeline.md` id-42 line is revised at promotion.

## Streaming — why it stays a separate epic

Nothing streams today: lightmap, direct-SH, and shadowmask all upload whole and stay
resident. `sh-probe-streaming` builds the cluster-of-cells residency substrate
(resource-agnostic key, LRU eviction, prefetch and hysteresis), names lightmap-layer
and shadowmask as future subscribers, and wires only SH; its cluster-directory section
id is reserved and unemitted.

The shadowmask is co-keyed with the lightmap — same UV and layer, with the layer baked
per-vertex — so its streamability couples to the lightmap's, and the natural streamed
unit is cell-keyed baked atlas data for both together. Compression composes with
streaming rather than foreclosing it: bytes per resident texel is orthogonal to which
texels are resident, and re-baking into cluster-addressable compressed form is cheap
pre-stable.

## Prior-art and collision map

- **No plan owns id-42 size, RAM, or VRAM reduction** — the open gap this epic closes.
- `lighting-scale--shadowmask-cold-working-set` (done) — restructured the assignment
  seam; compile-time RAM only, byte-identical output, with the explicit non-goal
  "bounding the output below its on-disk size — a format question." Confirms the gap is
  open, not owned.
- `lighting-scale--sh-base-atlas-at-rest-slimming` (done) — BC-at-rest precedent for
  ids 34/35. Its section-length-stable posture is available but deliberately not
  taken; see *Prior commitments preserved*. Note also its opening argument that an
  unconditional at-rest win should not be bundled with a gated, contract-rewriting
  sibling — a thesis the current one-format-evolution framing chooses against, and
  the crux of the outstanding split question.
- `shadowmask-array-atlas` (done) — closed "no action"; id 42 is already a
  `texture_2d_array` within device limits.
- `shadowmask-bake-scaling` (done) — restructured the composite into streaming
  membership → assignment → fill. This epic changes the assignment step and adds an
  encode phase at the pack seam; the composite it builds on is in place.
- `bc7-color-textures` (draft, stub) — owns BC7. Not a dependency in either direction.
- `sh-probe-streaming` (in-progress) and `large-map-spatial-residency` (epic seed) —
  see the sequencing section above.

## Proof shape

Proof covers both axes across the full lifetime: production (slot assignment, codec
selection, BC5 encode), serialization (round-trip, codec length check, slot-range
rejection), persistence (on-disk byte delta), the GPU return (resident byte delta,
upload format, greater-than-four overlap renders every mask), and cleanup (CPU payload
freed, reload reinstalls). The graceful-degradation and determinism rows close the
invariants. BC5 needs no codec-selection gate — a measured error report plus the visual
A/B confirm it.

Focused fixtures only. The ratio is codec-intrinsic and the error texel-local, and a
small fixture can force greater-than-four overlap, so no stress map is baked for proof.
Tests run without a GPU context; the upload and visual A/B are the thin GPU layer,
verified through the offscreen capture path.

## Orderings and edges

| id | scenario | ordering / mechanism | expected outcome |
|---|---|---|---|
| collision-free-slots | two or more selected lights share a lightmap texel | assignment writes each light's slot into the per-selection table before payload fill | no overlap edge has both endpoints on the same non-sentinel slot |
| budget-boundary | the layer-group product hits the array-layer ceiling exactly | the renderer's usability filter evaluates `layer_count × group_count` against the device maximum before texture creation | equal is kept, one greater degrades to the placeholder; no off-by-one device breach |
| codec-selection-boundary | layer count at the capacity floor's edge | the bake picks the codec from the layer budget before assignment fixes the slot space | at 128 layers BC5 pairs seat four masks and are chosen; at 129 the raw codec is chosen and seats four |
| format-edges | single-group, empty selection, each codec tag | header encodes codec tag and group count; `from_bytes` recomputes payload length from the tagged block size | every combination round-trips byte-exact |
| dropped-and-placeholder-lit | sentinel slot, or a non-zero group index against the one-layer placeholder | the shader clamps the computed array layer, then applies the per-path sentinel guard | both paths return fully lit; neither samples outside the bound texture |
| raw-fallback-parity | a bake in the fallback band | codec selection emits raw; assignment reverts to four channel slots in one group | capacity and bytes per mask match today's exactly |
