# Shadowmask Atlas — Per-Texel Mask Capacity — Research

Grounding for `index.md`. Facts cited by symbol. No line numbers — they stale.

Format, codec and lifecycle grounding lives in
`shadowmask-atlas-compress-at-rest/research.md`, which lands first. This file covers
only what capacity turns on: the layer budget, the binding ceiling, the assignment
rewrite, the shader cost, and the sequencing behind the residency epic.

## The ceiling, and what sets it

Today a texel's masks are the four RGBA channels of one `Rgba8Unorm` texel at array
layer `lightmap_layer`. The array layer is the receiver's *spatial* axis, not a
per-light one, so four is a hard per-texel cap. A fifth overlapping selected light is
dropped lowest-intensity first (`assign_channels_with_drops_controlled`), and a
dropped light frees zero bytes — the buffer stays fully allocated.

Raising the cap means a second addressing axis, and the only one available is the
array dimension: group *g* at `lightmap_layer + g × layer_count`. Groups stack into
the same `max_texture_array_layers` pool, which
`REQUIRED_MAX_TEXTURE_ARRAY_LAYERS = 256` requests as a hard limit and pre-checks
against the adapter, so the granted limit is exactly 256. The lightmap packer is
independently capped at `MAX_ATLAS_LAYERS = 256`.

## Why layer count is not author-controlled

`choose_layer_dim` (`crates/level-compiler/src/lightmap_bake.rs`) sizes the shared
per-layer dimension to host the **single largest BVH leaf**, growing by doubling and
capped at `MAX_ATLAS_DIMENSION` (8192), then spills remaining leaves into further
layers under leaf cohesion — a leaf that does not fit rolls whole to a fresh layer.
`round_atlas_dim` rounds each axis to a power of two, at least
`MIN_ATLAS_DIMENSION` (64).

So layer count tracks leaf structure, not map size. A map of many small leaves gets a
small per-layer dimension and multiplies layers. 65 layers at 128² is ~1.06M texels,
roughly 1,700 m² of lit surface at the default 0.04 m/texel — an ordinary mid-size
level, not a stress map.

`_lightmap_density` is not a usable dial. Finer density scales charts and the largest
leaf together, so the dimension grows in step and the layer count stays roughly flat
until the 8192 cap, past which finer density does spill. Coarsening density to save
memory can shrink the dimension and *raise* the layer count. The relationship is
non-monotonic, and authors would have no way to predict it.

What authors do control is per-texel light pileup — placement, plus the
`entity_shadow_min_intensity_ratio` and `entity_shadow_min_range` selection floors.
That is why any capacity claim has to hold as a format property rather than as an
authoring warning: a warning would name a condition authors cannot act on. It is also
why extreme measured overlap points at selection rather than at the atlas.

## Frequency is a decision input, not a measured fact

Per-texel greater-than-four overlap on today's content is **unmeasured**.
`static-light-shadowmask-world-receipt` judged it rare enough for a compiler warning
plus a global drop. `stress-warren-lit`'s 157 lights is a map-wide count, not a
per-texel one. No bug, review finding, or modder report has contradicted that since.

So this is a build-ahead lift, and it has to be named as one. The inversion to avoid
is letting the justifying measurement become a deliverable of the fix — that leaves
the premise unfalsifiable until the work is already done.
`shadowmask-atlas-compress-at-rest` ships the `--verbose` overlap instrumentation, and
this brief is gated on what it reports.

## Why BC5 pairs dominate BC4 planes

Single-channel BC4 **planes** — one mask per array layer — carry
`floor(256 / layer_count)` masks per texel. That is the obvious first shape, and it is
dominated.

`crates/level-compiler/src/bc5.rs` documents and implements BC5 as exactly two BC4
blocks in one 16-byte block — per 4×4 block, a BC4 R block then a BC4 G block. So
`.rg` **pairs** per group give the identical ratio (0.5 bytes per texel per mask
under either) at the same single binding, with **twice** the masks per texel, reusing
`encode_bc5_rg` verbatim rather than needing a single-channel BC4 wrapper exposed.
The plane layout was paying a capacity penalty for nothing.

