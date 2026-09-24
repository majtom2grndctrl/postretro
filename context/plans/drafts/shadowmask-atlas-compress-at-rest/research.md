# Shadowmask Atlas — BC5 Compress-at-Rest — Research

Grounding for `index.md`. Facts cited by symbol; the brief holds the decisions, this
file holds the investigation. No line numbers — they stale.

Capacity-side grounding — the layer budget, the binding ceiling, the assignment
rewrite, the consolidation and streaming sequencing — lives in
`shadowmask-atlas-mask-capacity/research.md`, since this brief makes no capacity
claim.

## The id-42 surface today

`ShadowmaskAtlasSection` (`crates/level-format/src/shadowmask_atlas.rs`):

- Header (width, height, layer_count, selected_light_count) + `channels: Vec<u8>`
  (one byte per selected light, `0..3` = RGBA channel, `SHADOWMASK_CHANNEL_DROPPED`
  = `0xFF` = globally dropped, padded to 4) + `data` = layer-major `Rgba8Unorm`
  payload. `SHADOWMASK_TEXEL_BYTES` = 4; 255 means fully visible.
- `from_bytes` validates `width × height × layer_count × 4` and gates channels at
  `> 3 && != 0xFF`.
- **Stored uncompressed.** The pack seam plans the section with no compression step —
  the sole raw baked atlas. Sections carry a **per-section version** at that seam
  (id 42 is at 1), independent of the file header's version.
- **Size is the lightmap atlas footprint, independent of light count.** Dims come from
  the shared lightmap atlas; a cached section disagreeing with it is rejected. A
  dropped light frees zero bytes — the buffer stays fully allocated.

**No byte magnitude is recorded anywhere in the repo.**
`lighting-scale--sh-base-atlas-at-rest-slimming` recorded ids 34, 35, 27, 41 and 45
on `campaign-test` and omitted 42. That plan opened with a measured-basis table
*before* choosing a codec; this brief restores that discipline by measuring first
(Task 1).

## Why BC5, and why the group count is fixed at two

The masks are **independent** per-light `[0,1]` visibility scalars, spatially smooth,
never a correlated colour tuple — BC4's home case.

`crates/level-compiler/src/bc5.rs` documents and implements BC5 as exactly two BC4
blocks in one 16-byte block: per 4×4 block it emits a BC4 R block then a BC4 G block,
from an `Rgba8Unorm` input whose B and A are ignored. It is dependency-free and
order-deterministic — trivial per-block min/max endpoints, a fixed integer palette
built with the D3D/wgpu hardware formulas so selector choice matches what the GPU
reconstructs, no cluster-fit refinement, no parallelism.

So two array-layer groups of BC5 carry `(g0.r, g0.g, g1.r, g1.g)` — **exactly four
masks**, today's number, at 0.5 bytes per texel per mask against today's 1.0.

Fixing the group count at two is what keeps this brief small, and the reasons compound:

- **The slot table needs no change.** Four slots fit `0..3` with `0xFF` as sentinel,
  as today. A grown group count would push slots to the full 8-bit range and collide
  with that sentinel — a widening this brief does not need.
- **The sample stays hoisted.** Both array layers (`lightmap_layer` and
  `lightmap_layer + layer_count`) are known from `lightmap_layer` alone, so both
  samples hoist out of the light loops exactly as the single sample does today.
  Control flow stays uniform, so `textureSample` stays legal and no explicit-level
  form is needed. Cost is **two hoisted samples per fragment instead of one** —
  fixed, independent of light count.
- **No capacity claim, so no capacity floor** — no raw fallback codec, no second
  permanent shader decode path, and no dependence on the unmeasured overlap
  frequency.

Once the group count grows, all three reverse at once. That cluster of consequences
belongs to `shadowmask-atlas-mask-capacity`, and keeping it out is what makes this
brief cheap.

## Why not BC7

BC7 would keep the RGBA packing, need no array-layer growth at all, and reach ≈4:1
rather than ≈2:1 — genuinely attractive. Rejected on data structure: BC7 models a
per-block cross-channel correlation that four independent masks do not have, so its
ratio is bought with error concentrated on exactly this data. No BC7-unorm encoder
exists in-tree, and `bc7-color-textures` owns that codec for `.prm` colour slots,
with its own mip chain and aesthetic gate. Forcing BC7 here as a proving ground for
the colour path would not transfer where it matters.

## No new device requirement, no new alignment

- `TEXTURE_COMPRESSION_BC` is **already a hard init requirement** — the renderer bails
  at device acquisition on an adapter lacking it. BC5 adds nothing. It also means a
  no-BC-adapter degradation row would be unreachable — the engine never reaches level
  load on such an adapter — so none is written.
- `round_atlas_dim` guarantees the lightmap atlas is power-of-two and at least 64 on
  both axes, and its comment names BC6H block alignment as the reason. The shadowmask
  shares those dimensions, so a BC shadowmask needs **no padding** — unlike the
  direct-SH path, whose axes are multiples of the tile dimension (6) and which pads
  via `bc6h_padded_atlas_dimensions`.
