# Specular Chunk-List Continuity

## Goal

Kill a runtime forward-specular artifact on world geometry: a continuous wall or floor loses a
light's highlight along a straight, grid-aligned line on dense maps. The per-chunk light-list bake
evicts overflow lights by slot index, so two adjacent chunks overlapped by the same set of lights
keep *different* subsets, and the boundary between them cuts the highlight mid-surface. Replace the
slot-index eviction with influence-ranked eviction so neighboring chunks keep the same brightest
lights, and raise the per-chunk cap so the threshold bites far less often on the dense maps that
drive the artifact.

## Scope

### In scope

- The `ChunkLightList` bake (`bake_chunk_light_list`, `chunk_light_list_bake.rs`): influence-ranked
  overflow eviction replacing the current slot-index-biased truncation, plus a raised
  `DEFAULT_PER_CHUNK_CAP` (`chunk_light_list.rs`).
- Ranking bake-side by the same influence quantity the runtime already ranks by
  (`sdf_select_influence`): the light's model-aware falloff contribution times its peak intensity,
  evaluated at the chunk's closest-approach point to the light.
- Bake-level tests delivering the acceptance criteria: eviction ordering (AC 1), high-slot/low-slot
  inversion (AC 2), neighbor consistency across a shared chunk face (AC 3), the contains-guard (AC 4,
  both the dim-contained and `>cap`-contained cases), and the `per_chunk_cap` default (AC 5).

### Out of scope

- **The `ChunkLightList` CSR wire format.** `ChunkLightListSection` (`chunk_light_list.rs`) already
  stores variable per-chunk counts via `offsets` + flat `light_indices`; raising the cap changes no
  on-disk layout, only the informational `per_chunk_cap` header field's value and the emitted index
  volume. No new section, no field reorder.
- **The runtime SDF per-light diffuse path.** `select_sdf_lights` already re-ranks the chunk window
  by influence per fragment (`sdf_light_select.wgsl`), so it is cap-tolerant and unaffected by the
  eviction change beyond seeing a larger, better-ordered candidate window. This spec does not touch
  its selection logic.
- **A shader cross-chunk blend.** Influence ranking removes the boundary at its source; a blend
  would treat the symptom and is not built.

## Direction

**Problem.** `bake_chunk_light_list` (`chunk_light_list_bake.rs`) clamps each over-cap chunk with
`bucket.truncate(cap)` after stable-partitioning contained lights to the front — a *slot-index-biased*
rule that keeps the lowest-numbered non-contained lights, so two adjacent chunks overlapped by the
same >`cap` lights keep *different* sets and a continuous surface loses a light's highlight exactly
at the chunk face.

**Prior commitments.** The runtime already ranks this exact buffer by influence, and this bake
mirrors that ranking rather than inventing a new one. `sdf_select_influence`
(`sdf_light_select.wgsl`) scores each chunk-window light as `atten * peak`, where `atten` is the
model-aware falloff `light_eval_falloff(dist, range, model)` (or `1.0` when `range == 0`) and
`peak = max(color.x, color.y, color.z)` over the premultiplied (color × intensity) color;
`select_sdf_lights` keeps the top-K by "influence descending, tie-break light index ascending." The
bake scores the same quantity — the light's own falloff-model contribution times its peak intensity —
at the chunk's closest-approach point to the light. It computes the falloff through
`lightmap_bake::falloff`, the compiler-side model-aware curve — the same falloff shape the runtime's
WGSL `light_eval_falloff` evaluates, implemented independently in Rust; ranking by a light's actual
contribution is what keeps the bake's kept set aligned with the set the runtime shades from.

