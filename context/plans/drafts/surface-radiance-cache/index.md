# surface-radiance-cache

Brief · resumable · reads: `context/lib/build_pipeline.md` §Compiler pipeline, §Build Cache · `context/lib/resource_management.md` §4.5 · `context/lib/development_guide.md` §1.4 · `context/research/gi-rtx-lite/ip-check.md` · read at 704ed232e

## Problem

Owner request: multi-bounce indirect light, and SH bake cost that does not grow with light count as maps grow. Cause: for each of 256 gather rays per probe, the bake shadows every reaching light at the hit point, so shadow rays scale with probes × rays × lights. A second bounce on that estimator would multiply it again, so the single-bounce bake leaves shadowed areas with no fill from light bounced twice. The one-sample estimator (PR #618) cut cold SH Bake 1.86× on the full hallway (648 → 349 s) but not the scaling. The near-term bake-time win now comes from it and the lightmap culls, and coloured bounce needs no cache, so neither is this brief's case (owner, 2026-10-25). Done means the SH base (id 34) and animated indirect deltas (id 27) read outgoing radiance from a surface radiance cache built once per bake, with multi-bounce. Cold SH bake time is measured against the one-sample estimator on hallway-mini, the full hallway, `stress-warren-mini` and `campaign-test`.

## Decisions

- **Gather from a bake-only surface radiance cache.** Fixed-size patches on world faces store outgoing radiance: albedo × irradiance. Gather rays look up the patch they hit instead of tracing shadow rays. Patch direct irradiance reuses today's light evaluation, early-outs and `soft_visibility`, so only the place shadows are traced changes.
- **Sequenced after the lightmap culls and coloured bounce (owner).** A sizing pass first measures the share of one-sample SH Bake still spent on shadow rays at gather hits; it prices the scaling half of the case before promotion.
- **Scope: id 34 and id 27.** Base and deltas share one estimator, so an animated light's bounce matches the static bounce beside it (owner). Ids 35, 41, 45, 47 and 48 trace no gather hits and keep their paths.
- **Albedo is whatever the bake uses when this builds.** That is grey today; coloured bounce is its own step and may land first. The cache reproduces the pre-cache estimator in single-bounce mode.
- **Multi-bounce is in scope (owner), a look change judged by A/B.** Bounce two and later gathers patch to patch from the cache. Fill strength is a mood choice, not only a correctness one, so a `prl-build` later-bounce scale s ∈ [0, 1] weights it: shipped bounce = bounce one + s × later bounces, wherever later bounces ship. s = 0 is single-bounce mode, used by the parity row; default 1. The scale is listed in `--help`, absent from `docs/`, and folded into the SH keys.
- **Warm locality kept (owner).** Editing one light re-bakes only that light's bounce-one patch radiance and the SH groups and delta entries it reaches. Warm stays dimmer-or-equal to cold, never miscoloured (`done/incremental-bake-per-element/`).
- **IP guard (owner).** Gather rays stay uniform or cosine and are never aimed using a prior result (ARM '177). Only summed bounce ships. The cache drops before packing and no patch list reaches PRL (`ip-check.md` Piece 6).
- **The cache lives between Geometry and atlas preparation and never reaches PRL.** It is keyed on positions, built on pre-cut faces and dropped before atlas preparation. No face identity crosses the cut (`build_pipeline.md` §Compiler pipeline). Bake-only: no format, runtime or shader change; SH and delta stage versions bump.
- **Deterministic.** Cold bytes are independent of worker count, arrival order and cache state; seeds derive from stable patch ids and global light indices.
- **Bounded.** Patches exist only on world faces after exterior culling. Patches no gather ray reaches are not shaded if that saves meaningfully; the executor measures the hit share and reports it (`development_guide.md` §1.4). The cache's full lifetime and every co-resident copy are charged to the SH stages' working-set accounting. Overage is a named error, never automatic coarsening.
- **Speed is measured, not gated (owner).** A missed parity row stops for the owner; a small speed-up does not.
- **Non-goal: emissive bounce.** It reverses the rule that emissive surfaces light nothing (`resource_management.md` §4.5); a separate owner ruling.
- **Patch direct takes the embedded-emitter early-out.** A light whose emitter sphere reaches into geometry can hide a cap from all four probes, so on the plain all-clear early-out its patches would read too bright where the one-sample baseline, which has no early-out, does not. The SH probe fix lands first (owner, 2026-10-30) and gives direct SH and scatter the lightmap's per-light rule (`texel_footprint::EmbeddedEmitters`, `ProbeEarlyOut::for_light`); patch direct reuses it, so a flagged light traces its full set at any patch with a clear probe.
- **Non-goal: coloured bounce.** Its own step (owner); this brief carries whatever albedo exists.

## Acceptance

### Automated
Parity: per-probe pre-encode irradiance, single-bounce cache vs the one-sample estimator's bake, over probes with nonzero bounce.
- [ ] On `campaign-test` and a fixture room, id 34 differences at p50, p99 and max are no larger than the BC6H encode error the baseline volume carries at the same percentiles.
- [ ] On a fixture with an animated light, id 27 meets the same bound, and base + Σ deltas at full animation equals the all-static bake of the same lights within it.
- [ ] A probe whose rays all miss geometry reads black; a map with zero static lights bakes an all-zero id 34.
- [ ] A back-face hit reads what the baseline estimator reads for it, within the parity bound.
- [ ] A cold bake traces no shadow ray from a probe gather hit; shadow rays come from patch direct only, and halving probe spacing or doubling rays per probe leaves that count unchanged (cfg(test) counters; research P1).
- [ ] Over the same probes, the single-bounce cache's mean signed relative error against the one-sample bake is within ±1%, for id 34 and id 27.
- [ ] id 34 probe depth moments are byte-identical to the one-sample bake's (research P14).
- [ ] On the embedded-emitter fixture, patches beside a flagged light store its full-set visibility, and patch radiance matches a no-early-out reference within the parity bound (research P13).
- [ ] A map lit only by emissive textures bakes an all-zero id 34 and allocates no patch layer (research P11).

Multi-bounce:
- [ ] A probe in a room lit only by bounce off an adjacent lit room reads brighter with multi-bounce than single-bounce; a probe in direct-lit open space changes by less than the parity bound.
- [ ] Each added bounce adds less energy than the one before, and the total converges under a fixed bounce count.
- [ ] At s = 0, id 34 and id 27 bytes equal single-bounce output; at s = 1, the unscaled multi-bounce sum; between, each probe reads between the two per channel.
- [ ] With s > 0, a patch reached only by a later-bounce gather stores its bounce-one radiance; the reach skip drops only patches no gather of any bounce reads, and the skipped set is identical at one worker and at many (research P2).
- [ ] A closed room of albedo-1 faces at s = 1 bakes a finite id 34, no brighter than bounce count × bounce one per probe per channel (research P10).

Locality and keys:
- [ ] A patch reached by no light stores zero bounce-one radiance; a patch inside a light's range, facing it, with a clear segment to it, stores nonzero.
- [ ] Warm, after one light edit: only that light's patch layer, and only groups whose reaching sets held it before or hold it after the edit, re-bake; a static edit re-bakes no indirect delta entry, and an animated edit only that light's entries (counts asserted). Groups beyond its reach hit cache.
- [ ] Changing s re-keys the SH stages; restoring it hits cache.
- [ ] Warm output is never brighter than cold per probe per channel.
- [ ] After removing a static light, inserting one ahead of the others, or adding a duplicate of one, every patch layer served from cache equals the same light's layer in a cold bake of the edited map, byte for byte; layers of lights whose global index moved re-bake (research P3, P4).
- [ ] After reordering animated lights in the map, warm id 27 equals cold id 27 byte for byte (research P5).
- [ ] A geometry edit, a texture-mean edit or a saturation edit re-keys every patch layer, SH group and indirect delta entry; restoring the edit hits cache (research P6, P7).
- [ ] The first warm build after this change misses every pre-cache `sh_group` and indirect delta entry (research P8).
- [ ] The later-bounce scale is listed in `prl-build --help`, a non-finite or out-of-range value fails before any stage runs, and no file under `docs/` names it (grep gate).

Order and determinism:
- [ ] Cold id 34 and id 27 bytes identical at one worker and at many, and across repeat runs.
- [ ] Cold id 34 and id 27 bytes are unchanged by a populated build cache directory and by a governor pause during patch shading.
- [ ] Two cold bakes that differ only in lightmap density, so in atlas-preparation face cuts, give identical id 34 and id 27 bytes.
- [ ] Every probe and patch gather traces the same fixed direction set across two bakes with different lights and s, and no gather direction reads cache radiance (test, plus a grep gate on gather-direction sources).
- [ ] The diff touches no file under `crates/level-format`, the runtime crates or shaders, and adds no `SectionId`; a cold `campaign-test` bake's section list equals the pre-change list (diff and grep gate).

Bounds:
- [ ] Cache peak bytes and lifetime measured on the full hallway; counted by the working-set accounting; freed before atlas preparation. A forced-low budget fails with the named error.
- [ ] A forced-low budget refuses with the named error before any patch is shaded or probe ray traced (research P9).
- [ ] On the full hallway, patch-layer cache entries' total bytes are measured and reported against `--cache-max-size`; serialization buffers count toward the cache's peak.
- [ ] A map with no animated lights builds no unit layer (research P12).
- [ ] Patch count equals the patches over the Geometry stage's static world faces; a fixture with a mover and an exterior-culled face gets no patch on either.

### Measurement (report, not gate)
- [ ] Sizing pass, before build: share of cold one-sample SH Bake and Delta SH Bake spent on shadow rays at gather hits, on the full hallway and `stress-warren-mini`, recorded in `research.md`.
- [ ] Cold `--release --no-cache` Build Summary on hallway-mini, the full hallway, `stress-warren-mini` and `campaign-test`, before (one-sample estimator) and after: SH Bake, Delta SH Bake and total, serialized one bake at a time. Share of patches gather rays reach.

### Manual
- [ ] Owner A/B at `campaign-test` stations on `--release` builds: s = 0, an intermediate s, and s = 1. Shadowed fill reads right, no hot spots, overall energy acceptable; the owner picks the default s.
- [ ] Entities and fog under the new id 34 show no patch-scale blotches or new leaks.

## Path

- Shape: per-light sparse patch layers for bounce one, over the patches in each light's reach. Cold sums static layers, warm groups sum their reaching lights, and deltas read unit layers (`research.md` §Warm-locality shape). Rival: one summed static layer, which re-keys every group on any light edit.
- Multi-bounce cost: per-light layers make a per-light second bounce cost about lights × patches, because bounced light spreads past each light's reach. Summed-layer bounces avoid that but key globally (open question below).
- First slice: single-bounce cache feeding id 34 on `campaign-test`, with the parity row. It falsifies the riskiest assumption, that patch discretisation stays inside the BC6H bound.
- Seams: `sh_bake.rs` `sample_radiance_rgb` and `closest_hit_in_leaf` (hit → face → patch), `bake_probe_indirect_rgb`, `sh_group.rs` `group_cache_key`, `delta_sh_cache.rs` `delta_sh_entry_cache_key`, `pipeline.rs` `run_after_parsing`.
- `sh_bake.rs` is large; split before extending if it passes ~800 lines.
- Measurement runs follow `measurements/bake-performance/measure-hallway.ps1 -Cold`; the owner launches the full-hallway run.

## Open questions

- After the sizing pass: does the shadow-ray share, plus multi-bounce, still earn this shape? — owner — **blocks build**
- Multi-bounce under warm locality: warm builds bake bounce one only (dimmer-or-equal, local) while cold adds later bounces, or bounces two and later re-bake globally on any light edit? Recommend warm bounce one only. — owner — **blocks build**
- Animated lights under multi-bounce: do deltas carry bounces two and later, or bounce one only? — owner — **blocks build**
- Later-bounce scale: bake-wide only, or also a per-map authored value? The A/B decides. — owner — does not block the first slice
- Patch size, and nearest vs filtered lookup across patches — **delegated**: the parity row and the RAM row decide.
- Whether a patch stores both sides of a face or back-face hits are shaded another way — **delegated**: the back-face row decides.
