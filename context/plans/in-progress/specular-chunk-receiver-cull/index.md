# Specular Chunk Receiver Cull

## Goal

Stop the `ChunkLightList` bake from dropping a static light from a chunk that
holds lit, unoccluded receiver surface. `bake_chunk_light_list` admits a light
into a chunk only if a ray from the light reaches one of nine **volume** proxy
points; a chunk whose only receiver is a thin surface sliver can have all nine
proxies in solid or behind geometry, so the light is dropped and the runtime
forward specular loop has no light to shade that surface's specular map. Sample
the **receiver geometry** inside the chunk instead, so a light is kept whenever
it can shade a real fragment — the invariant the runtime loop already assumes.

## Scope

### In scope

- Replace the volume-proxy visibility test in `bake_chunk_light_list`
  (`crates/level-compiler/src/chunk_light_list_bake.rs`) with a receiver-sampling
  test built from the chunk's own geometry: triangles clipped to the chunk AABB,
  their vertices and centroids offset along the surface normal.
- A triangle-to-chunk binning prepass feeding that test.
- Bump `CHUNK_LIGHT_LIST_STAGE_VERSION` so cached bakes from the volume-proxy
  cull are invalidated.
- Synthetic bake unit tests pinning the now-permitted case, the still-refused
  case, and cross-boundary continuity.
- An independent, `#[ignore]`-gated regression oracle over
  `content/dev/maps/campaign-test` asserting zero false-negative `(light, chunk)`
  pairs — the acceptance signal the fix drives to zero.

### Out of scope

- **The influence-ranked overflow eviction and the `256` cap**
  (`context/plans/done/specular-continuity`). That code runs only inside
  `if bucket.len() > cap`, downstream of the candidate-admission stage this spec
  changes; it is preserved verbatim. See AC 5.
- **The `ChunkLightListSection` wire format** (`chunk_light_list.rs`). No section,
  header, or field changes; the emitted index volume may grow as more lights are
  correctly kept. See AC 6.
- **Lightmap-texel receiver sampling.** Texel world positions are not available
  at this bake stage — see Alternatives rejected.
- **The runtime forward specular loop and the chunk lookup** (`forward.wgsl`,
  `ChunkGrid::from_section`). The runtime reads the same section shape; only its
  contents change. No shader edit.
- **A dev-tools chunk-grid overlay.** A separable presentation aid the diagnosis
  proposes; tracked as a follow-up, not built here. See Open questions.
- **Denser volume proxy sampling** (a 3×3×3 lattice). It shrinks the failing set
  but keeps the class — any volume proxy misses a receiver sliver thinner than
  its spacing. See Alternatives rejected.

## Direction

**Problem.** `any_ray_unoccluded` (`chunk_light_list_bake.rs`) answers "could any
receiver in this chunk see the light" by tracing to nine points on the chunk AABB
(`sample_points`: center + eight inset corners, clipped to the light-influence and
world boxes). Those are volume samples, not receiver samples. A chunk holding a
ceiling whose height sits within the 0.5 m corner inset of the chunk floor has all
nine proxies inside the solid brush above the ceiling; every ray from a light in
the room below hits the ceiling triangles first, all nine fail `segment_clear`,
and the light is dropped from a chunk it plainly lights. The runtime `use_specular`
loop then iterates only that chunk's `chunk_indices` and finds no light to feed
`blinn_phong`, so the specular map's highlights vanish along the chunk-grid plane.
Confirmed on campaign-test: six false-negative `(light, chunk)` pairs, each with
all nine bake proxies occluded while a receiver vertex in the chunk is provably
unoccluded (`context/research/specular-texture-cutoff.md`).