Crossover below today's four moves from `layer_count ≥ 65` (planes) to
`layer_count ≥ 129` (pairs). Note `floor(256 / L)` holds at 4 through L = 64 and drops
to 3 at L = 65; L ≥ 52 is where the plane ceiling stops being *better* than four, not
where it regresses.

## The binding ceiling, and the only slot available

Two BC5 textures would give `4 × floor(256 / layer_count)` masks per texel — never
below today's four at any layer count up to the 256 cap — at half today's bytes per
mask. It is blocked.

`renderer_init_resources.rs` documents that the forward pass requests exactly the
sampled-texture count its BGLs compose — 16 with `CUBE_ARRAY`, 15 without — and that
16 is the WebGPU spec floor. A 17th breaks the stated portability guarantee. The
cube-array case sets the ceiling, so the no-cube-array slack cannot be borrowed.

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
bridged: `direction_texture_format` already supports both `DIRECTION_FORMAT_OCT_RG8`
(`Rg8Unorm`) and `DIRECTION_FORMAT_OCT_RGBA8` (`Rgba8Unorm`), the animated atlas's
format.

Two blockers on the merge:

1. The animated atlas is **compute-written** — created with `STORAGE_BINDING` and
   composed each frame by the animated-lightmap compute pass — while the static one is
   upload-once with `TEXTURE_BINDING` only. A merged texture needs storage usage, with
   the compute pass confined to its slice range.
2. They **index array slices differently**: static by lightmap layer, animated by
   dense animated slot through the group-4 binding-7 lookup. The merge needs a unified
   slice scheme.

This is the "lightmap array-consolidation refactor" that
`static-light-shadowmask-world-receipt` banked as the fallback for a feature needing
array-layer headroom.

**But note what this does and does not block.** Group-addressed raw `Rgba8Unorm` —
slot `s` addressing group `s / 4` and channel `s % 4` — reaches the same
`4 × floor(256 / layer_count)` mask capacity at **one** binding, today, with no
encoder and the existing channel-select decode. The second binding buys that capacity
at half the bytes. Capacity is not blocked; capacity-with-compression is. Reading
the two-texture layout as *the* capacity endgame overstates the block, and the
capacity table's own arithmetic is what shows it.

## Consolidation sequencing after SH residency

SH residency (Slices 1–3 of `context/plans/in-progress/sh-probe-streaming/`)
shipped in PR #516. Its former in-flight sequencing constraint is satisfied;
the parent epic remains open for independent authored hints. The original
ordering mattered at three points of contact:

1. **A pinned guard.** Its acceptance includes that the SH sampler gains no binding
   and no per-fragment locate-read, and that the forward fragment texture inventory
   and compose BGL budgets are unchanged — with a runnable guard named,
   `forward_pipeline_sampled_texture_request_matches_bgl_definitions`. A consolidation
   changes that inventory from 16 to 15, so one epic pins the number the other
   rewrites and "unchanged" loses its baseline mid-flight.
2. **Overlapping substance.** Consolidation's hard part is a unified array-slice index
   scheme; streaming's is dynamic array-slice residency, with cluster install and
   eviction rewriting which slice holds what. `sh-probe-streaming` explicitly names
   lightmap layers as its next subscriber through its generalization door and flags
   that its directory shape must not foreclose a second resource cheaply.
3. **A new compute writer.** Consolidation makes the static direction atlas
   compute-written for the first time, adding a writer to the compose passes streaming
   must mark dirty on a mid-level cluster install.

If the two-texture layout is ever wanted, the remaining order is lightmap
array-consolidation → two-texture shadowmask. Neither of the one-binding layouts
enters that chain. Mask-capacity work remains gated on measured overlap, not on
the parent epic's authored-hints slice.

## Assignment simplifies under abundant slots

