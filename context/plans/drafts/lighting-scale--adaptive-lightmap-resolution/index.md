# lighting-scale--adaptive-lightmap-resolution

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4 · `context/lib/build_pipeline.md` §Build Cache · §Baked texture mips · `context/lib/experimental_spikes.md` · read at a097035

## Problem
Owner-raised, performance capability. Static lightmap irradiance charts bake at uniform texel
density — every chart is full-resolution regardless of how much high-frequency detail its lighting
carries. The goal is a style-agnostic default that stores each sector's irradiance at the coarsest
resolution whose bilinear reconstruction stays within an imperceptible error of the full bake,
recovering VRAM where lighting is smooth or absent while leaving sharp static shadows and hard
spotlight edges pixel-crisp. The gating risk is whether the post-hoc downsample+repack survives the
cache-key/layout inversion; the size of the win is also unproven — the irradiance atlas is
BC6H-compact and may be a minor term beside the SH volume — so it is measured before shipping surface
is committed. The gate is net-positive recovery plus a clean inversion, not a magnitude threshold:
any improvement counts, since what content this engine will be asked to render is unknown. So this
lands in two phases: a build-to-learn gate first, the shippable coarsening only if the gate returns
green.

## Decisions
- **Two-phase, gated execution.** Phase 1 is build-to-learn: an output-preserving `--lightmap-analyze`
  pass + a one-leaf inversion proof, delivered as a findings note. Phase 2 (the coarsening repack,
  boundary grading, escape valve) builds only if the note shows a net-positive win (any improvement
  counts) and a clean inversion.
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
  bake stays under tolerance (rel_p95/rel_max, harmonized with the SH measured classifier;
  darkness/empty is the trivial-pass sub-case). Any sector with a sharp static-shadow terminator or hard spotlight
  cutoff fails the gate and stays full-res — the two failure modes the owner named are refused by
  construction. Layer: compiler, post-hoc over the finished dense bake; never a runtime change.