**Prior commitments.** This fix sits in the same function as
`specular-continuity`, which replaced slot-index overflow eviction with
influence-ranked eviction and raised the cap to 256. That change lives inside the
`if bucket.len() > cap` block; this change replaces the per-candidate
`any_ray_unoccluded` admission test that runs *before* any light reaches that
block. The two do not overlap: campaign-test's busiest chunk holds six lights
against the 256 cap, so eviction never fires here — this is the reachability cull,
not the overflow eviction (diagnosis, "Why it is not the specular-continuity
mechanism"). The eviction stays exactly as `specular-continuity` left it; this
spec only widens the candidate set that reaches it. The bake's stated contract —
"a false KEEP costs one redundant runtime candidate; a false DROP is a cell-sized
hole… Err lit" (`any_ray_unoccluded` doc comment) — is the contract this fix
finally honors; the volume proxies violated it.

**Alternatives rejected.**

- *Lightmap-texel receiver centers instead of clipped triangles.* Texel centers
  would align the kept set with the exact fragments the runtime shades. But the
  texel world positions are produced by the lightmap bake
  (`chart_texel_world_position`, `ChartPlacement` in `chart_raster`), consumed
  inside `lightmap_bake`, and **not threaded into `ChunkLightListInputs`** — that
  struct carries only `bvh`, `primitives`, `geometry`, `lights`, `tree`,
  `portals`, `exterior_leaves`. Using texel centers means a new input across two
  bake stages, a new cache-key dependency, and one ray per 4 cm texel versus one
  per triangle vertex — a far larger bake cost for a sub-triangle alignment gain
  that the bake's err-lit conservatism does not need. Clipped triangles are the
  geometry already in hand: `GeometryResult` carries the world-space triangles and
  per-vertex normals, and `segment_clear` already iterates them. Decision:
  clipped triangle points.
- *Conservative variant — drop `any_ray_unoccluded` entirely, keep on
  `overlaps_chunk` + portal flood.* This removes every false negative but
  re-admits every genuinely-occluded light: the diagnosis found that **most**
  mixed-membership drops are correct (`absent-vertex-unoccluded false` — the
  omitted side is behind a wall and must stay dropped). Dropping the occlusion
  test moves that filtering to the per-fragment `use_specular` loop, which then
  rejects those lights only by range and `NdotL`, paying per-fragment specular
  cost on dense maps for lights that never contribute. Receiver sampling keeps the
  occlusion filter — a receiver point behind a wall still fails `segment_clear` —
  so it fixes the false negatives *without* re-admitting the correct drops. That
  is the whole reason to sample receivers rather than just delete the test.
- *Denser volume proxies (3×3×3 lattice, mid-plane samples).* Shrinks the failing
  cases but keeps the class: any volume-sampled proxy misses a receiver sliver
  thinner than its spacing. Not a fix, a mitigation.

## Acceptance criteria

- [ ] **Permit — receiver kept.** A synthetic bake reproducing the diagnosed
      false negative — a chunk holding a receiver surface (e.g. a ceiling whose
      height lies within the corner inset of the chunk, or a floor sliver) lit by
      an in-range point light in the open space the surface faces, positioned so
      all nine former `sample_points` land in solid or behind that surface —
      retains the light in that chunk's baked `light_indices`. A companion
      assertion confirms the pre-change volume-proxy cull dropped it (the nine
      proxy segments are all occluded).
- [ ] **Refuse — occluded receiver dropped.** A synthetic bake where a chunk is
      overlapped by an in-range light (its range sphere intersects the chunk AABB)
      but every receiver surface inside the chunk is behind a solid occluder from
      that light (e.g. a wall between the light's room and the receiver) still
      omits the light from that chunk's `light_indices`. Genuinely-occluded drops
      are preserved.
- [ ] **Cross-boundary continuity.** A single receiver triangle spanning a
      chunk-grid plane, lit and unoccluded on both sides, retains the light in
      **both** neighboring chunks' `light_indices`, so no reachability-stage cut
      falls mid-surface at the grid plane.
- [ ] **Empty-air chunk drops the light.** A chunk overlapped by an in-range
      light but containing no receiver triangle omits the light — no receiver, no
      fragment to shade, correctly no candidate.
- [ ] **Eviction preserved.** This spec edits no code inside the
      `if bucket.len() > cap` block — a review gate, not a runnable assertion. The
      existing eviction tests in `chunk_light_list_bake.rs` (the `overflow_*`,
      `contained_*`, `boundary_spanning_*`, and `per_chunk_cap_clamps_overflow`
      cases `specular-continuity` added) stay green unchanged: an over-cap
      synthetic chunk still keeps the top-`cap` lights by influence with contained
      lights first. This spec only widens the candidate set that reaches eviction.
- [ ] **Section and cache.** The `ChunkLightListSection` on-disk layout,
      `has_grid`/placeholder behavior, and the `MAX_SECTION_PAYLOAD_BYTES` hard
      error are unchanged — a review gate: the on-disk format constant
      `CHUNK_LIGHT_LIST_VERSION` (1) and `chunk_light_list.rs` are not edited.
      Separately and runnably, the bake cache epoch
      `CHUNK_LIGHT_LIST_STAGE_VERSION` is bumped (2→3) so a cache entry produced
      by the volume-proxy cull is a miss, not a stale hit, at an unchanged
      explicit cap — the two constants are distinct.
- [ ] **Campaign-test oracle at zero.** An `#[ignore]`-gated regression test bakes
      the chunk light list for `content/dev/maps/campaign-test` and asserts
      **zero** false-negative `(light, chunk)` pairs under an independent detector:
      a static light that `overlaps_chunk` a chunk, is omitted from that chunk's
      baked list, yet has a triangle vertex inside the chunk that is within
      `falloff_range`, whose triangle faces the light, and that a brute-force
      all-triangle segment test proves unoccluded. The diagnosis measured six such
      pairs on the volume-proxy bake; the fix drives them to zero.
- [ ] **Binning linearization.** The triangle-to-chunk binning prepass indexes
      each chunk by the same linear formula the per-cell loop reads
      (`z * nx * ny + y * nx + x`). A binning test on an asymmetric grid
      (`nx != nz`) with a lone receiver triangle keeps the light in the cell
      holding the triangle and drops it in the transposed-index cell, so a
      transposed binning formula fails — a case the symmetric default fixture
      cannot catch.

## Tasks

### Task 1: Receiver-sampling admission cull

Replace the volume-proxy visibility test in `bake_chunk_light_list`
(`crates/level-compiler/src/chunk_light_list_bake.rs`) with a receiver-sampling
test. Before the `for z/y/x` chunk loop, build a triangle-to-chunk binning
prepass: for each triangle in the geometry (walk the flat `inputs.geometry.geometry.indices`
in threes, or use `inputs.geometry.face_index_ranges` to bound each face's span into
that index list), compute its world-space
AABB, map that AABB to the range of chunks it overlaps using the already-computed
`world_min`, `cell`, and `dims`, and record the triangle in each overlapped
chunk's list (store triangle start-indices or vertex-index triples; a
`Vec<Vec<...>>` of length `chunk_count`). Bin at the identical linear index the
per-cell loop reads — `chunk_idx = z * nx * ny + y * nx + x` — since
`any_receiver_unoccluded` fetches a chunk's bin by that index; a transposed
binning formula reads the wrong bin and false-drops the light. The default
single-quad fixture is a symmetric `3×1×3` grid with floor in every cell, so it
cannot fail a transpose; add a binning-linearization test on an asymmetric grid
(`nx != nz`) with a receiver triangle in a single cell whose transposed index
lands on an empty cell, so a wrong-bin read produces no receiver point and the
assertion fails (AC 8). Inside the per-candidate loop, replace
the `if !any_ray_unoccluded(...) { continue; }` guard with
`if !any_receiver_unoccluded(...) { continue; }`, a new function that, for the
chunk's binned triangles: clips each triangle to the chunk AABB into a convex
polygon (reuse `crate::geometry_utils::clip_winding_to_half_spaces`, the
Sutherland–Hodgman convex clipper `portals.rs` already uses, against the six
axis-aligned chunk planes, rather than reimplementing), skips empty results,
takes that polygon's vertices plus its centroid as receiver points, offsets each
point off the receiving plane along the triangle's geometric normal (cross
product of two edges) toward the light — for point/spot lights the sign of
`normal · (light_origin − point)`, for directional lights toward `−aim`; when
that dot is within `RAY_EPSILON` of zero (a light grazing the receiver plane),
skip the offset and test the point on-plane rather than risk pushing it into
solid and reading the surface as its own occluder — by a
small constant `RECEIVER_NORMAL_OFFSET_METERS` (a named constant; constraint: at
least `RAY_EPSILON`, well under `cell`, enough to lift the point clear of its own
receiving plane so `segment_clear`'s `SAMPLE_END_TOLERANCE_METERS` end-graze
allowance is not consumed by the surface itself), then calls the existing
`segment_clear(bvh, primitives, geometry, light, point)` and returns `true` on the
first clear segment. Return `false` when the chunk has no binned triangle or every
receiver point's segment is blocked. Do **not** gate the receiver test on
front-facing: the bake must err lit (its own doc contract), and a mesh-backfacing
fragment can face the light under normal mapping, which the per-fragment
`use_specular` loop resolves exactly via `N_bump·L`; adding a mesh-normal facing
gate here would risk a false drop. Remove `any_ray_unoccluded`; drop `sample_points` from the production
path, but retain it as a `#[cfg(test)]` helper (or reconstruct its nine
points in that test) since the AC 1 companion replay still traces the
nine former proxy segments. Keep `segment_clear`, `ray_triangle_hit`,
`SAMPLE_END_TOLERANCE_METERS`, and `RAY_EPSILON` unchanged. Leave `overlaps_chunk`,
the contained-light guard, the `light_reachable` portal flood, and the entire
`if bucket.len() > cap` eviction block untouched — this task changes only the
admission test, not eviction (the eviction rows referenced by AC 5 must stay
exactly as `specular-continuity` left them). Bump `CHUNK_LIGHT_LIST_STAGE_VERSION`
(currently `2`) so a cache entry keyed on an unchanged explicit cap but produced by
the old cull is invalidated (the cache key hashes inputs, `cell_size_meters`,
`per_chunk_cap`, and the stage version, not the admission algorithm). Add synthetic
`#[cfg(test)]` unit tests delivering AC 1 (permit; include the companion assertion
that a direct replay of the nine former proxy segments is all-occluded so the test
documents the exact condition the fix repairs), AC 2 (refuse; a two-room wall
between light and receiver, extending the existing `two_room_geometry` shape), AC 3
(a receiver triangle spanning a chunk-grid plane kept in both chunks), and AC 4
(empty-air chunk). Keep the existing eviction tests green unchanged (AC 5). Note
the bake-time cost: the prepass is `O(triangles + Σ per-chunk triangle counts)` and
the admission test now traces one segment per clipped receiver point rather than up
to nine fixed proxies; this is bake-only, no runtime cost.

