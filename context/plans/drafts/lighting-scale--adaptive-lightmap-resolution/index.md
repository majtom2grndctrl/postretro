# lighting-scale--adaptive-lightmap-resolution

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4 · `context/lib/build_pipeline.md` §Build Cache · §Baked texture mips · `context/lib/experimental_spikes.md` · read at a097035

## Problem
Owner-raised, performance capability. Static lightmap charts bake at uniform texel density —
every chart is full-resolution regardless of how much high-frequency detail its lighting
actually carries. On a low-end GPU (GTX 1660) this spends VRAM the frame budget can't afford.
The goal is a style-agnostic default that stores each sector's lightmap at the coarsest
resolution whose bilinear reconstruction stays within an imperceptible error of the full bake —
recovering VRAM/disk where lighting is smooth or absent, while leaving sharp static shadows and
hard spotlight edges pixel-crisp. When done: prl-build bakes at full resolution, then per sector
selects the coarsest level passing a reconstruction-error gate, downsamples with a linear-space
prefilter, repacks, and records where it acted; sharp edges and author-protected regions stay
full-res.

## Decisions
- **Supersede the authored-only stance, on a new premise.** `plans/done/lighting-scale--lightmap-bake-scaling`
  rejected a measured error/contrast classifier because it assumed the coarsening target was
  un-measurable gameplay intent. That premise is withdrawn: the goal now is near-imperceptible
  VRAM reduction for low-end GPUs, and a reconstruction-error gate is a representation-efficiency
  tool (band-limiting), not an intent proxy. Capture the superseding stance in `context/lib` at
  promotion. Undo cost: the prior decision is a done record, not code.
- **Signal = post-hoc reconstruction error, gated on the sharpest feature.** Bake full-res, then
  per sector pick the coarsest candidate level whose downsample→bilinear-reconstruct error vs the
  full bake stays under tolerance (mirrors `sh_coarsen.rs::classify_levels` rel_p95/rel_max;
  darkness/empty is the trivial-pass sub-case). Any sector containing a sharp static-shadow
  terminator or hard spotlight cutoff fails the gate and stays full-res — the two failure modes
  the owner named are refused by construction. Layer: compiler, post-hoc over the finished dense
  bake (determinism); never a runtime change.
- **Downsample with a linear-space low-pass prefilter before decimation.** Irradiance is linear
  HDR f32; the compiler's existing Mitchell-Netravali linear downsample (texture-mip path) is a
  reuse candidate. Runtime reconstruction is the existing irradiance linear sampler — no runtime
  or format change.
- **≤1-level boundary grading** between adjacent sectors (`sh_coarsen.rs::smooth_pair` precedent),
  so resolution never jumps abruptly across a seam.
- **Sector unit = per-BVH-leaf** — the packer already groups charts by leaf and sizes a shared
  layer via `choose_layer_dim`. Per-chart is the finer alternative (Path).
- **Artist force-fine hatch.** A brush entity `lightmap_protect_volume` (+ `--lightmap-protect-aabb`),
  mirroring `sh_protect_volume`, hard-pins overlapping sectors to full-res, overriding the gate —
  the backstop for a near-threshold edge the author wants crisp. Union-composed CLI + map, like
  the SH hatch.
- **Characterization pass, not a gate.** `--lightmap-analyze` (output-preserving) reports per-sector
  chosen level, reconstruction error, and recovered texels, so an author can see where coarsening
  acted and catch an unwanted softening. Measure-and-report; never a user setting.
- **Non-goals** (warranted where a reader would assume owed): true section-skip / a zero-static
  runtime path — coarsen-to-minimum already collapses empty atlases and a black atlas stays
  correct at runtime, so a new zero-static representation is deferred and riskier; occlusion-culling
  never-seen faces; a per-surface material opt-out flag; the direction and shadowmask atlases
  (irradiance only this pass); denoise (analytic + deterministic bake — no noise).

## Acceptance

### Automated
- [ ] Hard static shadow preserved: a fixture with a sharp static-geometry shadow terminator keeps
  its terminator sector(s) at full resolution; reconstruction error there stays under tolerance.
- [ ] Hard spotlight edge preserved: a fixture with a hard-edged spotlight pool keeps its edge
  sector(s) at full resolution.
- [ ] Smooth sector coarsened (permit side): a large soft-gradient lit region with no sharp edges
  coarsens at least one level, reconstruction error under tolerance.
- [ ] Empty collapse: an all-black static atlas coarsens to the minimum and sampled irradiance
  stays zero (black stays black, not white).
- [ ] Determinism: two `--no-cache` bakes of one map are byte-identical pre-BC6H; warm and cold
  bakes produce the identical coarsened atlas.
- [ ] Force-fine override: a sector inside a `lightmap_protect_volume` stays full-res even where
  the gate would coarsen; a sector outside every volume follows the gate.
- [ ] `--lightmap-analyze` is output-preserving: emitted PRL bytes identical with and without it.
- [ ] ≤1-level grading: no two adjacent sectors differ by more than one resolution level.

### Manual
- [ ] Visual A/B (campaign-test, closet-reveal, a hard-spotlight scene): no perceptible smudging of
  static shadows or spotlight edges at default tolerance; atlas-byte/VRAM reduction recorded per map.
- [ ] Findings note: recovered VRAM per map at default tolerance, the tolerance's visual headroom,
  and the analysis pass's per-sector level histogram.

## Path
- **Riskiest assumption, first slice.** The coarsening decision needs the baked atlas, but layout
  is decided pre-bake in `prepare_atlas` (shared warm/cold). So coarsening is a SECOND stage:
  full-res bake → classify per sector → downsample + repack into a new, smaller layout → the cache
  fingerprint must reflect the POST-coarsen layout. First slice: prove a
  full-bake → downsample-one-leaf → repack path yields a correct, deterministic atlas whose cache
  key reflects the final layout. If that inversion doesn't hold cleanly, the shape is wrong.
- Seams by symbol: gate mirrors `sh_coarsen.rs::classify_levels` (rel_p95/rel_max, `smooth_pair`);
  measurement seam = post-dilate `CompositedAtlas` (`pipeline/lightmap_stage.rs::bake_fused_prepared`
  is the live path, not `bake_layered_section_controlled`); downsample candidate = the
  Mitchell-Netravali filter in the texture-mip path; layout/cache = `atlas_layout_fingerprint` /
  `section_input_hash` (`lightmap_layer.rs`), packer `pack_layers` / `choose_layer_dim`; hatch
  mirrors `sh_protect_volume` (`parse.rs`, `pipeline.rs::combined_protect_aabbs`, FGD).
- Dilation/gutter ordering interacts with downsampling — decide downsample-then-dilate vs
  dilate-then-downsample so gutters stay valid at reduced resolution.
- Rival: forward density prediction (choose density before baking) — rejected: frequency content
  isn't knowable before the bake, and a warm-approximate predictor breaks determinism.

## Open questions
- Sector unit per-leaf vs per-chart — **delegated**: executor picks; leaf is the packer's natural
  unit, per-chart is finer.
- Error-tolerance default and metric (rel_p95/rel_max vs a perceptual measure) — **delegated**:
  executor picks a defensible, conservative default; the analysis pass and manual A/B tune it.
