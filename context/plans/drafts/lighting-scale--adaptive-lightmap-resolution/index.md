# lighting-scale--adaptive-lightmap-resolution

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4 · `context/lib/build_pipeline.md` §Build Cache · §Baked texture mips · `context/lib/experimental_spikes.md` · read at a097035

## Problem
Owner-raised, performance capability. Static lightmap irradiance charts bake at uniform texel
density — every chart is full-resolution regardless of how much high-frequency detail its lighting
carries. The goal is a style-agnostic default that stores each sector's irradiance at the coarsest
resolution whose bilinear reconstruction stays within an imperceptible error of the full bake,
recovering VRAM where lighting is smooth or absent while leaving sharp static shadows and hard
spotlight edges pixel-crisp. Two things are unproven and must gate the shippable surface: whether
the recovered irradiance VRAM is material to the low-end-GPU (GTX 1660) goal at all — the irradiance
atlas is BC6H-compact and may be a minor term beside the SH volume — and whether the post-hoc
downsample+repack survives the cache-key/layout inversion. So this lands in two phases: a
build-to-learn gate first, the shippable coarsening only if the gate returns green.

## Decisions
- **Two-phase, gated execution.** Phase 1 is build-to-learn: an output-preserving `--lightmap-analyze`
  pass + a one-leaf inversion proof, delivered as a findings note. Phase 2 (the coarsening repack,
  boundary grading, escape valve) builds only if the note shows a material win and a clean inversion.
  The owner decides how much surface to commit at the phase boundary — the note characterizes, it is
  not an automated threshold.
- **Supersede the authored-only stance, on a new premise.** `plans/done/lighting-scale--lightmap-bake-scaling`
  rejected a measured classifier assuming the coarsening target was un-measurable gameplay intent.
  That premise is withdrawn: the goal is near-imperceptible VRAM reduction, and a reconstruction-error
  gate is a representation-efficiency tool (band-limiting), not an intent proxy — harmonizing the
  lightmap track with the SH `variable-base-probe-density` measured classifier. Capture the
  superseding stance in `context/lib` at promotion. Undo cost: the prior decision is a done record.
- **Signal = post-hoc reconstruction error, gated on the sharpest feature.** Bake full-res, then per
  sector pick the coarsest candidate level whose downsample→bilinear-reconstruct error vs the full
  bake stays under tolerance (mirrors `sh_coarsen.rs::classify_levels` rel_p95/rel_max; darkness/empty
  is the trivial-pass sub-case). Any sector with a sharp static-shadow terminator or hard spotlight
  cutoff fails the gate and stays full-res — the two failure modes the owner named are refused by
  construction. Layer: compiler, post-hoc over the finished dense bake; never a runtime change.