### Task 2: Independent campaign-test regression oracle

Add an `#[ignore]`-gated regression test asserting the fix eliminates the
diagnosed false negatives on `content/dev/maps/campaign-test`, using a detector
whose occlusion test is code-path-independent from the production cull. Load the
map through the existing `fixture_pipeline::load_fixture` helper
(`crates/level-compiler/src/fixture_pipeline.rs`), which runs
parse → partition → visibility → geometry → BVH and returns
`FixturePipeline { geometry, bvh, primitives, tree, faces, exterior_leaves,
lights }`; extend that helper (or regenerate inside the test via
`portals::generate_portals(&tree)`) to also supply the portal list the bake's
`light_reachable` flood needs, since `FixturePipeline` does not currently expose
portals. Wrap `FixturePipeline`'s `lights` (a `Vec<MapLight>`) with
`AlphaLightsNs::from_lights` — the same bridge the in-module bake tests use — to
supply the `&AlphaLightsNs` that `ChunkLightListInputs.lights` expects, build
`ChunkLightListInputs` from the pipeline products, and call
`bake_chunk_light_list` with `DEFAULT_CELL_SIZE_METERS` and
`DEFAULT_PER_CHUNK_LIGHT_CAP`. Then enumerate the
compacted static lights in the same order the bake emits indices — call the
module-private `compacted_static_lights` (the test is in-module) so slot `i` maps
to the same light the section's `light_indices` reference. For every
`(static light slot, chunk)` pair: if the light `overlaps_chunk` the chunk AABB
(reuse the production predicate — the candidate gate is shared, only the occlusion
test must be independent) and the chunk's baked `light_indices` omit the slot, scan
the triangles whose vertices fall inside the chunk AABB and flag the pair as a
false negative if any such vertex is within `light.falloff_range` of the light
origin, its triangle's geometric normal faces the light, and a **brute-force
all-triangle** segment test (iterate every triangle in `geometry`, not the BVH,
so a BVH-traversal bug cannot mask a false negative) finds the light-to-vertex
segment clear. That brute-force test must not count the vertex's own receiving
triangle at the segment endpoint as the occluder: apply the same
`SAMPLE_END_TOLERANCE_METERS` end-graze allowance `segment_clear` uses — stop
counting hits short of the vertex — or cast to a point lifted off the surface
along the normal as the cull does. Without it every facing vertex reads blocked
and the oracle is vacuously zero regardless of the fix. Assert the flagged count is zero (AC 7). Document in a regression
comment that the diagnosis measured six such pairs on the pre-fix volume-proxy
bake, and that this oracle covers the vertex-in-chunk class the diagnosis measured
— a receiver triangle that passes through a chunk with no vertex inside it is
outside this oracle's probe but inside the Task 1 cull's clipping coverage, so a
zero count here proves the diagnosed pairs are gone, not that every conceivable
sub-triangle receiver is covered. Directional lights (`falloff_range == 0`) fall
outside the range gate, so the detector is a no-op for them; campaign-test's
diagnosed pairs are point/spot. Gate the test `#[ignore]` (heavy map, on-demand
only, per `context/lib/testing_guide.md` §3); it needs only the
geometry/BVH/chunk-bake stages, not the SH or lightmap bakes, so it is far cheaper
than the SH determinism gates.

