# Specular Runtime Continuity

## Goal

Kill two independent artifacts in the runtime forward specular path on world geometry. First,
a continuous wall or floor loses its highlight along a straight, grid-aligned line on dense maps —
the per-chunk light-list bake evicts overflow lights by slot index, so adjacent chunks keep
different sets of lights and the boundary between them cuts the highlight mid-surface. Second, a
static light's specular fades on a *different curve* than the same light's baked diffuse and ends
more abruptly, because the shader applies a fixed linear attenuation with a hard range cutoff
regardless of the light's authored falloff model. Both make view-dependent specular read as broken
where diffuse looks correct.

## Scope

### In scope

- The `ChunkLightList` bake (PRL id 23): influence-ranked overflow eviction and a raised per-chunk
  cap, replacing the current slot-index-biased truncation.
- The runtime forward specular loop in `forward.wgsl` (`use_specular` block): evaluate the authored
  falloff model for static-light specular attenuation.
- Plumbing the falloff-model enum into the packed `SpecLight` GPU record without growing it past 64
  bytes.
- Bake-level and reference-parity tests for both.

### Out of scope

- **The `ChunkLightList` CSR wire format.** `ChunkLightListSection` (`chunk_light_list.rs`) already
  stores variable per-chunk counts via `offsets` + flat `light_indices`; raising the cap changes no
  on-disk layout, only the informational `per_chunk_cap` header field's value and the emitted index
  volume. No new section, no field reorder.
- **The runtime SDF per-light diffuse path.** `select_sdf_lights` already re-ranks the chunk window
  by influence per fragment (`sdf_light_select.wgsl`), so it is cap-tolerant and unaffected by the
  eviction change beyond seeing a larger, better-ordered candidate window. This spec does not touch
  its selection logic.
- **The dynamic-tier light loop and its `light_eval_falloff` helper.** Dynamic lights already
  evaluate a per-model falloff (`light_eval.wgsl`); their softened inverse-model window intentionally
  differs from the baked static curve and is not unified here (see Open questions).
- **A shader cross-chunk blend.** Influence ranking removes the boundary at its source; a blend
  would treat the symptom and is not built.
- **`SpecLight.cone_cos.z`.** Claimed by the `specular-shadowmask-occlusion` spec for the shadowmask
  channel. This spec touches only `.w` (see Cross-spec coordination).

## Direction

**Problem.** Two distinct causes, both in the runtime forward specular path. (1) `bake_chunk_light_list`
(`chunk_light_list_bake.rs`) clamps each over-cap chunk with `bucket.truncate(cap)` after
stable-partitioning contained lights to the front — a *slot-index-biased* rule that keeps the
lowest-numbered lights, so two adjacent chunks overlapped by the same >`cap` lights keep *different*
sets and a continuous surface loses a light's highlight exactly at the chunk face. (2) The
`use_specular` loop in `forward.wgsl` computes `atten = max(1 - dist/range, 0)` with a preceding
`if range > 0.0 && dist > range { continue; }`, a fixed linear curve applied to every light
regardless of its `falloff_model`, while the same light's diffuse was baked through
`lightmap_bake.rs::falloff` honoring `Linear` / `InverseDistance` / `InverseSquared`.

**Prior commitments.** The runtime already has an influence metric for this exact buffer:
`sdf_select_influence` (`sdf_light_select.wgsl`) ranks chunk-window lights by
`atten * peak`, where `atten = max(1 - dist/max(range,0.001), 0)` (or `1.0` when `range == 0`) and
`peak = max(color.x, color.y, color.z)` over the premultiplied color, and `select_sdf_lights` keeps
the top-K by "influence DESCENDING, tie-break light index ASCENDING." Task A mirrors this exact
metric bake-side rather than inventing a new one. The runtime already honors per-model falloff for
dynamic lights via `light_eval_falloff` (`light_eval.wgsl`) and encodes the model as
`falloff_model_u32` (`Linear→0`, `InverseDistance→1`, `InverseSquared→2`, `crates/lighting/src/lib.rs`);
Task B stores the same enum index in the static `SpecLight` record. The static baked diffuse curve
this spec matches is `lightmap_bake.rs::falloff`, and Task B diverges from `light_eval_falloff` where
that helper diverges from the bake (argued in Alternatives rejected and Open questions).