**Alternatives rejected.** *Just raise the cap without ranking.* Raising the cap lowers the
probability a chunk overflows but does not remove the artifact — any map that still overflows keeps
the slot-index bias, and the failure re-appears on the next denser map. Ranking is the correctness
fix; the cap raise only reduces how often the fix is exercised. *Remove the cap entirely.* The
runtime `use_specular` loop iterates every light in the chunk window per fragment (`chunk_count`), so
an unbounded cap converts the correctness bug into a per-fragment perf cliff in a pathological chunk.
A bounded raise plus ranking keeps the loop cost bounded while removing the boundary. *Rank the
eviction but leave the cap at 64 (or raise it less).* Influence ranking alone removes the slot-index
bias, so a rank-only change is the minimal correctness fix and the 64→256 raise is a separable,
deliberate trade. The runtime `use_specular` loop iterates every windowed light per fragment with no
per-fragment top-K (unlike the SDF selection path, which caps at `SDF_SELECT_K = 4`), so on the
densest chunks — the ones that overflow — a higher cap costs proportionally more per-fragment specular
work. The raise buys two things for that cost: the threshold bites on fewer maps, and — because any
residual boundary lands at the *weakest kept* light — moving the cut from the 64th to the 256th-ranked
light shrinks the residual boundary's *magnitude*, not only its frequency. Forward per-fragment light
loops are not the current frame bottleneck (`perf-forward-light-cull` was shelved on that evidence, on
the dynamic loop — adjacent, not identical, to this static specular loop), so the hot-path cost is
affordable headroom today. `256` is chosen on that basis and stays a tunable the owner may lower if a
future dense map profiles the specular loop as hot. *Rank by a distance-only linear window.* A linear `1 - d/range` score is cheaper but misranks against the
runtime for `InverseSquared` lights — the common realistic falloff — where the runtime's per-fragment
weight is `1/d²`; a linear bake score would keep a different top-`cap` set than the runtime prefers,
which is what "keep the lights the runtime shades from" exists to avoid. *An array-atlas / packing /
layer-spill fix* (as used for the lightmap and SH atlases) addresses nothing here: the specular light
data is not a fixed-footprint texture atlas but a variable-length CSR structure (`offsets` + flat
`light_indices`) already sized to its exact data at upload (`ChunkGrid::from_section`,
`render-cpu/src/chunk_list.rs`), bounded only by the 16 MB `MAX_SECTION_PAYLOAD_BYTES` hard error —
there is no footprint pressure to relieve.

## Acceptance criteria

- [ ] On a synthetic over-cap chunk (candidate set larger than the cap, with lights of varied
      intensity and distance), the retained `light_indices` are exactly the top-`cap` by the bake
      influence metric (contained lights retained first), verified against an independently computed
      reference ordering — **not** the lowest-`cap` by slot index.
- [ ] A bright light placed at a *high* slot index is retained in an over-cap chunk while a dim
      light at a *low* slot index is evicted — the direct inversion of today's slot-biased behavior.
- [ ] Two adjacent chunks whose candidate sets both include the same boundary-spanning light that
      ranks above `cap` at *both* chunks' closest-approach points both retain it after eviction, so no
      light strong at a shared chunk face survives on only one side. A light near the shared face has
      near-equal closest-approach distance in both chunks, hence near-equal influence and rank, so it
      clears the cap on both sides together; the only cross-face disagreement possible is at the
      weakest-kept rank — a light sitting exactly at the cap threshold — the acknowledged residual whose
      magnitude the raised cap shrinks. On a dense/over-cap continuous surface this yields no
      grid-aligned specular cutoff mid-surface at any light bright enough to clear the cap.
- [ ] A contained light — one whose origin lies inside the chunk — is evicted only after every
      non-contained candidate is gone: no non-contained light is ever kept over a contained one, at any
      cap and candidate-set size (the contains-guard invariant is preserved). Below `cap` contained
      lights, none is evicted; when a single chunk holds more than `cap` contained lights, truncation
      keeps the `cap` lowest-slot contained lights deterministically.
- [ ] The `ChunkLightListSection` round-trips unchanged in layout; the baked section's
      `per_chunk_cap` carries the new default when the pipeline bakes with
      `DEFAULT_PER_CHUNK_LIGHT_CAP`; the 16 MB `MAX_SECTION_PAYLOAD_BYTES` hard error still fires on
      an over-budget bake rather than dropping data silently.

## Task: Influence-ranked chunk overflow eviction and raised cap

