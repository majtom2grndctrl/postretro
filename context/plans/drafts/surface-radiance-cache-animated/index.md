# surface-radiance-cache-animated

Brief · compact · reads: `context/lib/build_pipeline.md` §Compiler pipeline, §Build Cache, §PRL section IDs (id 27) · `context/lib/development_guide.md` §1.4 · `context/research/gi-rtx-lite/ip-check.md` Pieces 5–6 · `plans/drafts/surface-radiance-cache/` · read at 2c0491090

## Problem

Owner request (2026-10-30), the deferred half of `surface-radiance-cache`. That brief gathers the SH base (id 34) from a bake-only surface radiance cache with multi-bounce. Animated indirect deltas (id 27) keep the one-sample estimator: each delta probe ray re-shadows its one light at the hit and carries bounce one only. So an animated light's bounce comes from a different estimator than the static bounce beside it and lacks the later bounces, and a light switched between static and animated changes its fill. Done means id 27 gathers from the same cache through per-light unit patch layers carrying the bounces id 34 carries. Inside each light's delta reach, base + Σ deltas at every light's peak equals the all-static bake at any `_bounce_fill`. Delta caching stays exact.

## Decisions

- **One estimator for base and deltas** (owner's original ruling; parent Scope bullet). Each id 27 entry gathers from its light's unit layer: patch outgoing radiance under a unit-radiance copy of the light, shaded by the parent's patch direct with its probe-and-escalate visibility and embedded-emitter rule. A gather hit reads the patch and traces no shadow ray.
- **Deltas carry later bounces** (recommended; owner decides, Open questions). A unit layer is bounce one + `_bounce_fill` × later bounces, with id 34's bounce count and fill. Bounce transport is linear in lights, so only this keeps base + Σ deltas equal to the all-static bake at every fill; bounce one alone holds it only at fill 0. Price: one later-bounce solve per animated light, each about the static solve's cost (`research.md` §Pricing).
- **`_bounce_fill` reaches id 27 as it reaches id 34.** The entry key folds it. Unit-layer components (bounce one, later bounces) are fill-free, so with cached layers a fill edit re-gathers entries and shades no patch.
- **Reach unchanged.** id 27 keeps today's domain (falloff cube ∩ portal flood), and the pre-base-SH CSR plan stays identical. Growing it to every probe that sees a lit patch costs ~208 MB dense per light on the full hallway, past the 128 MiB section floor (`build_pipeline.md` §PRL section IDs). So equality is claimed inside each light's domain only; bounce outside it is dropped, as today, and its share is measured.
- **Exact warm contract kept** (`build_pipeline.md` §Build Cache). A unit layer reads only its light, the geometry and the bounce materials, and seeds from patch ids and the keyed unit light's digest, with no light-global axis; the entry key adds fill to today's fold, and the stage version bumps. A warm build therefore bakes later bounces for every light with a missed entry: this diverges from the parent's bounce-one-only warm rule, which governs only the approximate base, and warm composed output stays dimmer-or-equal to cold.
- **Warm locality.** A unit layer is built only for a light with a missed entry. Editing one animated light rebuilds its layer and misses only its entries. A static-light edit, or an animated colour or intensity edit, builds no layer and misses no entry.
- **Lifetime extends through the delta bake.** The patch structure outlives the SH base bake. It is freed when the indirect delta bake ends, still before atlas preparation, so no pre-cut face identity crosses the cut. This diverges from the parent's free-at-base-end.
- **Bounded.** The SH-delta gate charges the cache resident through the delta bake as its own term beside today's 3× dense factor: the patch structure plus the unit layers resident at once, projected before any SH ray. Overage is the named error, never coarsening (`development_guide.md` §1.4).
- **IP guard.** id 27 keeps its per-light, per-probe unit shape. No per-light layer per patch or texel ships, the cache never reaches PRL, and gather rays stay on the fixed uniform set (`ip-check.md` Pieces 5–6). Build waits on counsel.
- **Deterministic.** Cold id 27 bytes are independent of worker count, arrival order and cache state.
- **Non-goals.** id 34 bytes (the parent owns them); ids 41, 45, 47 and 48, which trace no gather hits. No wire or runtime change: payload shape, `DELTA_SH_VOLUMES_VERSION` and compose stay as they are.

## Acceptance

### Automated
Parity, fill 0: per-probe pre-encode id 27 against the full-set delta under the same visibility rule and production bounce albedo, over probes with nonzero delta.
- [ ] On `campaign-test` and an animated-light fixture, p50, p99 and max differences are no larger than the composed-at-peak BC6H error at the same percentiles, and mean signed relative error is within ±1%.
- [ ] A delta gather traces no shadow ray. Shadow rays come from unit-layer patch direct only, and halving probe spacing leaves their count unchanged.
- [ ] On the embedded-emitter fixture made animated, the light's unit layer matches a no-early-out reference within the parity bound.

Composition:
- [ ] On a fixture whose animated lights' domains cover every valid probe, cold base + Σ deltas at each light's peak matches a cold bake with those lights made static at peak. It holds per probe per channel within the composed BC6H bound at fill 0, an intermediate fill and fill 1.
- [ ] A probe reached only by an animated light's second bounce reads nonzero id 27 at fill 1 and zero at fill 0.

Keys and locality:
- [ ] After inserting an animated light ahead of the others, removing one, reordering, or duplicating one, warm id 27 equals cold byte for byte.
- [ ] Warm, after one animated-light transport edit: one unit layer built, and only that light's entries miss (counts asserted).
- [ ] Warm, after a static-light edit or an animated colour or intensity edit: no unit layer built, zero id 27 misses.
- [ ] A geometry, texture-mean or saturation edit misses every id 27 entry; restoring it hits.
- [ ] Changing `_bounce_fill` changes id 27, and warm equals cold after the change.
- [ ] The first warm build after this change misses every pre-change id 27 entry.

Order and scope:
- [ ] Cold id 27 bytes are identical at one worker and at many, across repeat runs, with a populated cache directory, and across a governor pause during unit-layer shading.
- [ ] id 34, 35, 41, 45, 47 and 48 bytes equal the parent's output on every fixture.
- [ ] The id 27 CSR equals the pre-change plan. The diff touches no file under `crates/level-format`, the runtime crates or shaders (grep gate).
- [ ] Every delta probe gather and unit-layer patch gather traces the same fixed direction set across bakes with different lights and fill; no direction reads cache radiance.

Bounds:
- [ ] A budget one byte below the new projection refuses with the named error before any patch is shaded or ray traced; a budget at the projection admits.
- [ ] Measured peak cache bytes during the delta bake stay within the projection on `stress-warren-mini` and `campaign-test`, serialization buffers included. The cache is freed before atlas preparation.

### Measurement (report, not gate)
- [ ] Cold `--release --no-cache` Delta SH Bake and total, before and after, on `stress-warren-mini` and `campaign-test`. An animated-light sweep at N, 2N and 4N lights, with unit-layer time and gather time reported separately.
- [ ] On `campaign-test`, the share of animated-light bounce energy in the all-static bake that lands on probes outside id 27's domain.

### Manual
- [ ] Owner A/B at animated stations on `campaign-test` and `kinematic-platform` (`--release`) at the chosen fill default. An animated light's fill reads like the static fill beside it, with no patch-scale blotches as it animates.

## Path

- Seams: `delta_sh_bake.rs` `bake_subblock` (replace `bake_probe_indirect_rgb` with a gather over the unit layer), `delta_sh_cache.rs` `delta_sh_entry_cache_key`, `INDIRECT_DELTA_SH_STAGE_VERSION`, `pipeline/delta_working_set.rs` `gate_delta_working_set`, and `pipeline.rs` `run_after_parsing` (ShBake → DeltaShBake). Proof precedents: `sh_bake/bounce_parity_tests.rs` `composed_at_peak` and `delta_base_slots`; `colour_tests.rs` `animated_unit_bounce_composed_at_runtime_peak_matches_equivalent_static_bounce`; `cache_cross_bake_tests.rs` `delta_bakes_keep_indirect_locality_progress_stats_and_policy_identity`.
- Shape: light-major. Build a light's unit layer on its first entry miss, bake its entries, then free the layer, keeping CSR output order. Rival: build every unit layer up front in the base stage. That is simpler, but it holds Σ layers at once.
- Later bounces per light retrace patch gathers. Rival: a bake-only patch-hit table shared across lights (`research.md` §Pricing). It replaces rays with lookups, but needs counsel on Crytek '673 first.
- First slice: bounce-one unit layers feeding id 27 on `campaign-test`, with the parity row. It falsifies the riskiest assumption, that unit-layer discretisation stays inside the composed bound.
- `delta_sh_bake.rs` passes ~800 lines with its in-file tests: move the tests out first, in their own commit.

## Open questions

- Counsel: does '451 cl. 1 read on a scalar curve × a baked per-light, per-probe delta (`ip-check.md` question 1)? Does deriving that delta from bake-only per-light surface layers, dropped before packing, read on cl. 3 when nothing per-texel ships? Does carrying later bounces change either reading? — owner, counsel — **blocks build**
- Deltas carry later bounces, or bounce one only? Recommend later bounces: it is the only form that matches static fill at the default fill, and it costs about one static later-bounce solve per animated light. — owner — **blocks build**
- Whether unit layers persist in the stage cache — **delegated**: the warm fill-edit timing decides.
- Unit-layer storage precision and how many layers are resident at once — **delegated**: the RAM row decides.