Today's assignment (`shadowmask_bake/assignment.rs`) is two-phase: a bounded exact
search for a 4-colouring under `SHADOWMASK_COLOR_SEARCH_NODE_BUDGET`, falling back on
budget exhaustion to a priority greedy that can **drop a mask on search-budget
grounds** rather than on the device ceiling. That machinery exists because four
colours is scarce — and at four colours it is still the right machinery, which is why
`shadowmask-atlas-compress-at-rest` leaves it alone.

Under groups grown on demand, colours are abundant. A deterministic greedy first-fit —
stable selection order, lowest free slot, at most Δ+1 slots — is complete and drops
only at the array-layer ceiling. Retiring the search makes no-drop-below-budget true
by construction, collapses the two four-hardcoded loops to one, and removes a
search-budget-dependent nondeterminism source. Cost: greedy may use a few more groups
than an optimal colouring right at the ceiling. Accepted; the drop there is graceful.

`lighting-scale--shadowmask-cold-working-set` (landed) restructured this seam — it
deletes the per-(light, texel) membership record and derives the overlap graph
analytically, so adjacency arrives on a cheaper footing. Re-anchor against it before
building. One foreclosure it already created: that deleted record is the only
structure where per-texel visibility values and cross-light adjacency coexist.
Intensity-ordered retention reads light parameters only and is unaffected, but a
contribution- or coverage-weighted retention priority would have to re-materialize
that term.

## The shader cost — capacity's real price

Both decode paths sample the atlas **once per fragment**, hoisted out of their light
loops, because every light shares one texel's four channels; each light then selects
its channel by index. The world-specular path carries the comment "undo this if
specular gains per-light UVs"; the promoted-union path hoists because every promoted
light shares the fragment's lightmap UV and layer.

At a group count **fixed at two** the hoist survives — both layers are known from
`lightmap_layer` alone, which is what makes the compress-at-rest brief cheap. Once
the group count **grows**, each light's mask sits on its own array layer and the
sample moves inside both loops. Both contain `continue` and `break`, which is
non-uniform control flow, where WGSL forbids the implicit-derivative sampling
function. The explicit-level form is required; the atlas carries one mip level, so no
sampled value changes.

The real cost is **one sample per fragment becoming one per contributing static
light**, behind the early-outs already gating those loops. This is intrinsic to
capacity and survives into every layout in the brief's table. It is the feature's
price rather than a codec artifact, so it belongs in acceptance and not only here.

## The slot index and its sentinel

The slot crosses into the runtime via the `SpecLight` shadowmask field and the
promoted record's metadata as a float, with the sentinel preserved. The shader's
dropped-sentinel comparison constant is presently 4.0, one past the last RGBA channel;
it must move above every representable slot.

On the wire, the current 8-bit table element is safe only because slots occupy `0..3`
and the sentinel is `0xFF`. A group-addressed slot reaches the full 8-bit range —
at `layer_count = 1` the ceiling is 256 groups — so the element must widen and the
sentinel move outside the representable slot range. This is a genuine collision, not a
theoretical one, and it is the reason the compress-at-rest brief's fixed group count
of two needs no widening at all.

## Prior commitments preserved

- `rendering_pipeline.md` §4: absent, rejected, or dropped shadowmask data is fully
  lit, independent of pool-shadow promotion and its crossfade. Preserved, extended to
  an over-budget layer-group product.
- Static→static world shadowing stays exactly zero via the pool-shadow
  union-subtraction dead-zone. Group addressing changes mask *location*, not the union
  term.
- `static-light-shadowmask-cache-addendum` ships a live byte-for-byte guarantee for
  this section. The in-tree BC paths are pure functions, so the BC6H lossy exemption
  in `build_pipeline.md` §Build Cache stays available and untaken.
- `static-light-shadowmask-world-receipt` banked the lightmap array-consolidation as
  the fallback for a feature needing array-layer headroom. Only the two-texture layout
  triggers it.