**Alternatives rejected.** *Just raise the cap without ranking.* Raising the cap lowers the
probability a chunk overflows but does not remove the artifact — any map that still overflows keeps
the slot-index bias, and the failure re-appears on the next denser map. Ranking is the correctness
fix; the cap raise only reduces how often the fix is exercised. *Remove the cap entirely.* The
`use_specular` loop iterates every light in the chunk window per fragment (`chunk_count`), so an
unbounded cap converts the correctness bug into a per-fragment perf cliff in a pathological chunk.
A bounded raise plus ranking keeps the loop cost bounded while removing the boundary. *An
array-atlas / packing / layer-spill fix* (as used for the lightmap and SH atlases) addresses nothing
here: the specular light data is not a fixed-footprint texture atlas but a variable-length CSR
structure (`offsets` + flat `light_indices`) already sized to its exact data at upload
(`ChunkGrid::from_section`, `render-cpu/src/chunk_list.rs`), bounded only by the 16 MB
`MAX_SECTION_PAYLOAD_BYTES` hard error — there is no footprint pressure to relieve. *Reuse
`light_eval_falloff` for static specular.* It does not match the baked diffuse curve for the inverse
models (it multiplies a linear window onto `1/d` / `1/d²` and has no hard cutoff), so it would leave
the very diffuse/specular mismatch Task B exists to remove; Task B mirrors `lightmap_bake.rs::falloff`
instead.

## Acceptance criteria

- [ ] On a synthetic over-cap chunk (candidate set larger than the cap, with lights of varied
      intensity and distance), the retained `light_indices` are exactly the top-`cap` by the bake
      influence metric (contained lights retained first), verified against an independently computed
      reference ordering — **not** the lowest-`cap` by slot index.
- [ ] A bright light placed at a *high* slot index is retained in an over-cap chunk while a dim
      light at a *low* slot index is evicted — the direct inversion of today's slot-biased behavior.
- [ ] Two adjacent chunks whose candidate sets both include the same bright, boundary-spanning light
      both retain that light after eviction, so no light bright at a shared chunk face survives on
      only one side. On a dense/over-cap continuous surface this yields no grid-aligned specular
      cutoff mid-surface.
- [ ] A light whose origin lies inside a chunk is never evicted from that chunk, at any cap and any
      candidate-set size (the contains-guard invariant is preserved).
- [ ] The `ChunkLightListSection` round-trips unchanged in layout; `per_chunk_cap` carries the new
      default; the 16 MB `MAX_SECTION_PAYLOAD_BYTES` hard error still fires on an over-budget bake
      rather than dropping data silently.
- [ ] The packed `SpecLight` record stays exactly 64 bytes (`spec_light_size_is_64` holds) and
      carries the authored falloff-model enum index in `cone_cos.w`; a pack test asserts bytes 60..64
      decode to `0`/`1`/`2` for a `Linear`/`InverseDistance`/`InverseSquared` light respectively.
- [ ] For each of the three falloff models, the shader's static-specular distance attenuation equals
      the baked diffuse curve `lightmap_bake.rs::falloff` across a distance sweep from 0 to beyond
      `range` (verified by a host-side reference mirroring the WGSL branch, within float tolerance):
      `Linear` reaches 0 continuously at `range`; `InverseDistance`/`InverseSquared` follow `1/d` /
      `1/d²` inside `range` and cut to 0 beyond it, matching diffuse's own cutoff distance.
- [ ] On a scene with an `InverseSquared` static light, the specular highlight no longer extends
      past the surface region where the same light's baked diffuse has fallen off — the two fade
      together rather than specular over-reaching on a linear curve.

## Tasks

### Task A: Influence-ranked chunk overflow eviction and raised cap

