# surface-radiance-cache

Brief · resumable · reads: `context/lib/build_pipeline.md` §Compiler pipeline, §Build Cache · `context/lib/resource_management.md` §4.5 · `context/lib/development_guide.md` §1.4 · `context/research/gi-rtx-lite/ip-check.md` · read at 704ed232e

## Problem

Owner request: reclaim bake time after the lightmap penumbra floor doubled the Lightmap Bake stage (hallway-mini cold total 440 s → 638 s), and keep SH bake cost from growing with light count as maps grow. The SH indirect bake is about half the full hallway's cold compile. Cause: for each of 256 gather rays per probe, the bake re-shadows every reaching light at the hit point, so shadow rays scale with probes × rays × lights. A one-sample estimator lands first (Path) and cuts the per-light ray count, but not the scaling. The bake is also single-bounce, so shadowed areas get no fill from light bounced twice. Done means the SH base (id 34) and animated indirect deltas (id 27) read outgoing radiance from a surface radiance cache built once per bake, with multi-bounce. Cold SH bake time is measured against the one-sample estimator on hallway-mini, the full hallway, `stress-warren-mini` and `campaign-test`.

## Decisions

- **Gather from a bake-only surface radiance cache.** Fixed-size patches on world faces store outgoing radiance: albedo × irradiance. Gather rays look up the patch they hit instead of tracing shadow rays. Patch direct irradiance reuses today's light evaluation, early-outs and `soft_visibility`, so only the place shadows are traced changes.
- **Prerequisite: the one-sample estimator lands and is measured first (owner: least visual difference first).** This brief is re-scoped against that measurement before promotion.
- **Scope: id 34 and id 27.** Base and deltas share one estimator, so an animated light's bounce matches the static bounce beside it (owner). Ids 35, 41, 45, 47 and 48 trace no gather hits and keep their paths.
- **Albedo is whatever the bake uses when this builds.** That is grey today; coloured bounce is its own step and may land first. The cache reproduces the pre-cache estimator in single-bounce mode.
- **Multi-bounce is in scope (owner), a look change judged by A/B.** Bounce two and later gathers patch to patch from the cache. A `prl-build` switch selects single-bounce for the parity row; it is listed in `--help`, absent from `docs/`, and folded into the SH keys.
- **Warm locality kept (owner).** Editing one light re-bakes only that light's bounce-one patch radiance and the SH groups and delta entries it reaches. Warm stays dimmer-or-equal to cold, never miscoloured (`done/incremental-bake-per-element/`).
- **IP guard (owner).** Gather rays stay uniform or cosine and are never aimed using a prior result (ARM '177). Only summed bounce ships. The cache drops before packing and no patch list reaches PRL (`ip-check.md` Piece 6).
- **The cache lives between Geometry and atlas preparation and never reaches PRL.** It is keyed on positions, built on pre-cut faces and dropped before atlas preparation. No face identity crosses the cut (`build_pipeline.md` §Compiler pipeline). Bake-only: no format, runtime or shader change; SH and delta stage versions bump.
- **Deterministic.** Cold bytes are independent of worker count, arrival order and cache state; seeds derive from stable patch ids and global light indices.
- **Bounded.** Patches exist only on world faces after exterior culling. Patches no gather ray reaches are not shaded if that saves meaningfully; the executor measures the hit share and reports it (`development_guide.md` §1.4). The cache's full lifetime and every co-resident copy are charged to the SH stages' working-set accounting. Overage is a named error, never automatic coarsening.
- **Speed is measured, not gated (owner).** A missed parity row stops for the owner; a small speed-up does not.
- **Non-goal: emissive bounce.** It reverses the rule that emissive surfaces light nothing (`resource_management.md` §4.5); a separate owner ruling.
- **Non-goal: embedded-emitter bias in probe bakes.** Parity requires keeping today's emitter path; the roadmap item fixes it afterward.
- **Non-goal: coloured bounce.** Its own step (owner); this brief carries whatever albedo exists.

## Acceptance

### Automated
Parity: per-probe pre-encode irradiance, single-bounce cache vs the one-sample estimator's bake, over probes with nonzero bounce.
- [ ] On `campaign-test` and a fixture room, id 34 differences at p50, p99 and max are no larger than the BC6H encode error the baseline volume carries at the same percentiles.
- [ ] On a fixture with an animated light, id 27 meets the same bound, and base + Σ deltas at full animation equals the all-static bake of the same lights within it.
- [ ] A probe whose rays all miss geometry reads black; a map with zero static lights bakes an all-zero id 34.
- [ ] A back-face hit reads what the baseline estimator reads for it.

Multi-bounce:
- [ ] A probe in a room lit only by bounce off an adjacent lit room reads brighter with multi-bounce than single-bounce; a probe in direct-lit open space changes by less than the parity bound.
- [ ] Each added bounce adds less energy than the one before, and the total converges under a fixed bounce count.

Locality and keys:
- [ ] A patch reached by no light stores zero bounce-one radiance; a patch in reach stores nonzero.
- [ ] Warm, after one light edit: only that light's patch layer, and only groups and delta entries whose reaching sets hold it, re-bake (counts asserted). Groups beyond its reach hit cache.
- [ ] Toggling single-bounce mode re-keys the SH stages; toggling it back hits cache.
- [ ] Warm output is never brighter than cold per probe per channel.

Order and determinism:
- [ ] Cold bytes identical at one worker and at many, and across repeat runs.
- [ ] Atlas-preparation face cuts leave id 34 and id 27 unchanged.

Bounds:
- [ ] Cache peak bytes and lifetime measured on the full hallway; counted by the working-set accounting; freed before atlas preparation. A forced-low budget fails with the named error.

### Measurement (report, not gate)
- [ ] Cold `--release --no-cache` Build Summary on hallway-mini, the full hallway, `stress-warren-mini` and `campaign-test`, before (one-sample estimator) and after: SH Bake, Delta SH Bake and total, serialized one bake at a time. Share of patches gather rays reach.

### Manual
- [ ] Owner A/B, single- vs multi-bounce, at `campaign-test` stations: shadowed fill reads right, no hot spots, overall energy acceptable.
- [ ] Entities and fog under the new id 34 show no patch-scale blotches or new leaks.

## Path

- Shape: per-light sparse patch layers for bounce one, over the patches in each light's reach. Cold sums static layers, warm groups sum their reaching lights, and deltas read unit layers (`research.md` §Warm-locality shape). Rival: one summed static layer, which re-keys every group on any light edit.
- Multi-bounce cost: per-light layers make a per-light second bounce cost about lights × patches, because bounced light spreads past each light's reach. Summed-layer bounces avoid that but key globally (open question below).
- First slice: single-bounce cache feeding id 34 on `campaign-test`, with the parity row. It falsifies the riskiest assumption, that patch discretisation stays inside the BC6H bound.
- Seams: `sh_bake.rs` `sample_radiance_rgb` and `closest_hit_in_leaf` (hit → face → patch), `bake_probe_indirect_rgb`, `sh_group.rs` `group_cache_key`, `delta_sh_cache.rs` `delta_sh_entry_cache_key`, `pipeline.rs` `run_after_parsing`.
- `sh_bake.rs` is large; split before extending if it passes ~800 lines.
- Measurement runs follow `measurements/bake-performance/measure-hallway.ps1 -Cold`; the owner launches the full-hallway run.

## Open questions

- Re-scope after the one-sample estimator is measured: does the cache's remaining saving, plus multi-bounce, still earn this shape? — owner — **blocks build**
- Multi-bounce under warm locality: warm builds bake bounce one only (dimmer-or-equal, local) while cold adds later bounces, or bounces two and later re-bake globally on any light edit? Recommend warm bounce one only. — owner — **blocks build**
- Animated lights under multi-bounce: do deltas carry bounces two and later, or bounce one only? — owner — **blocks build**
- Patch size, and nearest vs filtered lookup across patches — **delegated**: the parity row and the RAM row decide.
- Whether a patch stores both sides of a face or back-face hits are shaded another way — **delegated**: the back-face row decides.
