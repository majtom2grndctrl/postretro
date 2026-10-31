# surface-radiance-cache

Brief · resumable · reads: `context/lib/index.md` §2 · `context/lib/build_pipeline.md` §Compiler pipeline, §Build Cache · `context/lib/resource_management.md` §4.5 · `context/lib/development_guide.md` §1.4 · `context/research/gi-rtx-lite/ip-check.md` · read at 60cc94d09

## Problem

Owner request: multi-bounce indirect light, and SH bake cost that does not grow with light count. Levels will outgrow today's maps, and surface lights will multiply light counts. Cause: every probe gather hit shadows every reaching light, so shadow rays scale with probes × rays × lights; a second bounce on that estimator multiplies again, so shadowed areas get no fill from light bounced twice. To keep that cost local, warm builds bound each probe group's lights, so authors iterate on dimmer indirect light than release ships. Done means the SH base (id 34) gathers from a surface radiance cache built once per bake, with multi-bounce: gather cost no longer depends on light count, patch shading scales with patches × lights in reach, and a warm build bakes the bytes release ships.

## Decisions

- **Gather from a bake-only surface radiance cache.** Patches on static world faces store outgoing radiance, bounce albedo × irradiance / π, what the gather reads today. A gather ray reads the patch it hits and traces no shadow ray.
- **Per-light patch layers.** Each static light shades its own layer over the patches in its reach, with today's light evaluation, early-outs and probe-and-escalate visibility; layers sum into one cache before any gather. A light edit re-shades only its layer, so the cost that scales with lights stays local. Layers bake the light's colour and intensity in: unit transport scaled at the sum reads toward Tencent '451 cl. 1 (`ip-check.md` Piece 3) and waits for the animated follow-up's counsel read.
- **Embedded emitters.** Patch direct takes the probe-point bakes' rule, which lands first (owner, 2026-10-30): a flagged light traces its full set. It flags every static point or spot light with an emitter sphere, sdf-shadowed lights included, since all of them bounce. Layer keys fold the flagged set, as id 35's key does.
- **Parity measures discretisation.** The reference is the test-only full-set bounce under the same per-light embedded-emitter rule and production bounce albedo. The shipping one-sample estimator is the speed baseline.
- **Scope: id 34.** Animated deltas (id 27) keep today's estimator until `surface-radiance-cache-animated` follows (owner, 2026-10-30), after counsel reads '451 cl. 1. Until then an animated light's bounce lacks later bounces; accepted.
- **Multi-bounce (owner), judged by A/B.** Later bounces gather patch to patch over the summed cache for a fixed bounce count. A `worldspawn` key `_bounce_fill` ∈ [0, 1] weights them: shipped bounce = bounce one + fill × later bounces. `0` is single bounce, `1` full physical fill; the A/B picks the default. Invalid values fall back to the default with a warning, as `_sh_density_fidelity` does; no compiler flag (`index.md` §2). This reverses `done/coloured-sh-bounce`'s no-knob ruling for later bounces only.
- **Warm id 34 equals cold.** Warm and cold bake id 34 through one path: layers from cache, then the sum, later bounces and the whole-volume gather. The volume caches as one entry keyed on the summed cache's digest; grouped SH caching retires, since any light edit moves every group. A warm build ships release bytes at any fill, so the warm-approximation warning retires; `--release` stays the ship mode, as it reads and writes no cache. Re-running later bounces and the gather costs gather rays, not shadow rays.
- **Keys follow content.** Layers key on the light's content, the geometry, bounce materials and the flagged set, never list position. Layers sum in digest order, so reordering entities leaves id 34 bytes unchanged.
- **IP guard (owner).** Gather rays stay uniform or cosine, never aimed by a prior result (ARM '177). Per-light layers live only in the bake and its build cache and are summed before any gather; id 34 ships summed bounce, and the cache never reaches PRL (`ip-check.md` Piece 6).
- **Lifetime.** Built after Geometry on pre-cut faces, freed when the SH base bake ends, before atlas preparation; no face identity crosses the cut. Bake-only: no format, runtime or shader change.
- **Bounded.** Patches exist only on static world faces, after exterior culling, inside some light's reach. The SH base stage joins working-set accounting with the cache as its first charge, priced before any ray; overage is a named error, never automatic coarsening (`development_guide.md` §1.4).
- **Deterministic.** Cold bytes are independent of worker count, arrival order and cache state.
- **Proof is the slope.** A light-count sweep shows gather cost flat; no measurement of today's maps gates the build.
- **Non-goal: emissive bounce.** It reverses the rule that emissive surfaces light nothing (`resource_management.md` §4.5); a separate owner ruling.
- **Non-goal: texture-level bounce multiplier.** Designers from other radiosity tools expect one; it is decided with surface lights' per-texture properties. Additive later, since keys already fold bounce materials.

## Acceptance

### Automated
Parity: per-probe pre-encode irradiance, single-bounce cache vs the reference, over probes with nonzero bounce, on `campaign-test` and a fixture room.
- [ ] id 34 differences at p50, p99 and max are no larger than the BC6H encode error the baseline volume carries at the same percentiles; the report sets the reference's reseed control (`FullSetReseeded`) beside them.
- [ ] Mean signed relative error is within ±1%.
- [ ] A back-face hit reads what the reference reads for it, within the parity bound.
- [ ] On the embedded-emitter fixture, with a flagged point light and a flagged sdf-shadowed light, patches match a no-early-out reference within the parity bound (research P10).

Cost:
- [ ] A cold bake traces no shadow ray from a probe gather hit; halving probe spacing or doubling rays per probe leaves the shadow-ray count unchanged (cfg(test) counters, plus a grep gate: the gather path calls no visibility routine; research P1).
- [ ] Doubling a fixture's static lights leaves probe-gather ray and patch-lookup counts unchanged; patch-direct light evaluations equal the sum over lights of patches in each light's reach (cfg(test) counters).

Multi-bounce:
- [ ] A probe in a room lit only by bounce off an adjacent lit room reads brighter with multi-bounce than single-bounce; a probe in direct-lit open space changes by less than the parity bound.
- [ ] On a fixture whose albedo is below one, each added bounce adds less energy than the one before.
- [ ] A closed room of albedo-1 faces at fill 1 bakes a finite id 34, no brighter than bounce count × bounce one per probe per channel (research P8).
- [ ] At fill 0, id 34 bytes equal single-bounce output; at fill 1, the unscaled multi-bounce sum; between, each probe reads between the two per channel.

Warm builds and keys:
- [ ] Warm id 34 bytes equal a cold bake of the final map after each of: edit A, then B, then revert to A; one light removed and another added in one edit; an entity reorder; a colour edit; `_bounce_fill` changed with a light edit; every static light removed; a geometry edit. The fixture has a probe that gathers patches lit by a light beyond today's group reach (`falloff_range` + `SH_REACH_CUTOFF_METERS`) (research P2, P9, P12, P13, P18).
- [ ] After one light's edit, only its layer re-shades (counts asserted); every other layer hits cache, and the re-gather traces no shadow ray (cfg(test) counters). With no edit, a warm build hits every layer and the id-34 entry.
- [ ] After removing a static light, inserting one ahead of the others, adding a duplicate, or flipping one to animated (`_animated` or `setLightAnimation`), every layer served from cache equals that light's layer in a cold bake of the edited map, byte for byte (research P2, P3).
- [ ] A geometry, texture-mean or saturation edit re-keys every layer and the id-34 entry; restoring it hits cache (research P4, P5).
- [ ] Changing `_bounce_fill` re-shades no layer; id 34 moves with it, warm and cold alike.
- [ ] Toggling `--no-penumbra-floor` on the embedded-emitter fixture serves no layer shaded under the other setting; changing `--soft-shadow-samples` re-shades no layer (research P16).
- [ ] An animated-light-only edit re-shades no layer and leaves id 34 bytes unchanged (research P17).
- [ ] With a layer removed or corrupted, a warm build re-shades only that layer and hits the rest; after A, B, revert A, the revert hits every layer the eviction sweep kept. Eviction moves hit counts, never bytes (research P12, P15).
- [ ] A warm build into an empty cache directory, or after only `--release` builds, misses every layer and gives `--release` bytes (research P14).
- [ ] The first warm build after this change misses every pre-change id-34 cache entry (research P6).
- [ ] A warm build emits no SH-approximation warning, and `build_pipeline.md` §Build Cache states warm id 34 equals cold.
- [ ] `_bounce_fill` appears in the FGD and `docs/level_design.md`; a non-finite or out-of-range value bakes the default and warns once. The coloured-bounce knob gate admits `_bounce_fill` as the one worldspawn key and FGD field naming bounce, and still finds no bounce, albedo or saturation `prl-build` flag.

Order and determinism:
- [ ] Cold id 34 bytes identical at one worker and at many, across repeat runs, with a populated build cache directory, and across a governor pause during patch shading.
- [ ] Two cold bakes that differ only in lightmap density, so in atlas-preparation face cuts, give identical id 34 bytes.
- [ ] Every probe and patch gather traces the same fixed direction set across bakes with different lights and fill; no gather direction reads cache radiance (test, plus a grep gate on gather-direction sources).

Regression guards (pass on a build that leaves the Problem unsolved):
- [ ] id 34 probe depth moments are byte-identical to the one-sample bake's (research P11).
- [ ] id 27 bytes equal the base commit's (`cmp` of `--release` PRLs on `stress-warren-mini` and an animated-light fixture).
- [ ] With zero static lights, or lit only by emissive textures, id 34 irradiance is all zero, depth moments equal the one-sample bake's, and no layer is allocated; a probe whose rays all miss reads black (research P9).
- [ ] The diff touches no file under `crates/level-format`, the runtime crates or shaders, and adds no `SectionId` (diff and grep gate).

Bounds:
- [ ] Cache peak bytes and lifetime measured on the full hallway and charged by the SH base stage's working-set accounting; freed before atlas preparation.
- [ ] A forced-low budget refuses with the named error before any patch is shaded or probe ray traced (research P7).
- [ ] On the full hallway, patch-layer cache entries' total bytes are reported against `--cache-max-size`; serialization buffers count toward the cache's peak.
- [ ] Patch count equals the patches over static world faces inside some light's reach; a fixture with a mover and an exterior-culled face gets no patch on either.

### Measurement (report, not gate)
- [ ] Warm SH Bake after one light edit on the full hallway, with patch shading, later bounces and gather timed apart, against today's warm build.
- [ ] Light-count sweep: one synthetic map at N, 2N and 4N static lights; gather time and patch-direct time reported apart, cold, one bake at a time.
- [ ] Cold `--release --no-cache` Build Summary on hallway-mini, the full hallway, `stress-warren-mini` and `campaign-test`, before (one-sample estimator) and after: SH Bake and total. Share of shaded patches no gather reads.

### Manual
- [ ] Owner A/B at `campaign-test` stations on `--release` builds: fill 0, an intermediate fill, and fill 1, plus a closed white-walled room. Shadowed fill reads right, no hot spots, overall energy acceptable; the owner picks the default.
- [ ] Entities and fog under the new id 34 show no patch-scale blotches or new leaks.

## Path

- First slice: single-bounce cache feeding id 34 on `campaign-test`, with the parity rows. It falsifies the riskiest assumption, that patch discretisation stays inside the BC6H bound.
- The whole-volume cold path becomes the only id-34 path, cached as one entry. The grouped warm path (`sh_group.rs`, its `build_shell`), the warm warning, and tests of the retired approximation go with it (`testing_guide.md`).
- Static embedded-emitter classification moves ahead of ShBake in `run_after_parsing`, still shared by ids 35, 41, 47 and 48. Patch direct needs a variant that keeps sdf-shadowed lights. `BounceVisibility::FullSet` calls plain `soft_visibility` today and needs the per-light rule.
- Seeds can follow patch id and the light's digest, as id 27's follow its keyed unit light.
- Seams: `sh_bake.rs` `sample_radiance_rgb`, `closest_hit_in_leaf` (hit → face → patch) and `bake_probe_indirect_rgb`; `pipeline.rs` `run_after_parsing`; working-set precedent `pipeline/delta_working_set.rs`; worldspawn parse precedent `parse.rs`; knob gate `tests/coloured_bounce_source_gates.rs`.
- `sh_bake.rs` is large; split before extending if it passes ~800 lines.
- Measurement runs follow `measurements/bake-performance/measure-hallway.ps1 -Cold`; the owner launches the full-hallway run.

## Open questions

- Bounce count — **delegated**: the convergence rows decide; a code constant, changed with a stage-version bump.
- Patch size, and nearest vs filtered lookup across patches — **delegated**: the parity rows and the RAM row decide.
- Whether a patch stores both sides of a face or back-face hits are shaded another way — **delegated**: the back-face row decides.
