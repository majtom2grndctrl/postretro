# Shadowmask No-Drop Atlas — Stacked Channel Blocks for >4 Overlap

## Goal

Stop dropping entity-shadow masks when more than four selected static lights overlap a
single lightmap texel. Today the shadowmask atlas packs per-light masks into the 4
RGBA channels of one `Rgba8Unorm` texel, so a 5th overlapping light's mask is dropped
(lowest intensity first) and its shadow disappears at runtime. Add a **block** dimension
stacked in the atlas's array layers so a texel can carry `4 × block_count` masks,
addressing each light's mask by a `(block, channel)` slot, and drop only when the
device array-layer budget (`layer_count × block_count ≤ max_texture_array_layers =
256`) is actually exceeded — a far higher ceiling than four.

## Scope

### In scope

- Extend `ShadowmaskAtlasSection` (`crates/level-format/src/shadowmask_atlas.rs`) with a
  block dimension: the per-selection channel table encodes a `(block, channel)` slot,
  the payload is layer-major over `layer_count × block_count` array layers, and the
  `<= 3` channel validation relaxes to the slot range. Advance the section/format
  version; section id stays 42.
- Compiler: replace the 4-color-with-drops assignment (`assign_channels_with_drops`)
  with a block-aware slot assignment that spills overlap into additional blocks and
  drops a mask only when `layer_count × block_count` would exceed the device array-layer
  budget. Deterministic and byte-stable.
- Renderer upload/view/filter (`crates/renderer/src/lighting/lightmap.rs`): stack the
  blocks in the `texture_2d_array` (`depth_or_array_layers = layer_count × block_count`),
  and extend `filter_usable_shadowmask_section`'s layer bound to the new product.
- Runtime linchpin (`crates/renderer/src/render/shadowmask.rs`,
  `crates/lighting/src/spec_buffer.rs`): carry the `(block, channel)` slot through the
  `SpecLight` shadowmask field (byte 56) and the promoted-record `meta1.z` field
  (byte 24) instead of a bare 0..3 channel, preserving the dropped sentinel.
- Shader (`crates/renderer/src/shaders/forward.wgsl`): decode the slot to
  `(block, channel)`, sample the array layer `lightmap_layer + block × layer_count`,
  and select the channel, in both the world-specular and promoted-union paths.
- Preserve the graceful-degradation contract (absent / rejected / over-budget → fully
  lit) and the static→static double-count invariant.

### Out of scope / non-goals

- **The shadowmask bake's memory / parallelism / progress.** Owned by
  `shadowmask-bake-scaling` (the sibling). This plan changes what the bake *emits* and
  how it is assigned to slots, not the bake's allocation lifecycle. It builds on that
  plan's streaming composite (see Sequencing).
