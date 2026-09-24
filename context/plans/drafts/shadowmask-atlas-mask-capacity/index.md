# Shadowmask Atlas — Per-Texel Mask Capacity

Brief · resumable · **gated** · reads: `context/lib/rendering_pipeline.md` §4,
`context/lib/build_pipeline.md` §PRL · §Build Cache

`shadowmask-atlas-compress-at-rest` lands first. It changes bytes, never masks, and
it ships the measurement this brief is gated on.

## Re-anchor before building

`shadowmask-atlas-compress-at-rest` (in `ready/`) places its two BC5 groups **side by
side within each layer** — a `2W × H × L` texture, `W` the lightmap layer width — not
stacked in array layers `L..2L`. Contract: `context/lib/build_pipeline.md` §PRL
(ShadowmaskAtlas at rest) and `rendering_pipeline.md` §4 (World specular shadowmask);
derivation in that brief's `research.md` §Group layout. Everything below was written
against stacked groups.

Invalidated, to rework before the gate's measurement is read:

- **Problem, Capacity model, and the low-capacity-bands argument.** Extra groups can
  tile within the layer, bounded by the 8192 texture dimension (≈`(8192/W)²` groups
  with 2D tiling), not by `floor(256/L)`. The layer budget stays untouched. Capacity
  now falls as `W` grows, not as `L` grows: many small leaves mean small `W` and ample
  room. The scarce band is a few huge leaves, `W` ≥ 4096. `W` = 8192 already ships
  with no shadowmask, a band this brief inherits.
- **"The sample leaves the hoist."** Forced only when groups grow on demand. A small
  fixed group count — four groups carry the gate's likely 5–8 outcome — may keep every
  sample hoisted in uniform control flow. Decide this with the layout; it is no longer
  a settled price.
- **Open question and non-goals on two textures and array consolidation.** In-layer
  tiling reaches capacity at one binding with BC5 bytes, so the two-texture layout is
  dominated, not blocked. The consolidation sequencing argument likely drops out.
- **Mechanics.** The filter compares tiled dimensions against
  `max_texture_dimension_2d`, not a layer-group product. The placeholder is 2×1 white.
  "Correct array layer" rows become "correct tile". The per-group half-texel clamp
  generalizes to every tile edge, in `v` as well as `u` under 2D tiling.

Still holds: the gate, greedy assignment once slots are abundant, sentinel widening
(needed only if slots can reach 256), byte-identical output, and the double-count
dead-zone. The overlap report now arrives on every bake (memo-carried); read it on
`stress-warren-hallway-inspection`, the performance yardstick, as well as other
representative content.

The question the rework answers: **tile groups within the layer — row or 2D — and
does a small fixed group count keep the hoist?**

## Gate

**Do not build this until `shadowmask-atlas-compress-at-rest` has landed and reported
peak observed per-texel overlap on real content.**

Greater-than-four per-texel overlap is **rare and unmeasured**.
`static-light-shadowmask-world-receipt` judged it rare enough for a compiler warning
plus a global drop, and no observation since has contradicted that — no bug, no
review finding, no modder report. `stress-warren-lit`'s 157 lights is a map-wide
count, not a per-texel one. This is a build-ahead lift, which is legitimate — but its
justifying measurement must not be a deliverable of the fix itself, or the premise
stays unfalsifiable until after the work is done. Hence the gate, and hence the
measurement belonging to the sibling brief.

The gate has three outcomes, and the middle one is the most likely:

- **Peak overlap ≤ 4 across representative content** — close this brief unbuilt. The
  compress-at-rest brief already delivers four masks at half the bytes, and there is
  no defect here to fix.
- **Peak overlap 5–8 on some content** — build it, sized to what was measured. The
  layout question below becomes answerable rather than speculative.
- **Peak overlap far above 8** — reconsider selection first. Mask pileup is downstream
  of `entity_shadow_min_intensity_ratio` and `entity_shadow_min_range`, and a
  selection that admits a dozen overlapping shadow-casters on one texel may be the
  actual defect, not the atlas's capacity.

## Problem

A texel carries at most four static-light masks. A fifth overlapping selected light
is dropped lowest-intensity first and its runtime shadow disappears. Under the
compress-at-rest brief that ceiling is unchanged — BC5 at a group count fixed at two
seats exactly the same four.

Raising it means letting the group count grow, which spends the array-layer budget:
groups stack into the same `max_texture_array_layers` pool, pinned at 256 and
requested as a hard limit at device acquisition, with the lightmap packer capped at
`MAX_ATLAS_LAYERS = 256` layers of its own.

## Capacity model

With `L` = lightmap `layer_count` and group count `P ≤ floor(256 / L)`:

| Layout | Masks/texel | Bytes/texel/mask | Bindings | Falls below four at |
|---|---|---|---|---|
| Today, and compress-at-rest | 4 | 1.0 / 0.5 | 1 | never |
| BC4 single-channel planes | `floor(256/L)` | 0.5 | 1 | L ≥ 65 |
| BC5 `.rg` pairs, P grown | `2 × floor(256/L)` | 0.5 | 1 | L ≥ 129 |
| Raw `Rgba8Unorm`, P grown | `4 × floor(256/L)` | 1.0 | 1 | never |
| Two BC5 textures, P grown | `4 × floor(256/L)` | 0.5 | 2 | never |