## Sequencing

**Phase 1 (sequential):** Task 1 — the cull and its synthetic unit tests. Blocks
Task 2: the oracle asserts Task 1's outcome and edits the same file's test module.

**Phase 2 (sequential):** Task 2 — the campaign-test oracle. Consumes the landed
cull; asserts zero false negatives against it.

## Rough sketch

**Receiver-sampling rule, both sides pinned.**

- *Permitted (was wrongly dropped; AC 1).* A chunk row holds a ceiling at height
  `y_c` with `y_c − chunk_floor < 0.5 m`; a light sits in the room below. The nine
  volume proxies all land at or above `chunk_floor + 0.5 m`, inside the solid brush
  above the ceiling, so every proxy segment hits the ceiling first and the light is
  dropped. Receiver sampling clips the ceiling triangle to the chunk, offsets a
  receiver point just below the ceiling toward the light, and finds a clear segment
  — the light is kept.
- *Refused (correctly dropped; AC 2).* A chunk is overlapped by the light's range
  sphere, but a solid wall stands between the light and every receiver surface in
  the chunk. Each clipped receiver point's segment to the light is blocked by the
  wall, exactly as under the volume proxies, so the light stays dropped. This is
  the `absent-vertex-unoccluded false` class the diagnosis found is the majority of
  mixed-membership drops; it must not regress into a keep.
