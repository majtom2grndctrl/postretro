# surface-radiance-cache

Brief · resumable · reads: `context/lib/index.md` §2 · `context/lib/build_pipeline.md` §Compiler pipeline, §Build Cache · `context/lib/resource_management.md` §4.5 · `context/lib/development_guide.md` §1.4 · `context/research/gi-rtx-lite/ip-check.md` · read at faaaf8680

## Problem

Owner request: multi-bounce indirect light, and SH bake cost that does not grow with light count. Levels will outgrow today's maps, and surface lights will multiply light counts. Cause: every probe gather hit shadows every reaching light, so shadow rays scale with probes × rays × lights, and a second bounce on that estimator multiplies again; shadowed areas get no fill from light bounced twice. To keep that cost local, warm builds bound each probe group's lights, so authors iterate on dimmer indirect light than release ships. Done means the SH base (id 34) gathers from a surface radiance cache built once per bake, with multi-bounce: gather cost no longer depends on light count, and a warm build bakes the bytes release ships.

## Decisions

- **Gather from a bake-only surface radiance cache.** Patches on static world faces store outgoing radiance, what the gather reads today. A gather ray reads the patch it hits and traces no shadow ray.
- **Per-light bounce-one layers.** Each static light shades its own sparse layer over the patches in its reach; layers sum into one cache before any gather, and shading meets the parity rows. A light edit re-shades only its layer, keeping the cost that scales with lights local; the layers are also the structure the animated follow-up reads. Layers bake the light's colour and intensity in: unit transport scaled at the sum reads toward Tencent '451 cl. 1 (`ip-check.md` Piece 3).
- **Embedded emitters.** Patch direct takes the probe-point bakes' rule, which lands first (owner): a flagged light traces its full set. It tests every static point or spot light with an emitter sphere, sdf-shadowed included, since all of them bounce, and flags those whose sphere reaches into geometry. A layer's key folds only its own light's flag and the penumbra-floor setting, so one flag never re-shades another light's layer.
- **Parity measures discretisation.** The reference is the test-only full-set bounce under the same per-light embedded-emitter rule and production bounce albedo.
- **Scope: id 34.** Animated deltas (id 27) keep today's estimator until `surface-radiance-cache-animated` follows (owner), after counsel reads '451 cl. 1. Until then an animated light's bounce lacks later bounces; accepted.
- **Multi-bounce (owner), judged by A/B.** Later bounces gather patch to patch over a fixed bounce count, across every static world face, so a room beyond every light's reach still takes bounced fill. A `worldspawn` key `_bounce_fill` ∈ [0, 1] weights them: shipped bounce = bounce one + fill × later bounces. `0` is single bounce, `1` full physical fill; the A/B picks the default. An invalid value falls back to the default with a warning; no compiler flag (`index.md` §2). This reverses `done/coloured-sh-bounce`'s no-knob ruling for later bounces only.
- **Warm id 34 equals cold.** Warm and cold bake id 34 through one path: layers from cache, then the sum, later bounces and the whole-volume gather. The gathered volume caches as one entry over every input the gather reads; grouped SH caching retires, since any light edit moves every group. Animation descriptors and the slot table rebuild every build, as today. A warm build ships release bytes at any fill, so the warm-approximation warning retires; `--release` stays the ship mode, as it reads and writes no cache. A warm build stopped mid-gather loses that gather, which every light edit re-runs anyway; accepted.
- **Keys follow content.** Layers key on the light's content, the geometry, bounce materials and their own flag, never list position. Reordering entities leaves id 34's probe tiles and depth moments unchanged.
- **IP guard (owner).** Gather rays stay uniform or cosine, never aimed by a prior result (ARM '177). Per-light layers live only in the bake and its build cache and sum before any gather; id 34 ships summed bounce, and the cache never reaches PRL (`ip-check.md` Piece 6). That reading of '451 cl. 3 for a dropped bake intermediate is engineering's; it joins the animated follow-up's counsel questions, and if counsel disagrees, one summed patch-direct pass replaces the layers.
- **Lifetime.** Built after Geometry on pre-cut faces, freed when the SH base stage ends, before the delta SH stage; no face identity crosses the atlas cut. Bake-only: no format, runtime or shader change.
- **Bounded.** Patches exist only on static world faces after exterior culling. The SH base stage joins working-set accounting with the cache as its first charge, priced before any ray or texture read; overage is a named error, never automatic coarsening (`build_pipeline.md` §Compiler pipeline, working-set gate; lifetime per `development_guide.md` §1.4).
- **Deterministic.** Cold bytes are independent of worker count, arrival order and cache state.
- **Proof is the slope.** No measurement of today's maps gates the build.
- **Non-goal: emissive bounce.** It reverses the rule that emissive surfaces light nothing (`resource_management.md` §4.5); a separate owner ruling.
- **Non-goal: texture-level bounce multiplier.** Designers from other radiosity tools expect one; the surface-lights brief decides it with its per-texture properties. Additive later, since keys already fold bounce materials.

## Acceptance

### Automated
Parity: per-probe pre-encode irradiance, single-bounce cache vs the reference, over probes with nonzero bounce, on `campaign-test` and a fixture room.
- [ ] id 34 differences at p50, p99 and max are no larger than the BC6H encode error the baseline volume carries at the same percentiles; the report sets the reference's reseed control (`FullSetReseeded`) beside them.
- [ ] Mean signed relative error is within ±1%. A back-face hit reads what the reference reads for it, within the parity bound.
- [ ] On the embedded-emitter fixture, with a flagged point light and a flagged sdf-shadowed light, patches match a no-early-out reference within the parity bound; ids 35, 41, 47 and 48, their keys and their warnings equal the base commit's, and the sdf-shadowed flag warns once per compile, naming the bounce bake (research P10).

Cost:
- [ ] A cold bake traces no shadow ray from a probe gather hit; halving probe spacing or doubling rays per probe leaves the shadow-ray count unchanged (cfg(test) counters, plus a grep gate: the gather path calls no visibility routine; research P1).
- [ ] Doubling a fixture's static lights leaves probe-gather ray and patch-lookup counts unchanged; patch-direct light evaluations equal the sum over lights of patches in each light's reach (cfg(test) counters).

Multi-bounce:
- [ ] A probe in a room lit only by bounce off an adjacent lit room reads brighter with multi-bounce than single-bounce; a probe whose rays hit only faces beyond every light's reach reads nonzero at fill 1 and zero at fill 0; a probe in direct-lit open space changes by less than the parity bound.
- [ ] On a fixture whose albedo is below one, each added bounce adds less area-weighted patch energy than the one before. A closed room of albedo-1 faces at fill 1 bakes a finite id 34, each bounce's area-weighted patch energy no larger than the previous one's within discretisation tolerance (research P8).
- [ ] At fill 0, id 34 bytes equal single-bounce output; at fill 1, the unscaled multi-bounce sum; between, each probe's pre-encode irradiance reads between the two per channel.

Warm builds and keys:
- [ ] Warm id 34 bytes equal a cold bake of the final map after each of: edit A, then B, then revert to A; one light removed and another added in one edit; an entity reorder; a colour edit; `_bounce_fill` changed with a light edit; every static light removed; a geometry edit; a validity-only brush edit; a probe-spacing change; an animated light added, removed or re-timed. The fixture has a probe that gathers patches lit by a light beyond today's 16 m group reach (research P2, P9, P12, P13, P18, P20, P21).
- [ ] After one light's edit, only its layer re-shades and the re-gather traces no shadow ray; after only an entity reorder, or no edit, every layer hits (cfg(test) counters; research P2).
- [ ] After removing a static light, inserting one ahead of the others, adding a duplicate, adding then removing a light whose reach holds faces no other light reaches, or flipping one to animated (`_animated` or `setLightAnimation`), every unchanged light's layer is served from cache and equals its cold layer byte for byte (research P2, P3, P22).
- [ ] Moving a flagged light into or out of a wall re-shades only its layer; toggling `--no-penumbra-floor` re-shades only flagged lights' layers; changing `--soft-shadow-samples` re-shades none (research P16, P19).
- [ ] Reversing the static lights' entity order leaves cold id 34 probe tiles and depth moments identical. A duplicated static light doubles cold patch radiance in its reach within float tolerance, and adding it warm misses the id-34 entry (research P2, P3).
- [ ] A geometry, texture-mean or saturation edit re-keys every layer and the id-34 entry; restoring it hits cache (research P4, P5). Changing `_bounce_fill` re-shades no layer; id 34 moves with it, warm and cold alike.
- [ ] An animated-light-only edit re-shades no layer and leaves id 34 probe tiles and depth moments unchanged; descriptors and slot table equal a cold bake's (research P17, P21).
- [ ] A layer removed, corrupted, or unfinished when a warm build was stopped mid patch shading (governor enter hook) re-shades alone and the rest hit; after A, B, revert A, the revert hits every layer the eviction sweep kept. Eviction moves hit counts, never bytes (research P12, P15, P25).
- [ ] A warm build into an empty cache directory, or after only `--release` builds, misses every layer and gives `--release` bytes; the first warm build after this change misses every pre-change id-34 entry (research P6, P14).
- [ ] A static light whose reach holds no patch shades nothing and allocates no layer; cold id 34 equals the map's without it (research P23).
- [ ] `_bounce_fill` appears in the FGD and `docs/level_design.md`. A non-finite or out-of-range value bakes the default and warns once, cold and warm, including a warm build whose id-34 entry hits.
- [ ] The coloured-bounce knob gate admits `_bounce_fill` as the one worldspawn key and FGD field naming bounce, and still finds no bounce, albedo or saturation `prl-build` flag.
- [ ] A warm build emits no SH-approximation warning; `prl-build --help`, `build_pipeline.md` §Build Cache and `testing_guide.md` no longer describe warm indirect light as approximate, per-group SH entries, or their determinism exemption (doc grep gate).

Order and determinism:
- [ ] Cold id 34 bytes identical at one worker and at many, across repeat runs, with a populated build cache directory, and across a governor pause during patch shading.
- [ ] Two cold bakes that differ only in lightmap density or a `_lightmap_scale` region, so in atlas-preparation face cuts, give identical id 34 bytes, and a warm rebuild hits every layer and the id-34 entry without rewrite; the pre-atlas memo lists and the material-stat persistence gate name the new stage ids and cache-writing file in place of `sh_group`.
- [ ] Every probe and patch gather traces the same fixed direction set across bakes with different lights and fill; no gather direction reads cache radiance (test, plus a grep gate on gather-direction sources).
- [ ] Patch-layer and id-34 cache gets and puts run outside governor permits.

Regression guards (pass on a build that leaves the Problem unsolved):
- [ ] id 34 probe depth moments are byte-identical to the one-sample bake's (research P11).
- [ ] id 27 bytes equal the base commit's (`cmp` of `--release` PRLs on `stress-warren-mini` and an animated-light fixture). The first warm build hits every pre-change id-27, id-35, id-47/48 and chunk-light-list entry: key helpers leaving `sh_group.rs` keep their bytes.
- [ ] With zero static lights, or lit only by emissive textures, id 34 irradiance is all zero and no layer is allocated; a probe whose rays all miss reads black (research P9).
- [ ] The diff touches no file under `crates/level-format`, the runtime crates or shaders, and adds no `SectionId` (diff and grep gate).

Bounds:
- [ ] A forced-low budget refuses by the SH base gate's name before any patch is shaded, probe ray traced, embedded-emitter direction tested or texture read, cold and warm alike; at its logged prediction a build admits and one byte below refuses; the delta and lightmap boundary checks still hold (research P7, P24).
- [ ] The gate's patch set equals the set the bake shades, asserted at the handoff as the delta CSR identity is; on every fixture, at fill 0 and 1, one worker and many, cold and warm, predicted bytes are no smaller than the measured peak of layers, sum, later-bounce buffers and the id-34 entry's encode and decode (research P27).
- [ ] The cache is freed when the SH base stage ends, before the delta SH stage begins (research P26).
- [ ] Patch count equals the patches over static world faces after exterior culling; a fixture with a mover and an exterior-culled face gets no patch on either.

### Measurement (report, not gate)
- [ ] Warm SH Bake after one light edit on the full hallway, with patch shading, later bounces and gather timed apart, against today's warm build.
- [ ] Light-count sweep: one synthetic map at N, 2N and 4N static lights; gather and patch-direct time apart, cold, one bake at a time.
- [ ] Cold `--release --no-cache` Build Summary on hallway-mini, the full hallway, `stress-warren-mini` and `campaign-test`, before (one-sample estimator) and after: SH Bake and total.
- [ ] On the full hallway: cache peak bytes, and patch-layer plus id-34 entry bytes against `--cache-max-size`.

### Manual
- [ ] Owner A/B at `campaign-test` stations on `--release` builds: fill 0, an intermediate fill, and fill 1, plus a closed white-walled room. Shadowed fill reads right, no hot spots, overall energy acceptable; the owner picks the default.
- [ ] Entities and fog under the new id 34 show no patch-scale blotches or new leaks.

## Path

- First slice: single-bounce cache feeding id 34 on `campaign-test`, with the parity rows. It falsifies the riskiest assumption, that patch discretisation stays inside the BC6H bound.
- `sh_bake.rs` is past 800 lines: split it first, behaviour-preserving, in its own commit.
- The whole-volume cold path becomes the only id-34 path. `sh_group.rs`'s grouped path and `build_shell` retire; its shared key helpers (`geometry_content_hash`, `bounce_material_hash`, `fold_bounce_hash`) move out unchanged.
- Static embedded-emitter classification moves ahead of ShBake in `run_after_parsing` and after the SH base gate, still shared by ids 35, 41, 47 and 48; patch direct needs a variant that keeps sdf-shadowed lights. `BounceVisibility::FullSet` calls plain `soft_visibility` today and needs the per-light rule.
- Mechanism candidates: store irradiance and apply albedo / π at the gather or at shading; sum layers in digest order; key the id-34 entry on the summed digest plus geometry, grid layout, validity, fill and stage version; seed by patch id and the light's digest, as id 27 seeds by its keyed unit light; derive patch ids from the face and the position on it, never from rank in the patch set; write each layer to cache as it finishes.
- Seams: `sh_bake.rs` `sample_radiance_rgb`, `closest_hit_in_leaf` (hit → face → patch) and `bake_probe_indirect_rgb`; `pipeline.rs` `run_after_parsing`; working-set precedent `pipeline/delta_working_set.rs` and `tests/lightmap_working_set_contract.rs`; worldspawn parse precedent `parse.rs` (`_sh_density_fidelity`); knob gate `tests/coloured_bounce_source_gates.rs`.
- Measurement runs follow `measurements/bake-performance/measure-hallway.ps1 -Cold`; the owner launches the full-hallway run.

## Open questions

- Bounce count — **delegated**: the convergence rows decide; a code constant, changed with a stage-version bump.
- Patch size, and nearest vs filtered lookup across patches — **delegated**: the parity rows and the gate's prediction row decide.
- Whether a patch stores both sides of a face or back-face hits are shaded another way — **delegated**: the back-face row decides.