Read the last two rows together. **Capacity and compression are two levers trading
against each other on one 256-layer budget.** BC5 halves bytes per mask and halves
masks per group; raw keeps four masks per group at full bytes. Group-addressed raw
reaches the two-texture layout's exact mask capacity at one binding, today, with no
encoder and nothing blocking it — the second binding buys that capacity *at half the
bytes*, which is bytes, not masks.

The BC4 plane row is recorded because single-channel planes are the obvious first
shape, and the row shows why they are dominated: BC5 is literally two BC4 blocks in
one 16-byte block, so `.rg` pairs give the same ratio at twice the capacity, reusing
`encode_bc5_rg` rather than needing a single-channel wrapper.

**Why the low-capacity bands are not merely theoretical.** `layer_count` tracks BVH
leaf structure, not map size or authored density. `choose_layer_dim` sizes the shared
per-layer dimension to host the single largest leaf, then spills the rest into
further layers under leaf cohesion, so a map of many small leaves gets a small
dimension and multiplies layers — 65 layers at 128² is roughly 1,700 m² of lit
surface at the default density, an ordinary mid-size level. `_lightmap_density` is
not a usable dial against this: finer density scales charts and the largest leaf
together so the layer count stays roughly flat until the 8192 cap, and coarsening can
*raise* it. Authors have no reliable control over `L`. That is why any capacity claim
here has to hold as a format property rather than as an authoring warning — a warning
would name a condition authors cannot act on.

## Decisions

Deliberately few. The layout choice is the open question below, and most downstream
decisions follow from it. These hold under any layout:

- **Greedy first-fit assignment, retiring the exact search.** Under groups grown on
  demand, colours are abundant, so the scarce-four-colour problem that
  `assign_channels_with_drops_controlled`'s bounded exact search and its
  `SHADOWMASK_COLOR_SEARCH_NODE_BUDGET` fallback exist to solve disappears. A
  deterministic greedy first-fit — stable selection order, lowest free slot, at most
  Δ+1 slots — is complete, uses at most one more slot than optimal in the cases that
  matter, and drops only at the array-layer ceiling. This makes no-drop-below-budget
  true by construction and removes a search-budget-dependent nondeterminism source.
  Do not reintroduce search for compactness near the ceiling. **This is correct only
  once slots are abundant** — it would be a regression at four.
- **The slot index must widen, and its sentinel must move.** Today `0xFF` is safe as
  the dropped sentinel only because slots occupy `0..3`. A group-addressed slot
  reaches the full 8-bit range including `0xFF` (at `L = 1` the ceiling is 256
  groups). Widen the wire element and place the sentinel outside the representable
  slot range. The shader's dropped-sentinel comparison constant, presently one past
  the last RGBA channel, moves above every representable slot in step.
- **The sample leaves the hoist, and control flow stops being uniform.** With groups
  grown on demand each light's mask sits on its own array layer, so the sample moves
  inside both light loops. Both contain `continue` and `break`, so the
  implicit-derivative sampling function is illegal there — the explicit-level form is
  required, and since the atlas carries one mip level this changes no sampled value.
  The real cost is **one sample per fragment becoming one per contributing static
  light**. This is intrinsic to capacity and survives into every layout in the table
  above; it is the price of the feature, and it carries a frame-time acceptance row
  rather than going unmeasured.
- **Preserve graceful degradation and the double-count dead-zone.** Absent, rejected,
  or over-budget data still resolves to fully lit via
  `filter_usable_shadowmask_section`, extended to compare the layer-group product.
  The static→static union subtraction is untouched: this changes where a mask lives,
  never the union term.
- **Byte-identical output.** `static-light-shadowmask-cache-addendum` ships a live
  byte-for-byte guarantee for this section, and the in-tree BC paths are pure
  functions. The BC6H lossy exemption stays available and untaken, as in the
  compress-at-rest brief.

### Non-goals

- **Compression.** `shadowmask-atlas-compress-at-rest` owns it and lands first.
- **Selection eligibility and ranking.** Out of scope, but note the gate above: if
  measured overlap is extreme, selection is the better lever.
- **The lightmap array-consolidation refactor**, and the two-texture layout that
  depends on it. See the open question.
- **Streaming or residency.** `large-map-spatial-residency`.

## Open questions

**Which layout — and it is a real question, not a formality.** The answer depends on
the measured peak overlap the gate produces, and on whether bytes or masks matter more
at that number.

*Group-addressed raw* is the shortest route to capacity: `4 × floor(256/L)` masks at
one binding, no encoder, no fidelity gate, the existing channel-select decode, and
nothing blocking it. It costs bytes — it would partly give back what the
compress-at-rest brief just won, and the two cannot both apply to the same section
without a per-level choice between them.