- *Empty air (AC 4).* No triangle bins into the chunk, so there is no receiver
  point; the function returns `false` and the light is dropped. Correct — the
  runtime shades no fragment in an air chunk.

The rule is the same question `any_ray_unoccluded` asked ("can any receiver in this
chunk see the light"), evaluated on the receivers that actually exist instead of on
AABB proxies. It preserves the err-lit bias: any single clear receiver segment
keeps the light.

**Why the oracle is a real signal, not a tautology.** The Task 1 cull keeps a light
when a clipped receiver point (polygon vertex, edge-clip point, or centroid, no
facing gate) is unoccluded via `segment_clear`'s BVH path. The Task 2 oracle flags
a false negative when a triangle *vertex* inside the chunk is facing and unoccluded
via a *brute-force* segment test. A vertex inside the chunk is one of the cull's
clipped receiver points, so any vertex the oracle flags is a point the cull's
receiver test reaches — *provided* the chunk first cleared the untouched
portal-reachability filter and contained-light guard, which run ahead of that
test and can drop a light before any receiver segment is traced. The zero is
grounded empirically, not by construction: the diagnosis's detector shares this
oracle's definition (overlaps, omitted, facing, brute-force-clear) and found all
six pre-fix false negatives on campaign-test were receiver-cull drops with every
bake proxy occluded, not portal drops, so the fixed cull keeps exactly those and
the oracle reads zero on this map. The
independence that matters is the occlusion code path (brute-force all-triangle vs
BVH-traversal) and the receiver enumeration (raw vertices vs clipped polygon), so a
bug in the production path surfaces as a nonzero oracle rather than hiding behind
its own predicate.

**Files touched.** `crates/level-compiler/src/chunk_light_list_bake.rs` (admission
cull, binning prepass, new `any_receiver_unoccluded`, `RECEIVER_NORMAL_OFFSET_METERS`,
stage-version bump, tests); `crates/level-compiler/src/fixture_pipeline.rs` (expose
portals to the oracle, if not regenerated in-test).

## Invariants

`/orchestrate` hands this table to every task agent with the Goal and AC list.

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Err lit: any receiver in a chunk with a clear segment to an in-range light keeps that light in the chunk | Task 1 (`any_receiver_unoccluded` returns true on first clear receiver segment) | A facing gate or a too-coarse receiver set could false-drop a lit fragment; no facing gate, and clipping covers spanning triangles | AC 1, AC 3, AC 7 |
| Occlusion drops preserved: a light whose every in-chunk receiver is behind a solid occluder stays dropped | Task 1 (receiver points reuse `segment_clear`, which still counts intervening triangles) | Deleting the occlusion test (the conservative variant) would re-admit these; receiver sampling keeps `segment_clear` | AC 2 |
| Influence-ranked overflow eviction (`specular-continuity`) unchanged | `specular-continuity`; this spec touches only the pre-eviction admission test | The `if bucket.len() > cap` block, contained-guard, and cap value must not be edited by Task 1 | AC 5 |
| Section layout and payload backstop unchanged | `chunk_light_list.rs` (untouched) | A wider candidate set grows the index volume but not the format; `MAX_SECTION_PAYLOAD_BYTES` still hard-errors | AC 6 |
| Stale cull memos invalidated | Task 1 (stage-version bump) | The cache key hashes inputs + cap + version, not the admission algorithm; an unchanged explicit cap would otherwise reuse a volume-proxy memo | AC 6 |
| Prepass and per-cell loop agree on chunk linear index | Task 1 (bin at `z * nx * ny + y * nx + x`, the loop's own index) | A transposed binning formula reads the wrong bin and false-drops; the symmetric default fixture cannot catch it | AC 8 |

## Open questions

- The dev-tools chunk-grid overlay the diagnosis proposes (draw the grid, make the
  next occurrence a one-frame read) is separable presentation and is left as a
  follow-up. If the owner wants the overlay bundled, it is additive and does not
  change this spec's cull or oracle.
- The diagnosis's six false-negative pairs sit in cells 177/178, 113/115, 238,
  250–258, 329–331, not the HUD's cell 117 from the original screenshot. Matching
  the exact on-screen surface is a one-frame in-engine check with the overlay
  above, not a source question, and does not gate this fix — the mechanism and its
  campaign-test count are what AC 7 pins.