- **The lightmap / irradiance / direction atlases.** Untouched; `lightmap_layer` (the
  receiver's spatial array layer) keeps its meaning. Only the shadowmask atlas gains
  the block dimension.
- **Old-`.prl` migration.** All fixtures re-bake from source (project posture); the
  format version advances and stale caches regenerate.
- **Raising the per-texel cap above the device array-layer budget.** Past
  `layer_count × block_count = 256` the existing drop is retained as the graceful
  fallback (see Direction — this is a chosen, owner-visible ceiling, not a foreclosed
  case).
- **Selection eligibility / ranking.** Which lights are selected
  (`entity_shadow_select`) is unchanged.

## Direction

**Problem.** The shadowmask atlas has exactly four per-texel mask slots (the RGBA
channels of one `Rgba8Unorm` texel; the array layer is the receiver's `lightmap_layer`,
not a per-light axis), so >4 overlapping selected lights force a mask drop and a
missing runtime shadow.

**Observation and why now.** The per-texel frequency of >4 overlap on *today's* content
is unmeasured — `static-light-shadowmask-world-receipt` judged it rare enough to handle
with a compiler warning and a global drop, and the 157 lights on `stress-warren-lit` are
a map-wide count, not a per-texel one. This plan lifts the ceiling anyway, as a
**deliberate build-ahead owner decision**: stress warrens already approximate a real
map's square footage but real maps will carry more geometry and more lights, so the
right design bar is materially more headroom than four, chosen ahead of a per-texel
receipt rather than after it. That premise is what justifies the format surface; it is
stated plainly here so a later reader knows the frequency was a decision input, not a
measured fact.

**Prior commitments.** `rendering_pipeline.md` §4 commits that absent, rejected, or
dropped shadowmask data is fully lit, and that static→static world shadowing stays
exactly zero via the pool-shadow union-subtraction dead-zone (the double-count
invariant). Both are preserved: the block expansion changes only how a light's mask is
*located*, not the degradation or dead-zone logic, and drop survives as the
over-device-budget fallback. `build_pipeline.md` id 42 states masks are "packed into
RGBA channels, with `0xFF` ... for globally dropped masks" — this plan **revises** that
statement (masks in `(block, channel)` slots; drop only past the device layer budget)
and updates the doc at promotion.

`static-light-shadowmask-world-receipt` banks a "lightmap array-consolidation refactor"
as the fallback **if a future feature needs the array-layer headroom**. This plan is that
feature: its blocks stack into the same `max_texture_array_layers = 256` pool the
lightmap array atlas already occupies (`atlas layers = layer_count × block_count`). The
interaction is explicit here rather than passed silently — block headroom is
`floor(256 / layer_count)`, so on a high-`layer_count` map it is small, and if the
banked consolidation later shrinks `layer_count` (packing more charts per spatial layer)
it *frees* block headroom, while a future finer `_lightmap_density` that grows
`layer_count` *spends* it. The two are coupled through the shared budget; this plan does
not trigger the consolidation, but names the coupling so whoever lands consolidation
weighs shadowmask blocks in the layer budget. That the predecessor's per-`>4` **compiler
warning** was an author-facing signal (a spot is over-piled with lights) is preserved in
spirit, not silently absorbed — see Task 2 and AC 8.

**Placement.** The change is a runtime-format capacity expansion, so it spans the wire
format, the compiler's slot assignment, the renderer upload/metadata, and the shader —
each edited at its own boundary, with the `(block, channel)` encoding pinned once
(Boundary inventory). The block dimension is stacked in the existing `texture_2d_array`
rather than in a second texture, so the upload/view/filter path and the single atlas
binding are reused; the cost is the `layer_count × block_count ≤ 256` device
interaction, handled explicitly.

**Alternatives rejected.** (a) A **budget-neutral fixed channel widening** — a second
sampled texel or a wider format giving 8 masks/texel with zero interaction with the
shared 256-layer budget. Genuinely appealing (it sidesteps the consolidation coupling
above), but rejected against the build-ahead bar: a fixed ×2 is a one-time step to 8,
and the design target is "materially more than four" for maps heavier than the current
warrens, which a fixed factor does not scale to. Block-stacking reaches
`4 × floor(256/layer_count)` and degrades gracefully at the device ceiling; the
consolidation coupling is the accepted price, made explicit above. (b) A second
shadowmask texture/binding per channel group — adds a bind slot + BGL entry + shader
sample per group and does not scale past two groups. (c) A variable-length per-texel
light list with an indirection buffer — a large shader and data-model departure with
poor GPU fit. (d) **Measure first, then decide** — a per-texel selected-light histogram
on a real warren before committing any format. Rejected by the owner as build-ahead: the
target is a higher bar than today's content, so a today-frequency receipt would not
change the decision. (Task 2 still emits the histogram as a `--verbose` diagnostic, so
the number exists — it is just not a gate.) (e) Keep dropping but rank better — still
drops shadows, which the directive forbids.

**Forecloses.** The stacked-block representation caps per-texel capacity at
`4 × (256 / layer_count)` masks; on a high-`layer_count` map (many spatial atlas
layers) that ceiling can bind, and the wire format encodes the block dimension, so
changing the representation later is another format bump. This is the deliberate,
owner-visible ceiling named in Out of scope: past it, the existing lowest-intensity
drop is the graceful fallback rather than an error. The `.prl`/cache one-way door is
moderate and mitigated by the re-bake-from-source posture (no stored map to migrate);
the harder-to-reverse cost is **content expectation** — once maps are authored assuming
>4 shadows render, backing the format out re-introduces the gap on content that now
depends on it. Accepted as part of the build-ahead decision.

## Boundary inventory

The per-light mask location crosses Rust → wire → runtime f32 → shader. Pin the
encoding once: a **slot** `s = block * 4 + channel`. The dropped sentinel is retained
per surface. Field widths are implementation choices (state the constraint, not the bit
layout); the constraint is that every field below must represent slots `0 ..
(block_count * 4 − 1)` plus the dropped sentinel.

| Name | Rust (compiler) | Wire / serde | Runtime f32 | Shader |
|---|---|---|---|---|
| Mask slot | `channels: Vec<u8>` entry = slot, `0xFF` = dropped | `channels` bytes, header carries `block_count`; payload layer-major over `layer_count × block_count` | `SpecLight` byte 56 and promoted `meta1.z` = `slot as f32`, `>= dropped-sentinel` = none | decode `block = s / 4`, `channel = s % 4`; array layer = `lightmap_layer + block * layer_count` |

## Wire format

`ShadowmaskAtlasSection` mirrors its current little-endian layout (16-byte-aligned
header, then `channels`, pad to 4, then the layer-major `Rgba8Unorm` payload), extended
by:

- A `block_count: u32` in the header (or an equivalent single integer from which the
  total array-layer count `layer_count × block_count` is recovered). Empty selection
  encodes as today (`selected_light_count = 0`).
- `channels` entries now range `0 .. (block_count * 4 − 1)` or `0xFF` (dropped); if the
  slot range can exceed a `u8`, widen the channel-table element type — an
  implementation choice, but the validation must accept the full slot range and reject
  out-of-range non-sentinel values, replacing the current `<= 3 || 0xFF` gate.
- `data` payload length is `width × height × (layer_count × block_count) ×
  SHADOWMASK_TEXEL_BYTES`; `from_bytes`' payload cross-check uses the product.

Section id stays 42; advance the format/PRL version so stale caches and fixtures
regenerate. State explicitly in the code that the layout mirrors the pre-block
shadowmask section plus the block dimension.

## Acceptance criteria

- [ ] On a fixture with a lightmap texel overlapped by more than four selected static
  lights (e.g. 6–8), every selected light receives a non-dropped `(block, channel)`
  slot in the baked section, and each light's shadow is present at runtime — no mask is
  dropped below the device array-layer budget.
- [ ] A mask is dropped only when assigning it would push `layer_count × block_count`
  past `max_texture_array_layers`; at or below the budget, no drop occurs. The drop,
  when it happens, is the lowest-intensity mask, matching the pre-change policy.
- [ ] `ShadowmaskAtlasSection::to_bytes` → `from_bytes` round-trips the `(block,
  channel)` slot table and the layer-major payload for `block_count > 1`; `from_bytes`
  rejects an out-of-range slot and a payload whose length disagrees with `width ×
  height × layer_count × block_count × 4`.
- [ ] A section whose `layer_count × block_count` exceeds the device budget is rejected
  by `filter_usable_shadowmask_section` with a `[Renderer]` error and the all-visible
  placeholder (fully lit), no panic — the existing graceful contract extended to the
  new layer math.
- [ ] Static→static world shadowing stays exactly zero: adding a light that lands on
  `block > 0` does not change world-specular output for surfaces already covered by
  block-0 lights (the pool-shadow union dead-zone is unaffected).
- [ ] Both shader decode paths (world-specular `cone_cos.z`, promoted-union `meta1.z`)
  sample the correct array layer `lightmap_layer + block × layer_count` and channel for
  a light on any block; shader tests covering `block > 0` pass.
- [ ] Re-baking the same map twice yields byte-identical shadowmask output
  (deterministic slot assignment).
- [ ] A single-block map (`block_count == 1`) produces a section semantically
  equivalent to the pre-change 4-channel layout (same masks, same drop-nothing
  behavior when ≤4 overlap).
- [ ] The authoring signal is preserved: the bake logs a warning when overlap forces
  the block count near the device array-layer budget (a spot over-piled with lights),
  and reports the peak observed per-texel overlap and the resulting `block_count` under
  `--verbose`. A non-verbose bake with comfortable headroom gains no new log spam.

## Tasks

### Task 1: End-to-end `(block, channel)` slot — thin vertical slice

Thread one `(block, channel)` slot through every layer and prove a >4-overlap texel
renders all its masks, before hardening. Extend `ShadowmaskAtlasSection` with the block
dimension per the Wire format section (header `block_count`, slot-encoded `channels`,
layer-major payload over `layer_count × block_count`, relaxed validation, advanced
version). In the compiler composite, assign selected lights to slots
`s = block * 4 + channel` so that ≤4-overlap maps to block 0 (reproducing today's
channels) and a 5th+ overlapping light spills to block 1 rather than dropping — a
minimal assignment is enough for this slice; the full deterministic policy and
device-budget cap are Task 2. In the renderer, upload the payload with
`depth_or_array_layers = layer_count × block_count` and thread the slot through the
runtime linchpin: `build_spec_light_shadowmask_channels` and
`pack_forward_shadowmask_metadata` write `slot as f32` (preserving the dropped
sentinel) into the `SpecLight` byte-56 field and the promoted-record `meta1.z` field
instead of a 0..3 channel. In `forward.wgsl`, decode `block = slot / 4`, `channel =
slot % 4`, sample the array layer `lightmap_layer + block × layer_count` via a
generalized `sample_shadowmask_atlas`, and select the channel via
`shadowmask_channel_value`, in both the world-specular and promoted-union paths. The
slice is proven when a fixture texel overlapped by 5–8 selected lights shows every
light's shadow at runtime (none dropped). This falsifies the wire ↔ runtime ↔ shader
boundary end to end.

### Task 2: Deterministic block assignment + device-budget cap + graceful degradation

Replace the composite's 4-color-with-drops assignment
(`assign_channels_with_drops`/`color_graph`) with a block-aware slot assignment that
packs each selected light into a `(block, channel)` slot so no two lights overlapping a
common texel share a slot, opening additional blocks as overlap demands, and drops a
mask (lowest intensity, as today) only when opening another block would push
`layer_count × block_count` past `max_texture_array_layers`. Assignment must be a pure,
order-deterministic function of the selection and per-light layer inputs so the section
is byte-stable (re-bake identical). The compiler must know the device array-layer
budget to cap `block_count`; thread the same `max_texture_array_layers` bound the
renderer enforces (a shared constant) into the bake so the cap is enforced at bake
time, and extend `filter_usable_shadowmask_section` so a section whose `layer_count ×
block_count` exceeds the runtime device limit still degrades gracefully to the
all-visible placeholder with a `[Renderer]` error (never a panic). Preserve the
static→static double-count dead-zone in the promoted-union path unchanged — the slot
generalization changes mask lookup, not the union subtraction's world-surface
dead-zone. Preserve the authoring signal the predecessor's `>4`-overlap warning gave:
track the peak observed per-texel overlap during assignment, warn when the block count
it forces approaches the device array-layer budget (an over-piled spot the author may
want to thin), and report the peak overlap and final `block_count` under `--verbose`;
a comfortably-under-budget bake emits no new non-verbose line.

### Task 3: Round-trip, shader, and invariant coverage

Add the tests that lock the contract: a `to_bytes`/`from_bytes` round-trip over a
`block_count > 1` section (and rejection of an out-of-range slot and a mismatched
payload length); a shader test that a light on `block > 0` samples the correct array
layer and channel in both decode paths (extending the existing
`render/tests/shader_tests.rs` shadowmask cases); a bake test that a >4-overlap texel
drops nothing below the device budget and drops lowest-intensity only past it; and a
double-count regression asserting world-specular output is unchanged when a block-0-lit
surface gains a `block > 0` light. Update the `build_pipeline.md` id-42 statement and
the `rendering_pipeline.md` §4 world-specular shadowmask statement at promotion to
describe the `(block, channel)` slots and the device-budget drop.

## Sequencing

**Cross-plan dependency:** land `shadowmask-bake-scaling` first. That plan restructures
the shadowmask composite into a streaming membership → assignment → fill shape; this
plan changes the *assignment* step (slots instead of 4-color drops) and the emitted
format. Building on the streamed composite means the slot assignment is a change to one
well-scoped step, not a re-litigation of the materialization. If this plan must land
first, its Task 2 assignment still slots into the current
`assign_channels_with_drops` seam, but then collides with the bake-scaling restructure
— avoid by ordering.

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the wire ↔ runtime ↔ shader
boundary with a minimal assignment.
**Phase 2 (sequential):** Task 2 — consumes Task 1's format and slot plumbing; adds the
deterministic assignment, device-budget cap, and graceful degradation.
**Phase 3 (sequential):** Task 3 — verifies Task 1/2 behavior and the preserved
invariants; consumes the finished format and assignment.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No selected mask dropped while `layer_count × block_count ≤ max_texture_array_layers` | Task 2 (block-spilling assignment) | Any residual 4-slot cap in assignment, metadata, or shader re-imposes the drop; Task 1's minimal assignment is provisional | AC 1, 2 |
| Absent / rejected / over-budget shadowmask → fully lit, no panic | existing `filter_usable_shadowmask_section` | Task 2 extends the layer bound to `layer_count × block_count`; a missed bound is a device breach | AC 4 |
| Static→static world shadowing exactly zero (pool-shadow union dead-zone) | existing promoted-union path | Task 1/2 change mask lookup, not the dead-zone; a slot decode that alters the union term breaks it | AC 5 |
| Shadowmask output bytes deterministic in (selection, per-light layer inputs, atlas dims) | Task 2 (order-deterministic slot assignment) | non-deterministic block opening order changes bytes | AC 7 |

## Rough sketch

Slot transport is pinned in the Boundary inventory (`s = block * 4 + channel`, array
layer `lightmap_layer + block × layer_count`). Assignment (Task 2) is graph coloring
with `4 × block_count` colors instead of 4, `block_count` grown on demand up to
`floor(max_texture_array_layers / layer_count)`, dropping lowest-intensity only when
the next block would exceed it — a generalization of the current `color_graph` loop,
not a new algorithm. The renderer already views the shadowmask as `D2Array` and uploads
`LayerMajor`; only the layer count and the per-light slot decode change. `block_count ==
1` must reproduce the pre-change section and behavior (AC 7) so the common case is
unaffected.
