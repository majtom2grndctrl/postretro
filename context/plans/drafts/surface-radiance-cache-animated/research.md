# surface-radiance-cache-animated — research

Read at 2c0491090. The parent brief's `research.md` holds the estimator, albedo and constraint notes; this file adds only what the delta follow-up needs. The brief's Decisions are the record.

## Today's id 27

- `delta_sh_bake.rs` `bake_subblock`: per CSR entry (affinity cell, animated light), 64 probes. Each probe is clipped to the light's falloff AABB (+0.5 m) and to probe validity, then `bake_probe_indirect_rgb` traces 256 Fibonacci rays. Each hit evaluates the one unit light through `sample_radiance_rgb` with `BounceVisibility::OneSample`. Bounce one only.
- Reach: `affinity_grid` `AffinityTransport::Indirect` keeps the falloff cube intersected with the light's portal flood. The pre-base-SH planner (`pipeline.rs` `plan_delta_bakes`) and the bake assert CSR identity (`ensure_bake_csr_matches_plan`).
- Seeds: `indirect_light_axis` is a blake3 digest of `encoded_key_light(unit_radiance_light(..))`; `light_seed_axes` is all zero, so there is no light-global axis.
- Key: `delta_sh_entry_cache_key` folds the geometry hash (all of `GeometryResult`), the bounce-material hash, affinity dims, cell coordinates, probe spacing, the cell's valid-probe mask, the seed axis, and the encoded unit light. `INDIRECT_DELTA_SH_STAGE_VERSION` is 5.
- Payload: 6×6 RGB16F tile = 216 B per probe, 13,824 B per CSR entry. `campaign-test`: 5 animated lights, 1,424 entries, 91,136 emitted probes against 63,418 valid base probes (`done/occluder-first-partition/measurement/10/round2/rebake/campaign-test.log`).
- Order: ShBake → DeltaShBake → DirectShBake → …, all before atlas preparation (`pipeline.rs` `run_after_parsing`). The dense id 34 RGBA16F source and its analysis clone stay resident through the delta bake.
- Gate: `pipeline/delta_working_set.rs` charges 3× cumulative dense delta bytes: 2× dense plus compaction, and 1× for base copies. It runs before Materials. Nothing charges a co-resident cache.

## What the new key must fold

A unit layer is a function of the unit light, the geometry, the bounce materials and the cache's constants (patch size, lookup, bounce count). The first three are already folded. The constants move with a stage-version bump. Static lights never enter the layer, so a static edit cannot miss an entry. The new input is `_bounce_fill`, under later-bounce deltas. The embedded-emitter flag derives from light and geometry, so it is covered.

## Pricing

All figures *est.*: patch counts come from `gi-rtx-lite/index.md` (hallway ≈1–2M patches at ~0.5 m), and the ray rate from the full hallway's ~621 s SH Bake over ~47 G shadow rays (~75 M rays/s, all cores). Patch gather rays R and bounce count K are the parent's delegated constants; the figures assume R = 64, K = 3.

| Option | Probe side | Patch side, per animated light | RAM resident in the delta bake |
|---|---|---|---|
| Today | 256 gather + ≤256 shadow rays per probe in reach | — | dense delta only |
| Bounce one only | 256 gather rays, no shadow rays | patch direct over patches in reach (4 → 32 probe-and-escalate rays each) | patch structure + Σ reach-patch layers resident at once |
| Later bounces (recommended) | same | + (K−1) gathers over patches within K−1 visibility hops of the light's domain: ≤ (K−1)·P·R ≈ 128–256 M rays worst case, ~2–4 s on the hallway | + up to 3·P·12 B per resident light (≈36–72 MB): bounce-one support, ping-pong, accumulation |
| Later bounces + grown reach | domain grows toward every valid probe | same | dense delta ≈ 962,636 × 216 B ≈ 208 MB per light on the hallway; rejected (128 MiB section floor, 256 MiB `--sh-delta-max-size`, 3× gate) |

The static later-bounce solve costs one such patch-side term. Later-bounce deltas add one per animated light, so the total is (1 + animated lights) × that term. Light-major build keeps the RAM column at one or a few lights. Roadmap surface lights would template many animated lights from one texture, so the per-light term is the one to watch.

Rival: a patch-hit table. Store the hit patch per (patch, gather direction) once, and every later bounce of every light becomes a sparse lookup sum with no rays. It costs P·R·4 B ≈ 256–512 MB bake RAM. A bake-only table of "sample rays hitting other samples" reads toward Crytek US20140327673A1 cl. 1 (`ip-check.md` Piece 6: distinguishable on "real-time" if the claims require it; status unknown).

## Domain truncation (premise correction)

"base + Σ deltas equals the all-static bake" is false today outside each light's domain. The cold base bake has no probe-side reach clip, since `bounce_contribution` early-outs only at the hit, so a probe outside a light's falloff cube that sees a lit wall gets bounce. The delta writes zero there. The parity suite counts delta probes without a base probe, not base probes beyond a delta's domain. The brief therefore claims equality inside the domain only and measures the share outside.

## IP reading

- id 27 already ships per-light bounce: a scalar curve × a per-light, per-probe unit delta (`ip-check.md` Piece 5, '451 (b)/(c) "unclear; the crux"; top counsel question 1). This brief keeps that shape. It changes how the delta is computed (from bake-only per-light surface layers) and what it carries (later bounces).
- '451 cl. 3, "transfer for static-object pixels from a matching lightmap", reads "absent while summed" for the shipped bounce layer (Piece 6). The unit layers are per-light transfer on static surfaces, but bake-only and dropped before packing. Whether a bake-only intermediate can read on cl. 3 is the new counsel question.
- The parent's guard "only summed bounce ships" is already untrue of id 27 as shipped. Its intent is cl. 3 (nothing per-light per texel), and this brief restates it that way.

## Parent premises checked

| Parent claim | Finding |
|---|---|
| IP guard: "Only summed bounce ships" | id 27 ships per-light bounce today; the guard holds for id 34 only |
| research: `INDIRECT_DELTA_SH_STAGE_ID` v3 | v5 at 2c0491090 |
| Path: precedent `delta_working_set.rs` | lives at `pipeline/delta_working_set.rs` |
| Lifetime: cache freed when the SH base bake ends | holds for the parent; this follow-up needs it through DeltaShBake |
| research: per-light patch layers "are the same structure animated deltas need as unit layers" | only if static layers are unit-radiance transport scaled at sum time. If they key on authored colour and intensity, the delta unit layers key differently, and a static ↔ animated flip shares no layer |

## Pin table

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| D1 | Delta gather hits a patch | the light's unit layer is complete before any of its entries gathers | zero shadow rays from delta probes |
| D2 | Light-major build, many workers | layer built → entries baked in parallel → layer freed; output assembled in CSR order | bytes match single-worker; peak holds the resident layers only |
| D3 | Warm, one animated light edited | entry keys are computed before any layer is built | only that light's layer is built; others' entries hit with no layer |
| D4 | Static ↔ animated flip of one light | the static layer is removed from the summed base; the unit layer gathers the same patches | base + delta at peak matches the static bake within the composed bound |
| D5 | Fill edit | entry key folds fill; layer components fill-free | every entry misses; no patch shading if components are cached; warm equals cold |
| D6 | Forced-low budget | gate projects cache + 3× dense before Materials | named error before any patch shade or SH ray |