Replace the slot-index-biased overflow eviction in `bake_chunk_light_list`
(`crates/level-compiler/src/chunk_light_list_bake.rs`) with influence-ranked eviction, and raise
`DEFAULT_PER_CHUNK_CAP` (`crates/level-format/src/chunk_light_list.rs`) from `64` to `256`. The
current `if bucket.len() > cap` block stable-partitions `contained_slots` to the front, then calls
`bucket.truncate(cap)` — keeping the lowest-numbered non-contained slots, which differ between
adjacent chunks. Rewrite that block to rank by a bake influence metric that mirrors the runtime
`sdf_select_influence` (`crates/renderer/src/shaders/sdf_light_select.wgsl`): for each slot in the
bucket, resolve its light via `static_slots[slot as usize].1` (the enumerate index equals the
compacted slot, already in scope), compute `d_min` = distance from the light origin to the closest
point of the chunk AABB using the same `center.clamp(chunk_min, chunk_max)` expression `overlaps_chunk`
already uses (`d_min = 0` when the origin is inside the AABB); compute
`range_atten = max(1 - d_min/max(range, 0.001), 0)` for `range > 0` else `1.0`; compute
`peak = light.intensity * max(color[0], color[1], color[2])`; and take `influence = range_atten * peak`.
Directional lights (`LightType::Directional`, no range) use `range_atten = 1.0`. Keep the contains
guarantee as a hard first tier: sort the bucket so contained slots (those already pushed to
`contained_slots`) come first, then non-contained slots by `influence` descending, tie-broken by
`slot` ascending (mirroring the runtime's ascending light-index tiebreak), then `bucket.truncate(cap)`.
This preserves the contains-guard while fixing the slot bias in the non-contained bulk, so neighboring
chunks keep the same brightest lights. Retain the existing `overflow_chunks`/`overflow_drops` counters
and the `log::warn!` (its wording may note that eviction now keeps the highest-influence lights). The
existing `per_chunk_cap_clamps_overflow` test passes an explicit `cap` and still holds (the clamp
mechanism is unchanged); add tests per the AC list asserting the kept set equals the influence-top-`cap`
reference, that a bright high-slot light survives over a dim low-slot light, and that a
boundary-spanning bright light is kept in both of two adjacent over-cap chunks. Raising the cap needs
no runtime GPU change: `ChunkGrid::from_section` (`crates/render-cpu/src/chunk_list.rs`) sizes the
offset and index storage buffers to the actual data, and the `use_specular` loop iterates the
data-driven `chunk_count`, so nothing in the shader or upload path hardcodes 64. The payload backstop
is unchanged — a raised cap can only add indices in chunks that genuinely overlap more than 64 lights,
and `MAX_SECTION_PAYLOAD_BYTES` still hard-errors the bake if the total exceeds 16 MB.

### Task B: Evaluate the authored falloff model for static-light specular

Plumb each static light's falloff model into the packed `SpecLight` record and make the forward
specular loop evaluate it, so a static light's specular fades on the same curve as its baked diffuse.
In `crates/lighting/src/spec_buffer.rs`, `pack_spec_lights` receives `&[MapLight]` and each
`postretro_level_loader::MapLight` already carries a `falloff_model` field (used in the module's own
`sample()` test fixture), so no new parameter or plumbing is needed to reach the data. The record's
final slot `cone_cos` (bytes 48..64) uses `.x` = `cos_inner` (48..52) and `.y` = `cos_outer` (52..56);
bytes 56..64 are two `0.0f32` pad writes today. Change **only the second** pad write (bytes 60..64,
`cone_cos.w`) to the falloff-model enum index as a plain float — `0.0` `Linear`, `1.0`
`InverseDistance`, `2.0` `InverseSquared` — matching the discriminant order of
`falloff_model_u32` in `crates/lighting/src/lib.rs`. Leave the first pad write (bytes 56..60,
`cone_cos.z`) as `0.0f32`; it is claimed by the `specular-shadowmask-occlusion` spec (see Cross-spec
coordination) and must not be repurposed here. The record stays exactly 64 bytes; do not reorder
fields. Update the layout doc comment in `spec_buffer.rs` (the `SPEC_LIGHT_SIZE` block) to document
`56..60` as reserved/shadowmask and `60..64` as the falloff-model index. In
`crates/renderer/src/shaders/forward.wgsl`, extend the `SpecLight` WGSL struct comment
(`cone_cos` at the `@group(2) @binding(2)` declaration) to document `.w` as the falloff-model index,
add a `spec_falloff(distance, range, model)` helper that mirrors `lightmap_bake.rs::falloff` exactly —
`model 0` (Linear): `clamp(1 - distance/range, 0, 1)`; `model 1` (InverseDistance):
`distance > range ? 0 : 1/max(distance, 1e-4)`; `model 2` (InverseSquared):
`distance > range ? 0 : 1/max(distance*distance, 1e-4)`, with `range = max(sl.position_and_range.w, 1e-4)`
— and in the `use_specular` loop replace both the `if range > 0.0 && dist > range { continue; }` guard
and the `atten = select(1.0, max(1.0 - dist/max(range, 0.001), 0.0), range > 0.0)` line with
`let atten = spec_falloff(dist, range, u32(round(sl.cone_cos.w)));`. Do **not** reuse
`light_eval_falloff`: it multiplies a linear window onto `1/d` / `1/d²` and omits the hard cutoff, so
it does not match the baked diffuse curve for the inverse models, which is exactly the mismatch this
task removes. Keep the `NdotL <= 0` early-out, the `cone_attenuation_cos` cone term, and the SDF
`visibility` multiply unchanged; only the distance attenuation changes. Do not clamp the inverse-model
result separately — the baked diffuse is itself unclamped near the light, so matching it (not adding a
new clamp) is what keeps diffuse and specular on one curve. Add a host-side reference test mirroring
the `spec_falloff` branches against `lightmap_bake.rs::falloff` across a distance sweep for each model
(the SDF selection already establishes the "Rust reference mirrors WGSL" pattern), plus the
bytes-60..64 pack test from the AC list.

## Sequencing

**Phase 1 (concurrent):** Task A, Task B — independent, no shared file. Task A edits
`chunk_light_list.rs` (cap constant) and `chunk_light_list_bake.rs` (eviction); Task B edits
`spec_buffer.rs` (pack `cone_cos.w`) and `forward.wgsl` (specular attenuation). Task A does not touch
`forward.wgsl` — the specular loop is already data-driven (`chunk_count`), so raising the cap needs no
shader edit — and Task B does not touch the bake, so the two do not collide within this spec. Their
headless-capture ACs observe different scenes (a dense over-cap surface for A; an inverse-square light
falloff for B) and can be validated independently.

The only cross-file caution is *cross-spec*, not intra-spec: both Task B and
`specular-shadowmask-occlusion` Task 1 edit `pack_spec_lights` and the `forward.wgsl` `SpecLight`
struct comment. See Cross-spec coordination — the two partition `cone_cos` z/w and must not collide.

## Cross-spec coordination

The `specular-shadowmask-occlusion` spec claims `SpecLight.cone_cos.z` (bytes 56..60) for its
shadowmask visibility channel; this spec's Task B claims `SpecLight.cone_cos.w` (bytes 60..64) for the
falloff-model index. The two specs partition the `cone_cos` `.z`/`.w` pad between them and must not
collide: neither reorders the 64-byte `SpecLight` record, neither touches the other's slot, and both
edit the same two sites — `pack_spec_lights` (`spec_buffer.rs`) and the `SpecLight` WGSL struct comment
(`forward.wgsl`). If both are in flight, whichever lands second must confirm the first's slot write is
intact (the first pad write for shadowmask `.z`, the second for falloff `.w`) rather than reverting it,
and must merge — not overwrite — the layout doc comment. This is pinned as an Invariant row below.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Contains-guard: a light whose origin is inside a chunk is never evicted from it | Task A (contained-slots first tier before truncation) | Task A — the influence re-rank must keep contained slots ahead of every non-contained slot, never let a dim contained light fall past `cap` while non-contained lights remain | AC 4 |
| Neighbor consistency: adjacent chunks keep the same top-`cap` lights by influence, so no boundary-spanning bright light survives on only one side | Task A (influence-ranked, not slot-biased, eviction) | Task A — tie-break must be a stable total order (influence desc, slot asc) so co-located candidate sets rank identically | AC 1, AC 2, AC 3 |
| `SpecLight` record is exactly 64 bytes, fields never reordered | Task B (reuse `cone_cos.w` pad in place) | Task B (write only bytes 60..64) | AC 6, `spec_light_size_is_64` (`spec_buffer.rs`) |
| `cone_cos` z/w partition: `.z` (56..60) = shadowmask channel, `.w` (60..64) = falloff model; neither spec writes the other's slot | Task B (this spec writes only `.w`) | Task B and `specular-shadowmask-occlusion` Task 1 — both edit `pack_spec_lights` and the `forward.wgsl` `SpecLight` comment; second to land merges, does not overwrite | AC 6; cross-spec confirmation at second landing |
| Static specular distance attenuation equals the baked diffuse curve for all three falloff models | Task B (`spec_falloff` mirrors `lightmap_bake.rs::falloff`) | Task B — must not substitute `light_eval_falloff` (wrong curve for inverse models) and must not add an inverse-model clamp diffuse lacks | AC 7, AC 8 |

## Rough sketch

**Task A influence metric — why the closest-approach point.** The runtime `sdf_select_influence`
evaluates influence at the *fragment* position, but the bake commits one kept set per whole chunk
before any fragment exists. Evaluating the metric at the chunk-AABB point closest to the light
(`center.clamp(chunk_min, chunk_max)`, already computed in `overlaps_chunk`) gives the *maximum*
influence any fragment in the chunk can receive from that light — the conservative "keep it if it can
matter strongly anywhere in this chunk" choice, and the point at which a light most nearly matches the
runtime's per-fragment ranking. Because that influence varies continuously across a chunk face (the
closest-approach distance changes smoothly), a light sitting exactly at the cap threshold is dropped
only where it is the *weakest* kept light, so any residual boundary lands where the dropped light's
own contribution is minimal — the opposite of the slot-index bias, which could drop an arbitrarily
bright light. Raising the cap to 256 (4× today) then makes the threshold bite far less often on the
dense maps that drive the artifact, while keeping the per-fragment loop bounded (vs. the rejected
unbounded cap). 256 is a bounded raise, not a derived optimum; the influence ranking is what makes any
cap safe, and the 16 MB payload hard error remains the backstop.