- The in-header format tag mirrors `LightmapSection::direction_format`, which already
  tags `DIRECTION_FORMAT_OCT_RG8` versus `DIRECTION_FORMAT_OCT_RGBA8` and branches the
  upload format on it. Same shape, same seam.

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
  warm output for `ShadowmaskAtlasSection` to match uncached output byte-for-byte, and
  `bc5.rs` is a pure function per above. Byte-identity is therefore achievable, and
  taking the exemption by analogy to BC6H would surrender a currently-satisfied
  guarantee for nothing. The whole section stays byte-identical across re-bakes.
- `shadowmask-bake-scaling`'s byte-identity AC is knowingly broken — the section's
  bytes change because its encoding does. Fixtures re-bake.
- The `build_pipeline.md` id-42 line is revised at promotion.

## Why this is its own brief

`lighting-scale--sh-base-atlas-at-rest-slimming` opens with the argument, and it
applies here almost verbatim: an unconditional, certain at-rest win must not be
bundled with a gated, contract-rewriting sibling, because `/build-spec` consumes one
spec per run and binding them holds the certain work hostage to the sibling's gate —
a mid-run no-go leaves one branch half-implemented and one review panel judging a
mixture. It also pre-answers the bundling counter-argument: what the coupling would
buy is *not* avoiding a second version bump, since pre-release there is no
compatibility obligation (`development_guide.md` §1.6 — version bump, named reject,
no shim), so a second id-42 bump is nearly free.

The tempting counter-premise — one wire surface, therefore one format evolution —
is precisely the position that argument rejects.

## Prior-art and collision map

- **No plan owns id-42 size, RAM, or VRAM reduction** — the open gap this brief closes.
- `lighting-scale--shadowmask-cold-working-set` (done) — restructured the assignment
  seam; compile-time RAM only, byte-identical output, with the explicit non-goal
  "bounding the output below its on-disk size — a format question." Confirms the gap
  is open, not owned. This brief does not touch assignment.
- `lighting-scale--sh-base-atlas-at-rest-slimming` (done) — BC-at-rest precedent for
  ids 34/35, and the splitting argument above. Its section-length-stable posture is
  available but deliberately not taken.
- `static-light-shadowmask-cache-addendum` (done) — the byte-for-byte cache guarantee
  this brief preserves.
- `shadowmask-array-atlas` (done) — closed "no action"; id 42 is already a
  `texture_2d_array` within device limits.
- `shadowmask-bake-scaling` (done) — restructured the composite into streaming
  membership → assignment → fill. The encode phase lands at the pack seam after
  assignment; the composite it builds on is in place.
- `bc7-color-textures` (draft, stub) — owns BC7. Not a dependency either way.
- `shadowmask-atlas-mask-capacity` (draft, gated) — the sibling half, gated on this
  brief's overlap measurement.
- `large-map-spatial-residency` / `sh-probe-streaming` — nothing streams today;
  lightmap, direct-SH and shadowmask all upload whole and stay resident. Compression
  composes with streaming rather than foreclosing it: bytes per resident texel is
  orthogonal to which texels are resident, and re-baking into cluster-addressable
  compressed form is cheap pre-stable. This brief changes no binding count, so it
  cannot disturb that epic's pinned forward-texture-inventory guard.

## Proof shape

Coverage across the full lifetime: production (BC5 encode at the pack seam),
serialization (round-trip, tagged length check, slot rejection), persistence (on-disk
byte delta against the Task 1 baseline), the GPU return (resident byte delta, upload
format, four-way overlap still carries every mask), and cleanup (CPU payload freed,
reload reinstalls). Determinism and graceful degradation close the invariants. BC5
needs no codec-selection gate — a measured error report plus the visual A/B confirm
it.

Focused fixtures only. The ratio is codec-intrinsic and the error texel-local, so no
stress map is baked for proof. Tests run without a GPU context; the upload and visual
A/B are the thin GPU layer, verified through the offscreen capture path.

## Orderings and edges

| id | scenario | ordering / mechanism | expected outcome |
|---|---|---|---|
| format-edges | empty selection, single light, full four-slot table, each tag | header encodes the format tag; `from_bytes` recomputes payload length from the tagged size | every combination round-trips byte-exact |
| budget-boundary | `2 × layer_count` hits the array-layer ceiling exactly | the usability filter evaluates the product before texture creation | equal is kept, one greater degrades to the placeholder; no off-by-one device breach |
| second-group-light | a selected light assigned slot 2 or 3 | both group layers are sampled hoisted, then the per-light select picks `(group, channel)` | the second-group light's shadow renders; first-group output is unchanged |
| dropped-and-placeholder-lit | sentinel slot, or a second-group slot against the one-layer placeholder | the shader clamps the computed array layer, then applies the per-path sentinel guard | both paths return fully lit; neither samples outside the bound texture |
| hoist-preserved | any fragment with several selected specular lights | both samples are issued once per fragment, outside the light loops | sample count per fragment is 2, not 2 × light count |
