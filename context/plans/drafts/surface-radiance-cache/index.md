# surface-radiance-cache

Brief · resumable · reads: `context/lib/index.md` §2 · `context/lib/build_pipeline.md` §Compiler pipeline, §Build Cache · `context/lib/resource_management.md` §4.5 · `context/lib/development_guide.md` §1.4 · `context/research/gi-rtx-lite/ip-check.md` · read at db827b1df

## Problem

Owner request: multi-bounce indirect light, and SH bake cost that does not grow with light count as maps grow. Levels will outgrow today's maps, and surface lights will multiply light counts. Cause: every probe gather hit shadows every reaching light, so shadow rays scale with probes × rays × lights; a second bounce on that estimator multiplies again, so shadowed areas get no fill from light bounced twice. Done means the SH base (id 34) gathers outgoing radiance from a surface radiance cache built once per bake, with multi-bounce: gather cost no longer depends on light count, and patch shading scales with patches × lights in reach. An author's warm build shows the same indirect light the release build ships.

## Decisions

- **Gather from a bake-only surface radiance cache.** Patches on static world faces store outgoing radiance: bounce albedo × irradiance. A gather ray reads the patch it hits and traces no shadow ray.
- **Patch direct uses probe-and-escalate visibility.** Today's light evaluation and early-outs, under the SH probe bakes' embedded-emitter rule, which lands first (owner, 2026-10-30). Patches shade once, so a converged estimate is affordable. Parity therefore compares against the test-only full-set bounce, which uses the same visibility, and measures discretisation alone; the shipping one-sample estimator is the speed baseline.
- **Scope: id 34.** Animated indirect deltas (id 27) keep today's estimator until `surface-radiance-cache-animated` follows (owner, 2026-10-30), after counsel reads '451 cl. 1 against per-light bounce. Until then an animated light's bounce lacks the cache's later bounces; accepted. Ids 35, 41, 45, 47 and 48 trace no gather hits.
- **Albedo is the production bounce albedo.** Coloured bounce has landed; patches store what the gather reads today. Bounce stays physical.
- **Multi-bounce is in scope (owner), a look change judged by A/B.** Later bounces gather patch to patch over a fixed bounce count. A `worldspawn` key `_bounce_fill` ∈ [0, 1] weights them: shipped bounce = bounce one + fill × later bounces. To authors it is how much light keeps bouncing past the first hit: `0` contrasty single bounce, `1` full physical fill. The A/B picks the default. The name stays clear of ericw-tools' `_bouncescale`, which scales all bounced light and would mislead Quake mappers. Invalid values fall back to the default with a warning. FGD and `docs/level_design.md` carry it; no compiler flag (`index.md` §2). This reverses `done/coloured-sh-bounce`'s no-knob ruling for later bounces only.
- **Warm id 34 equals cold.** Warm SH approximated indirect light because every gather hit re-shadowed its reaching lights (`build_pipeline.md` §Build Cache); the cache removes that cost, so warm builds bake every bounce and match release byte for byte. An author tuning lights on warm builds sees what ships, at any fill. Locality lives where the cost scales with lights: a light edit re-shades only that light's bounce-one layer (owner's warm-locality ruling, kept for the shadow-ray work). Later bounces and the probe gather re-run over the summed cache, costing gather rays only.
- **Keys follow content, not list position.** Patch layers key on the light's content, positions and bounce materials, the inputs SH group keys already fold. Seeds derive from patch ids and the keyed light's digest, as id 27 already seeds, so inserting or reordering lights never changes a cached layer's bytes.
- **IP guard (owner).** Gather rays stay uniform or cosine, never aimed by a prior result (ARM '177). Id 34 ships only summed bounce; nothing per light reaches a texel or patch in PRL. The cache never reaches PRL (`ip-check.md` Piece 6).
- **Lifetime.** The cache is built after Geometry on pre-cut faces and freed when the SH base bake ends, before atlas preparation; no face identity crosses the cut (`build_pipeline.md` §Compiler pipeline). Bake-only: no format, runtime or shader change.
- **Bounded.** Patches exist only on static world faces after exterior culling. The SH base stage joins working-set accounting, with the cache its first charge; overage is a named error raised before any patch is shaded, never automatic coarsening (`development_guide.md` §1.4).
- **Deterministic.** Cold bytes are independent of worker count, arrival order and cache state.
- **Proof is the slope.** Levels will outgrow today's maps, so a light-count sweep shows gather cost flat; no measurement of today's maps gates the build. Absolute speed-up is reported, not gated.
- **Non-goal: emissive bounce.** It reverses the rule that emissive surfaces light nothing (`resource_management.md` §4.5); a separate owner ruling.
- **Non-goal: texture-level bounce multiplier.** Expected by designers from other radiosity tools; decided with surface lights' per-texture properties. Additive later, since keys already fold bounce materials.

## Acceptance

### Automated
Parity: per-probe pre-encode irradiance, single-bounce cache vs the full-set bounce under the same visibility rule and production bounce albedo, over probes with nonzero bounce.
- [ ] On `campaign-test` and a fixture room, id 34 differences at p50, p99 and max are no larger than the BC6H encode error the baseline volume carries at the same percentiles.
- [ ] Over the same probes, mean signed relative error is within ±1%.
- [ ] A cold bake traces no shadow ray from a probe gather hit; shadow rays come from patch direct only, and halving probe spacing or doubling rays per probe leaves that count unchanged (cfg(test) counters; research P1).
- [ ] id 34 probe depth moments are byte-identical to the one-sample bake's (research P12).
- [ ] A probe whose rays all miss geometry reads black; a map with zero static lights bakes an all-zero id 34.
- [ ] A map lit only by emissive textures bakes an all-zero id 34 and allocates no patch layer (research P10).
- [ ] A back-face hit reads what the baseline reads for it, within the parity bound.
- [ ] On the embedded-emitter fixture, patches beside a flagged light match a no-early-out reference within the parity bound (research P11).
- [ ] id 27 bytes equal the pre-change bake's.

Multi-bounce:
- [ ] A probe in a room lit only by bounce off an adjacent lit room reads brighter with multi-bounce than single-bounce; a probe in direct-lit open space changes by less than the parity bound.
- [ ] On a fixture whose albedo is below one, each added bounce adds less energy than the one before.
- [ ] A closed room of albedo-1 faces at fill 1 bakes a finite id 34, no brighter than bounce count × bounce one per probe per channel (research P9).
- [ ] At fill 0, id 34 bytes equal single-bounce output; at fill 1, the unscaled multi-bounce sum; between, each probe reads between the two per channel.
- [ ] A patch reached only by a later-bounce gather stores its bounce-one radiance; the skipped set holds only patches no gather of any bounce reads, identical at one worker and at many (research P2).

Locality and keys:
- [ ] A patch reached by no light stores zero bounce-one radiance; a patch inside a light's range, facing it, with a clear segment to it, stores nonzero.
- [ ] Warm, after one light edit: only that light's patch layer re-shades (counts asserted); every other layer hits cache.
- [ ] After any sequence of light, geometry and `_bounce_fill` edits, warm id 34 bytes equal a cold bake of the final map.
- [ ] With no edit, a warm build hits every patch layer and every SH group.
- [ ] After removing a static light, inserting one ahead of the others, or adding a duplicate, every patch layer served from cache equals that light's layer in a cold bake of the edited map, byte for byte (research P3, P4).
- [ ] A geometry, texture-mean or saturation edit re-keys every patch layer and SH group; restoring it hits cache (research P5, P6).
- [ ] Changing `_bounce_fill` re-shades no patch layer; id 34 moves with it, warm and cold alike.
- [ ] The first warm build after this change misses every pre-cache SH group entry (research P7).
- [ ] `_bounce_fill` appears in the FGD and `docs/level_design.md`; a non-finite or out-of-range value bakes the default and warns once; no `prl-build` flag sets it.

Order and determinism:
- [ ] Cold id 34 bytes identical at one worker and at many, across repeat runs, with a populated build cache directory, and across a governor pause during patch shading.
- [ ] Two cold bakes that differ only in lightmap density, so in atlas-preparation face cuts, give identical id 34 bytes.
- [ ] Every probe and patch gather traces the same fixed direction set across bakes with different lights and fill; no gather direction reads cache radiance (test, plus a grep gate on gather-direction sources).
- [ ] The diff touches no file under `crates/level-format`, the runtime crates or shaders, and adds no `SectionId` (diff and grep gate).

Bounds:
- [ ] Cache peak bytes and lifetime measured on the full hallway and charged by the SH base stage's working-set accounting; freed before atlas preparation.
- [ ] A forced-low budget refuses with the named error before any patch is shaded or probe ray traced (research P8).
- [ ] On the full hallway, patch-layer cache entries' total bytes are reported against `--cache-max-size`; serialization buffers count toward the cache's peak.
- [ ] Patch count equals the patches over the Geometry stage's static world faces; a fixture with a mover and an exterior-culled face gets no patch on either.

### Measurement (report, not gate)
- [ ] Warm SH Bake after one light edit on the full hallway, against today's warm build.
- [ ] Light-count sweep: one synthetic map at N, 2N and 4N static lights; gather time and patch-direct time reported separately, cold, one bake at a time.
- [ ] Cold `--release --no-cache` Build Summary on hallway-mini, the full hallway, `stress-warren-mini` and `campaign-test`, before (one-sample estimator) and after: SH Bake and total. Share of patches gather rays reach.

### Manual
- [ ] Owner A/B at `campaign-test` stations on `--release` builds: fill 0, an intermediate fill, and fill 1, plus a closed white-walled room. Shadowed fill reads right, no hot spots, overall energy acceptable; the owner picks the default.
- [ ] Entities and fog under the new id 34 show no patch-scale blotches or new leaks.

## Path

- Shape: per-light sparse patch layers for bounce one, over the patches in each light's reach. Cold sums static layers; warm groups sum their reaching lights (`research.md` §Warm-locality shape). Layers store unit transport, scaled by colour × intensity when summed, so a colour or intensity edit re-bakes no layer and the animated follow-up shares them; it extends the cache's lifetime through the delta bake. Rival: one summed layer, which re-keys every group on any light edit.
- Later bounces gather over the summed bounce-one layer. SH groups key on the summed cache's digest, so any light edit re-gathers every group; that costs gather rays, not shadow rays.
- First slice: single-bounce cache feeding id 34 on `campaign-test`, with the parity row. It falsifies the riskiest assumption, that patch discretisation stays inside the BC6H bound.
- Seams: `sh_bake.rs` `sample_radiance_rgb` and `closest_hit_in_leaf` (hit → face → patch), `bake_probe_indirect_rgb`, `sh_group.rs` `group_cache_key`, `pipeline.rs` `run_after_parsing`; working-set precedent `pipeline/delta_working_set.rs`.
- `sh_bake.rs` is large; split before extending if it passes ~800 lines.
- Measurement runs follow `measurements/bake-performance/measure-hallway.ps1 -Cold`; the owner launches the full-hallway run.

## Open questions

- Bounce count — **delegated**: the convergence row decides; a code constant, changed with a stage-version bump.
- Patch size, and nearest vs filtered lookup across patches — **delegated**: the parity row and the RAM row decide.
- Whether a patch stores both sides of a face or back-face hits are shaded another way — **delegated**: the back-face row decides.