**Task B falloff — matching the bake, not the dynamic helper.** The static light whose specular this
touches already had its *diffuse* baked into the lightmap through `lightmap_bake.rs::falloff`, which
uses `clamp(1 - d/range)` for `Linear`, `1/d` clamped-at-range for `InverseDistance`, and `1/d²`
clamped-at-range for `InverseSquared`. To fade diffuse and specular of one light together, the
specular helper must mirror *that* function. The dynamic-tier `light_eval_falloff` is a deliberately
different curve (a linear window on `1/d` / `1/d²`, no hard cutoff) tuned for runtime dynamic lights;
reusing it would leave the inverse-model mismatch in place. For `Linear`, matching the bake also
removes the abrupt cutoff for free — `1 - d/range` reaches exactly 0 at `range`, continuously. For the
inverse models the bake itself steps to 0 at `range`; matching it means specular steps at the same
distance as diffuse, so the total is consistent (diffuse already steps there today).

**Files touched.** Task A: `crates/level-format/src/chunk_light_list.rs` (cap constant),
`crates/level-compiler/src/chunk_light_list_bake.rs` (eviction block + tests). Task B:
`crates/lighting/src/spec_buffer.rs` (`cone_cos.w` pack + doc + test),
`crates/renderer/src/shaders/forward.wgsl` (`spec_falloff` helper, `use_specular` attenuation, struct
comment).

## Open questions

- **Dynamic-tier vs. static falloff divergence.** After Task B, static specular matches
  `lightmap_bake.rs::falloff` while dynamic lights keep `light_eval_falloff`'s softened
  inverse-model window — two different inverse-model curves coexist in the engine (baked/static vs.
  runtime/dynamic). This spec intentionally does not unify them: static specular's reference is its
  own baked diffuse, and touching the dynamic curve would change the appearance of every dynamic
  light. Whether the dynamic helper should later adopt the hard-cutoff form for parity across tiers is
  a separate question the owner may decide; it is not a defect this spec introduces (the divergence
  predates it).
- **Cap value.** 256 is a pinned bounded raise, not a measured optimum. If a future dense map still
  overflows 256 in a chunk, influence ranking keeps the artifact minimal (the dropped lights are the
  weakest), but the owner may revisit the number. The CSR format and 16 MB payload backstop impose no
  obstacle to a further raise.