Replace the slot-index-biased overflow eviction in `bake_chunk_light_list`
(`crates/level-compiler/src/chunk_light_list_bake.rs`) with influence-ranked eviction, and raise
`DEFAULT_PER_CHUNK_CAP` (`crates/level-format/src/chunk_light_list.rs`) from `64` to `256`. The
production pipeline bakes with the `DEFAULT_PER_CHUNK_LIGHT_CAP` re-export of that constant
(`pipeline.rs`, `bake_chunk_light_list_cached(..., DEFAULT_PER_CHUNK_LIGHT_CAP, ...)`), so the raised
default reaches the runtime; explicit-cap test call sites keep their literal caps. The current
`if bucket.len() > cap` block stable-partitions contained slots to the front, then calls
`bucket.truncate(cap)` — keeping the lowest-numbered non-contained slots, which differ between
adjacent chunks. Rewrite that block to rank by a bake influence metric that mirrors the runtime
`sdf_select_influence` (`crates/renderer/src/shaders/sdf_light_select.wgsl`): for each `slot` in the
bucket, resolve its light via `static_slots[slot as usize].1` (`static_slots` is built
`.enumerate().map(|(slot, light)| (slot as u32, light))`, so a slot value equals its index, and
`static_slots` is in scope at the eviction site); compute `d_min` = distance from the light origin to
the closest point of the chunk AABB using the same `center.clamp(chunk_min, chunk_max)` expression
`overlaps_chunk` already uses (`d_min = 0` when the origin is inside the AABB); compute
`range_atten = lightmap_bake::falloff(light, d_min)` when `light.falloff_range > 0.0`, else `1.0`
(mirroring the runtime's `range > 0` gate — directional lights carry no range and take `1.0`, and the
`else` branch must be reached, since `lightmap_bake::falloff` floors `range` to `1e-4` internally and
would otherwise score a rangeless light near zero); compute
`peak = light.intensity * max(color[0], color[1], color[2])`; and take
`influence = range_atten * peak`. Reuse `lightmap_bake::falloff` rather than re-deriving the curve —
make it `pub(crate)` (it is the module-private `fn falloff(light: &MapLight, distance: f32)` in
`crates/level-compiler/src/lightmap_bake.rs`, the model-aware compiler-bake curve; both modules use
the same `crate::map_data::MapLight`, so it accepts the bucket's `&MapLight` directly). Keep the
contains guarantee as a hard first tier: sort the bucket by a key that is a total order over every
slot — contained slots (those already pushed to `contained_slots`) first, ordered among themselves by
`slot` ascending (their `static_slots` insertion order); then non-contained slots by `influence`
descending, tie-broken by `slot` ascending (mirroring the runtime's ascending light-index tiebreak) —
then `bucket.truncate(cap)`. Slots are unique, so the key fully disambiguates every pair and the result
does not depend on sort stability; compare `influence` with a total order (`f32::total_cmp`), not a
partial compare that can leave equal-influence slots unordered. Pinning the contained tier to `slot`
ascending makes truncation deterministic in the one case it cuts a contained light — a chunk holding
more than `cap` contained lights — where the lowest-slot contained lights survive.
This preserves the contains-guard while fixing the slot bias in the non-contained bulk, so
neighboring chunks keep the same brightest lights. Retain the existing `overflow_chunks` /
`overflow_drops` counters, the per-chunk `log::warn!`, and the grid-summary `log::warn!` (their
wording may note that eviction now keeps the highest-influence lights). The existing
`per_chunk_cap_clamps_overflow` test passes an explicit `cap` and still holds (the clamp mechanism is
unchanged); add tests per the AC list asserting the kept set equals the influence-top-`cap` reference,
that a bright high-slot light survives over a dim low-slot light, that a boundary-spanning bright
light is kept in both of two adjacent over-cap chunks, that a *dim* contained light survives eviction
in an over-cap chunk while brighter non-contained lights are dropped, that a chunk holding more than
`cap` contained lights keeps the `cap` lowest-slot contained lights, and that a section baked with
`DEFAULT_PER_CHUNK_LIGHT_CAP` carries the raised default in its `per_chunk_cap` header field. The
contains-guard test must make the contained light's influence strictly *below* the non-contained
candidates it outlives, or it fails to guard the tier: a contained light has `d_min = 0`, so its
`range_atten` is maximal and its dimness must come from a low `intensity` (or `color`), not distance.
The existing `contained_light_survives_per_chunk_cap_truncation` uses an influence-maximal contained
light (its `intensity`/`color` match the non-contained set and `d_min = 0` maxes its `range_atten`),
so it would pass even a wrong pure-influence sort that drops the contained-first tier — the new test
must use a genuinely dim contained light. Raising the cap needs no runtime GPU change:
`ChunkGrid::from_section` (`crates/render-cpu/src/chunk_list.rs`) sizes the offset and index storage
buffers to the actual data (`sec.offsets.len()`, `sec.light_indices.len()`), and the `use_specular`
loop iterates the data-driven `chunk_count`, so nothing in the shader or upload path hardcodes 64. The
payload backstop is unchanged — a raised cap can only add indices in chunks that genuinely overlap
more than 64 lights, and `MAX_SECTION_PAYLOAD_BYTES` still hard-errors the bake if the total exceeds
16 MB. Bump `CHUNK_LIGHT_LIST_STAGE_VERSION` (currently `1`) so cached bakes produced by the old
slot-biased eviction are invalidated: the cache key hashes the inputs, `cell_size_meters`,
`per_chunk_cap`, and the stage version but not the eviction algorithm, so the production cap change
(64→256) already forces a rebake, but a bake pinned to an unchanged explicit cap would otherwise reuse
a stale slot-biased memo.

## Sequencing

Single task, single phase. No intra-spec file collision.

## Cross-spec coordination

**Falloff-parity dependency (`specular-falloff-parity-tests`).** This spec's ranking correctness — "keep
the lights the runtime shades from" — rides on the bake `lightmap_bake::falloff` and the runtime
`light_eval_falloff` evaluating the same curve. That parity is real in current source but *unpinned by
tests*: the sibling draft `specular-falloff-parity-tests` records exactly that gap (the falloff curve
lives as several hand-maintained copies with no sweep test tying them together). The ranking here is no
stronger than that parity, so the two drafts are coupled: landing the parity tests first bounds this
spec's guarantee. The dependency is one of confidence, not compilation — this spec's own bake-side tests
(AC 1–3) verify the kept set against a reference computed with the *same* bake metric, so they hold
regardless — but the owner should sequence the parity tests alongside or ahead of this work so the
runtime-alignment claim is verified, not assumed.

`compiler-log-hygiene` retargets `info!` logs in `chunk_light_list_bake.rs` to `debug!` while
explicitly leaving the cap-overflow `log::warn!` calls at default verbosity. This task keeps those
same overflow warns (retuning only their wording). The two do not collide — both preserve the overflow
warn at default verbosity — but whichever lands second should keep the other's intent: the warn stays,
at `warn!` level, with its counters intact.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Contains-guard: a contained light is retained ahead of every non-contained light, evicted only when contained lights alone exceed the cap | Task (contained-slots first tier before truncation) | The influence re-rank must keep contained slots ahead of every non-contained slot, never let a dim contained light fall past `cap` while non-contained lights remain | AC 4 |
| Neighbor consistency: adjacent chunks keep the same top-`cap` lights by influence, so no boundary-spanning bright light survives on only one side | Task (influence-ranked, not slot-biased, eviction) | The sort key (influence desc, then `slot` asc; slots unique, so a total order) makes each chunk's ranking deterministic for reproducible bakes. Cross-chunk agreement rides on closest-approach continuity, not the tiebreak — the shared `slot` tiebreak fires only on exact influence ties, and adjacent chunks evaluate influence at different closest-approach points, so disagreement is bounded to the weakest-kept rank | AC 1, AC 2, AC 3 |

## Ordering pins

The overflow-eviction sort is the spec's one load-bearing ordering; its determinism and the neighbor
guarantee ride on it being a complete total order. These pins nail down every case the total order must handle; the Covered-by column names where each
is pinned — a dedicated AC or Task test (O2, O3, O4, O9), or Task/out-of-scope prose documenting it
as deterministic, harmless, or degenerate without a separate test (O1, O5, O6, O7, O8).

| id | Scenario | Ordering after sort | Expected outcome | Covered by |
|---|---|---|---|---|
| O1 | Two non-contained lights with bit-identical `influence` (identical lights at mirror positions) | equal `influence`; tie-break `slot` ascending | Fully disambiguated — lower `slot` ranks first; deterministic. Total order holds. | Task tie-break (confirmed) |
| O2 | One chunk holds more than `cap` **contained** lights | `[contained by slot asc][non-contained by influence desc, slot asc]`, `truncate(cap)` | The `cap` lowest-slot contained lights survive, deterministically; the rest are evicted. Contained still never loses to a non-contained. | AC 4; Task test enumeration |
| O3 | Shared boundary light ranking above `cap` at both chunks' closest-approach points | near-equal `d_min` → near-equal `influence` → same rank tier both sides | Kept in both chunks; no one-sided highlight. | AC 3 |
| O4 | Two near-face lights at ranks `cap` / `cap+1`, order flipped across neighbors by differing `d_min` | A keeps L drops M; B keeps M drops L | Acknowledged residual at the weakest-kept rank; AC 3 must not claim otherwise. Magnitude shrinks with the raised cap. | Direction / Rough sketch; AC 3 |
| O5 | `bucket.len() == cap` exactly | `if bucket.len() > cap` is false — sort **skipped**, list emitted in `static_slots` (slot) order | Full set kept (nothing dropped); emitted list order is not influence-sorted. Harmless: both runtime consumers are order-independent. | Task (>cap guard); the emitted list is not a canonical influence order |
| O6 | Zero-`influence` non-contained light in an **over-cap** chunk (Linear light at exactly `falloff_range`; or `intensity == 0`) | sort runs (`bucket.len() > cap`): `influence == 0` ranks last among non-contained, ties among zeros break on `slot` asc | Evicted first; kept only if room remains after the contained and positive-influence lights — never evicts a positive-influence light. Under cap the sort is skipped (O5): nothing is dropped and the emitted order is `static_slots` order, not influence order. Either way the light contributes 0 to the specular sum and is ignored by the SDF top-K. | Task (harmless) |
| O7 | `cap == 0` | `truncate(0)` empties the bucket, contained lights included | Degenerate; not a production path (default is `256`, explicit-cap tests use positive caps). The contains-guard is not promised at `cap == 0`. | Out-of-scope guard, low priority |
| O8 | `influence` is `NaN` | undefined under a partial compare | Not constructible from valid map data (finite origins, intensities, distances). The `f32::total_cmp` requirement (see Task) keeps the sort panic-free and deterministic if ever reached. | Task (total-order key) |
| O9 | Bright light at a **high** slot vs dim light at a **low** slot, over-cap | influence desc orders bright first regardless of slot | Bright kept, dim evicted — the inversion of today's slot bias. | AC 2 |

## Rough sketch

**Why the closest-approach point.** The runtime `sdf_select_influence` evaluates influence at the
*fragment* position, but the bake commits one kept set per whole chunk before any fragment exists.
Evaluating the metric at the chunk-AABB point closest to the light (`center.clamp(chunk_min,
chunk_max)`, already computed in `overlaps_chunk`) gives the *maximum* influence any fragment in the
chunk can receive from that light — the conservative "keep it if it can matter strongly anywhere in
this chunk" choice, and the point at which a light most nearly matches the runtime's per-fragment
ranking. Because that influence varies continuously across a chunk face (the closest-approach distance
changes smoothly), a light sitting exactly at the cap threshold is dropped only where it is the
*weakest* kept light, so any residual boundary lands where the dropped light's own contribution is
minimal — the opposite of the slot-index bias, which could drop an arbitrarily bright light.

**Why model-aware, not linear.** The runtime `sdf_select_influence` scores with the model-aware
`light_eval_falloff`, so a light's rank tracks its `InverseSquared` / `InverseDistance` / `Linear`
contribution, not a distance-only line. The bake ranks by the same shape (via `lightmap_bake::falloff`,
the Rust implementation of that same curve) so the set it commits per chunk is the set the runtime would most
want to shade — the divergence a linear bake score would introduce is largest exactly for the
inverse-square lights that dominate realistic content.

**On the cap value.** `256` is a bounded raise (4× today), not a derived optimum, and it does not need
to be one: the influence ranking is the correctness fix, so the exact cap is non-critical — whatever
is dropped at the threshold is always the weakest-influence light — and the 16 MB
`MAX_SECTION_PAYLOAD_BYTES` hard error is the backstop against a runaway bake. The CSR format imposes
no obstacle to a further raise if a future profiled dense map warrants it.

**Files touched.** `crates/level-format/src/chunk_light_list.rs` (cap constant);
`crates/level-compiler/src/chunk_light_list_bake.rs` (eviction block + tests);
`crates/level-compiler/src/lightmap_bake.rs` (`falloff` visibility to `pub(crate)`).