*BC5 pairs with the group count grown* keeps the compression and reaches
`2 × floor(256/L)`, which is ample at realistic layer counts and falls below four only
past 128 layers. This is the natural continuation of the compress-at-rest brief —
it changes the group count from a constant to a computed value and widens the slot
index, and little else.

*Two BC5 textures* gets both, and is genuinely blocked. The forward pass already
requests exactly 16 sampled textures per stage, which `renderer_init_resources.rs`
documents as the WebGPU spec floor; a 17th breaks that portability guarantee, and the
count is 15 without `CUBE_ARRAY` so the cube-array case sets the ceiling. The only
mergeable pair is the two direction atlases — they share the octahedral encoding,
`decode_lightmap_direction`, and the Nearest sampler, and `direction_texture_format`
already supports both their formats — but the animated one is compute-written while
the static one is upload-once, and the two index array slices differently. That merge
is the lightmap array-consolidation `static-light-shadowmask-world-receipt` banked,
and it must sequence behind `sh-probe-streaming`, whose acceptance pins the forward
texture inventory a consolidation would rewrite and which names lightmap layers as
its next generalization subscriber.

Recommendation, pending the measurement: **BC5 pairs with a grown group count.** It
preserves the at-rest win, needs no binding and no new epic, and reaches capacity
that only fails past a layer count the gate's measurement can check for directly.
Reach for group-addressed raw only if measured overlap exceeds what pairs can seat at
the layer counts real maps produce.

## Acceptance

Written against the recommended layout. If the gate selects another, the byte-related
rows change and the rest hold.

- [ ] On a fixture with a lightmap texel overlapped by 5–8 selected static lights,
      every selected light receives a non-sentinel slot and its shadow is present at
      runtime.
- [ ] A mask is dropped only when the next group would push the layer-group product
      past the engine's pinned array-layer maximum. At or below it, nothing is
      dropped. The drop, when it occurs, is the lowest-intensity mask, matching the
      pre-change policy.
- [ ] Slot assignment is collision-free: for every overlap edge in the selection
      graph the two lights hold different slots, or one holds the sentinel. Asserted
      over the assignment output, independent of any rendered texel.
- [ ] No drop arises from search-budget exhaustion — that path and its node budget no
      longer exist.
- [ ] Round-trip and rejection hold for a multi-group section: `to_bytes` →
      `from_bytes` reproduces header, widened slot table and payload; `from_bytes`
      rejects an out-of-range non-sentinel slot and a payload length disagreeing with
      the tagged format's block arithmetic.
- [ ] The widened sentinel cannot collide with a representable slot at any group
      count the layer budget admits — asserted, not assumed.
- [ ] A section whose layer-group product exceeds the pinned maximum is rejected to
      the all-visible placeholder with a `[Renderer]` error and no panic; a product
      exactly equal is retained, one greater degrades.
- [ ] A sentinel slot reads fully lit in both decode paths, and a baked non-zero group
      index reads fully lit rather than sampling out of range against the one-layer
      placeholder.
- [ ] Static→static world shadowing stays exactly zero: adding a light that lands in a
      non-zero group does not change world-specular output for surfaces already
      covered by group-zero lights.
- [ ] Both decode paths resolve a light in any group to the correct array layer, and
      neither samples through a call requiring uniform control flow — a
      source-inspection gate.
- [ ] Control: four-way overlap carries every mask with no drop, at any group count.
- [ ] Frame time on the world specular path is measured against the compress-at-rest
      baseline on a scene whose fragments carry several selected static specular
      lights, and the per-fragment sampling cost is recorded in the landing note.
      This is the feature's price and it is reported, not hidden.
- [ ] Re-baking a fixture twice yields a byte-identical section across differing
      worker-thread counts; slot-open order is a pure function of a stable ordering
      key, not of chart-worker or iteration order.
- [ ] The bake warns without `--verbose` only when a mask is actually dropped, naming
      the lights that lost masks and the ceiling that caused it, and reports layer
      count, group count, capacity and peak overlap under `--verbose`.

## Path

- **Assignment seam.** `crates/level-compiler/src/shadowmask_bake/assignment.rs` holds
  the overlap graph and the exact-search plus priority-greedy machinery this brief
  retires; `shadowmask_bake/fill.rs` holds the fill that consumes the assignment.
  Re-anchor on `lighting-scale--shadowmask-cold-working-set` (landed), which derives
  the overlap graph analytically. One foreclosure it already created: the deleted
  per-(light, texel) membership record was the only structure where per-texel
  visibility and cross-light adjacency coexist, so a contribution- or
  coverage-weighted retention priority would have to re-materialize that term.
  Intensity-ordered retention reads light parameters only and is unaffected.
- **Format.** Extends the compress-at-rest brief's header tag with a group count and
  a widened slot element. Additive — pre-release there is no compatibility
  obligation, so a second id-42 section-version bump is nearly free.
- **Upload and filter seam.** `crates/renderer/src/lighting/lightmap.rs`; the filter
  compares the layer-group product and the texture's array dimension becomes that
  product.
- **Runtime linchpin.** `crates/renderer/src/render/shadowmask.rs` carries the slot
  values and the promoted-light metadata; both the value width and the sentinel
  comparison constant change here.