- **Downsample with a linear-space low-pass prefilter before decimation** (irradiance is linear HDR
  f32; the compiler's Mitchell-Netravali texture-mip downsample is a reuse candidate). Runtime
  reconstruction is the existing irradiance linear sampler — no runtime or format change.
- **Coarsening level set = a fixed power-of-two ladder to a floor.** Levels are 2× downsample steps
  from full-res (the 2× `downsample_2x_f32` filter) down to a defined coarsest floor — a fixed ladder
  like the SH L0/L1/L2 track, not a free minimum chart dimension; fewer candidate layouts keep the
  cache fingerprint simple and deterministic. The floor and step count are the Phase-1/2 tunable.
- **≤1-level boundary grading** between adjacent sectors (`sh_coarsen.rs::smooth_pair`).
- **Sector unit = per-BVH-leaf** — the grain at which atlas footprint is banked, since the packer
  sizes each shared layer per leaf. Per-chart is a finer alternative measured against per-leaf in
  Phase 1, not an executor choice.
- **No new content-facing surface this brief.** The `lightmap_protect_volume` FGD entity is dropped —
  the automatic gate protects measurable hardness by construction. A force-fine escape valve is built
  only if Phase 2 surfaces a near-threshold miss, and then as a CLI-only protect flag mirroring the
  shipped `--sh-protect-aabb` — not `_lightmap_scale`, which is a density lever the gate coarsens
  *from* and so cannot force a region fine, and not a new modder one-way door. The `_lightmap_scale`
  KVP and the post-hoc gate still **compose**: the author sets a region's starting density, the gate
  then coarsens from whatever that bake produced.
- **Measurement characterizes, never gates the concept.** `--lightmap-analyze` is output-preserving
  (measure-and-report); it sizes the win and shows where coarsening acts. It is never a user setting.
- **Non-goals:** true section-skip / a zero-static runtime path (coarsen-to-minimum reaches the empty
  case; a black atlas stays correct — the no-static placeholder is white and ungated, so a runtime
  zero-static path is deferred and riskier); occlusion-culling never-seen faces; a per-surface opt-out
  flag; the direction and shadowmask atlases (irradiance only — irradiance is the BC6H-compact term
under test and shares the SH gate's structure; the other atlases differ in format and frequency and
have their own track); denoise (analytic + deterministic bake).

## Acceptance

### Phase 1 — gate (committed)
Automated (honesty gates):
- [ ] `--lightmap-analyze` runs on `campaign-test`, `closet-reveal`, and a hard-spotlight fixture
  (fixture to author — no hard-spotlight map exists in `content/` today) and emits, per sector, the
  coarsest level passing the reconstruction-error gate, its error, and recovered texels/bytes.
- [ ] Output-preserving: emitted PRL bytes identical with and without the flag.
- [ ] One-leaf inversion proof (ordering pin R1): a full-bake→downsample-one-leaf→repack path, on a
  leaf that coarsens ≥1 level, yields a correct atlas whose cache key reflects the final
  (post-coarsen) layout AND differs from the pre-coarsen fingerprint, so a stale pre-coarsen entry is
  never reused; two `--no-cache` runs are byte-identical.

Manual (measured findings — gate the Phase 2 decision, not thresholds):
- [ ] Findings note (resource-bounds proof): recovered irradiance VRAM per map in MiB (BC6H at-rest
  id-22 irradiance blob) against the uncoarsened bake of the same map as baseline, from a `--release`
  cold bake, fixture set and machine class stated; AND its share of total lightmap + SH at-rest VRAM
  (the exact section-id denominator pinned before the run — reported for context, not a threshold);
  the inversion's cleanliness; and a promote / adjust / stop recommendation, where **stop means only
  a net regression (the change costs more than it saves) or an inversion that cannot be made clean** —
  any net-positive recovery, however small, promotes.

### Phase 2 — shippable coarsening (gated on the Phase 1 note)
Automated:
- [ ] Hard static shadow preserved AND surround coarsened: on a fixture with a sharp static-geometry
  terminator, the terminator sector(s) stay full-res while a smooth-lit region of the same scene
  coarsens ≥1 level.
- [ ] Hard spotlight edge preserved AND pool interior coarsened: the hard edge sector(s) stay
  full-res while the smooth pool interior / falloff surround of the same scene coarsens ≥1 level.
- [ ] Smooth sector coarsened (permit side): a soft-gradient lit region coarsens ≥1 level, error under tolerance.
- [ ] Empty collapse: an all-black atlas coarsens to the minimum coarsening level; sampled irradiance
  stays zero AND the atlas does not degrade to the white no-static placeholder.
- [ ] Determinism: on a fixture that coarsens ≥1 leaf, warm and cold bakes produce the identical
  coarsened atlas; two `--no-cache` runs byte-identical.
- [ ] ≤1-level grading: no two adjacent sectors differ by more than one level.
- [ ] Seam preserves the sharp side (ordering pin R3): on a fixture with a collapsed/dark leaf
  adjacent to a hard-feature leaf, the hard-feature leaf stays full-res and the ≤1-level bound holds
  by refining the dark neighbour, not the sharp leaf.
- [ ] Grading fixpoint resolves a chain (ordering pin R4): a run of coarsenable leaves next to a
  pinned full-res leaf converges so every adjacent pair differs ≤1 level, protection applied before
  smoothing, the fixpoint terminating.
- [ ] Gutter valid at reduced resolution (ordering pin R2): a coarsened chart keeps a ≥1-texel gutter
  of its own dilated irradiance after repack; edge texels show no cross-chart bleed.
- [ ] No-op when nothing coarsens (ordering pin R5): on an all-full-res map the two-stage repack
  produces a pre-BC6H atlas byte-identical to the uniform bake (encoded id-22 section round-trips
  within BC6H tolerance); the atlas layout fingerprint is unchanged.
- [ ] Prefilter necessity (ordering pin R6): a mid-frequency gradient fixture coarsens within
  tolerance with the linear-space prefilter, and a decimate-without-prefilter (or gamma-space) path
  exceeds tolerance / shifts mean irradiance.
- [ ] Scale-region compose: a leaf inside a `_lightmap_scale` region bakes at the region's density,
  then the gate coarsens from that scaled bake (composition, not override or re-scale).

Manual:
- [ ] Visual A/B (campaign-test, closet-reveal, hard-spotlight scene): no perceptible smudging of
  static shadows or spotlight edges at default tolerance; VRAM recovered recorded per map in MiB
  against the uncoarsened bake baseline, from a `--release` bake, machine class stated.

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
- Per-chart vs per-leaf recovery — **measured in Phase 1**: the analyze pass reports recoverable per grain; per-leaf is committed, per-chart earns its place only if it recovers materially more.
- Error-tolerance **default value** — **measured**: conservative start, A/B-tuned. (Metric is settled — rel_p95/rel_max, harmonized with SH; the A/B is the perceptual validation, not a rival metric.)
- Level count and coarsest floor on the fixed power-of-two ladder — **measured** in Phase 1/2.
- Force-fine escape valve — none unless Phase 2 surfaces a near-threshold miss, then the SH-style CLI protect flag.