- **Downsample with a linear-space low-pass prefilter before decimation** (irradiance is linear HDR
  f32; the compiler's Mitchell-Netravali texture-mip downsample is a reuse candidate). Runtime
  reconstruction is the existing irradiance linear sampler — no runtime or format change.
- **≤1-level boundary grading** between adjacent sectors (`sh_coarsen.rs::smooth_pair`).
- **Sector unit = per-BVH-leaf** (the packer groups charts by leaf, sizes a shared layer via
  `choose_layer_dim`); per-chart is the finer alternative (Path).
- **No new content-facing surface this brief.** The `lightmap_protect_volume` FGD entity is dropped —
  the automatic gate protects measurable hardness by construction, and a force-fine escape valve, if
  Phase 2 needs one, reuses the shipped `_lightmap_scale` lever or a CLI-only protect flag, not a new
  modder one-way door. The `_lightmap_scale` KVP and the post-hoc gate **compose**: the author sets a
  region's starting density, the gate then coarsens from whatever that bake produced.
- **Measurement characterizes, never gates the concept.** `--lightmap-analyze` is output-preserving
  (measure-and-report); it sizes the win and shows where coarsening acts. It is never a user setting.
- **Non-goals:** true section-skip / a zero-static runtime path (coarsen-to-minimum reaches the empty
  case; a black atlas stays correct — the no-static placeholder is white and ungated, so a runtime
  zero-static path is deferred and riskier); occlusion-culling never-seen faces; a per-surface opt-out
  flag; the direction and shadowmask atlases (irradiance only); denoise (analytic + deterministic bake).

## Acceptance

### Phase 1 — gate (committed)
Automated (honesty gates):
- [ ] `--lightmap-analyze` runs on `campaign-test`, `closet-reveal`, and a hard-spotlight fixture and
  emits, per sector, the coarsest level passing the reconstruction-error gate, its error, and
  recovered texels/bytes.
- [ ] Output-preserving: emitted PRL bytes identical with and without the flag.
- [ ] One-leaf inversion proof: a full-bake→downsample-one-leaf→repack path yields a correct atlas
  whose cache key reflects the final (post-coarsen) layout; two `--no-cache` runs are byte-identical.

Manual (measured findings — gate the Phase 2 decision, not thresholds):
- [ ] Findings note: recovered irradiance VRAM per map AND its share of total lightmap + SH VRAM (so
  materiality against the 1660 goal is visible, per the reviewer's L1 concern); the inversion's
  cleanliness; a promote / adjust / stop recommendation for the owner.

### Phase 2 — shippable coarsening (gated on the Phase 1 note)
Automated:
- [ ] Hard static shadow preserved: a sharp static-geometry terminator keeps its sector(s) full-res.
- [ ] Hard spotlight edge preserved: a hard-edged spotlight pool keeps its edge sector(s) full-res.
- [ ] Smooth sector coarsened (permit side): a soft-gradient lit region coarsens ≥1 level, error under tolerance.
- [ ] Empty collapse: an all-black atlas coarsens to the minimum; sampled irradiance stays zero.
- [ ] Determinism: warm and cold bakes produce the identical coarsened atlas; two `--no-cache` runs byte-identical.
- [ ] ≤1-level grading: no two adjacent sectors differ by more than one level.

Manual:
- [ ] Visual A/B (campaign-test, closet-reveal, hard-spotlight scene): no perceptible smudging of
  static shadows or spotlight edges at default tolerance; VRAM recovered recorded per map.

## Path
- **Phase 1 is the first slice, and it is the make-or-break.** Coarsening needs the baked atlas, but
  layout is decided pre-bake in `prepare_atlas` (shared warm/cold) — so it is a SECOND stage
  (full-res bake → classify → downsample + repack) and the cache fingerprint must reflect the
  POST-coarsen layout. Prove that on one leaf, and measure the win in context, before any Phase 2
  surface. If the inversion doesn't hold cleanly, the shape is wrong.
- Seams by symbol: gate mirrors `sh_coarsen.rs::classify_levels`; analyze pass mirrors output-preserving
  `--sh-analyze` (`sh_analyze.rs`); measurement seam = post-dilate `CompositedAtlas`
  (`pipeline/lightmap_stage.rs::bake_fused_prepared`); downsample candidate = the Mitchell-Netravali
  texture-mip filter; layout/cache = `atlas_layout_fingerprint` / `section_input_hash`
  (`lightmap_layer.rs`), packer `pack_layers` / `choose_layer_dim`.
- Dilation/gutter ordering interacts with downsampling — decide downsample-then-dilate vs
  dilate-then-downsample so gutters stay valid at reduced resolution.
- Rival: forward density prediction (choose density before baking) — rejected: frequency content
  isn't knowable pre-bake, and a warm-approximate predictor breaks determinism.

## Open questions
- Sector unit per-leaf vs per-chart — **delegated**.
- Error-tolerance default and metric (rel_p95/rel_max vs perceptual) — **delegated**: conservative default; analyze pass + A/B tune it.
- Phase 2 force-fine escape valve, if needed: reuse `_lightmap_scale`, a CLI-only protect flag, or none — **delegated** to the Phase 2 decision, informed by the Phase 1 note.
