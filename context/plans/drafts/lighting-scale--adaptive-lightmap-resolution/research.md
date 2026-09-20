# research — adaptive-lightmap-resolution

Derivation and grounding behind the brief. Informs; the brief decides. Read at a097035.

## The principle

Spend lightmap resolution proportional to the sharpest feature a sector contains. A sector with
only soft gradients is band-limited — a low-pass prefilter + decimation + the runtime's existing
bilinear reconstruction reproduce it within a small error. A sector with a sharp static-shadow
terminator or a hard spotlight cutoff carries high frequency; downsampling it aliases/smudges, so
the reconstruction-error gate keeps it full-res. Darkness/empty is the trivial sub-case (zero
signal → maximally band-limited → coarsens hardest; an all-black atlas collapses to the minimum
and stays black).

This is the SH `classify_levels` structure ported to lightmaps: pick the coarsest level whose
reconstruction stays under tolerance. It is deliberately a *reconstruction-error / representation-
efficiency* gate, not a design-intent proxy.

## Why this supersedes the prior objection

`plans/done/lighting-scale--lightmap-bake-scaling` rejected a measured error/contrast classifier
for lightmap density, arguing the coarsening target was un-measurable gameplay intent ("the player
never approaches this surface"). The owner has withdrawn that premise: the driving goal is now
near-imperceptible VRAM reduction for low-end GPUs (observed: the engine at low frame rate on a
GTX 1660), and the gate is a band-limiting tool, not an intent proxy. The two things the owner will
not accept — a smudged static shadow, a blurred hard spotlight edge the author wanted crisp — are
exactly the high-frequency cases the gate refuses by construction, with the force-fine hatch as the
backstop for a near-threshold edge. `/validate-plan` should scrutinize the reopen against the prior
commitment; it stands on the new premise.

## SH precedent (the reused gate structure)

- Error gate: `sh_coarsen.rs::CoarsenParams` — `rel_p95_max = 0.10`, `rel_max_max = 0.25`, relative
  to a map-wide magnitude, with a darkness floor (`darkness_frac = 0.02 × map_p95`, min 1e-6) that
  bypasses the error comparison for near-black sectors.
- Boundary: `smooth_pair`/`demote_one` fixpoint enforces a ≤1-level bound so a coarse sector never
  abuts a fine one abruptly; protection/ceilings apply before smoothing.
- Escape hatch: `sh_protect_volume` brush entity + `--sh-protect-aabb` hard-pin overlapping regions
  to finest, union-composed (`combined_protect_aabbs`). The lightmap hatch mirrors this exactly.
- Analysis pass: `--sh-analyze` is output-preserving (summary + JSON, no byte change) — the model
  for `--lightmap-analyze`.

## Determinism / cache

Density from a lighting pre-pass must be deterministic and identical warm/cold or it breaks the
pre-BC6H byte-identity invariant. The safe pattern is post-hoc over the finished dense bake. The
architectural wrinkle: atlas layout is decided pre-bake in `prepare_atlas` (shared warm/cold), but
the coarsening decision needs the baked values — so coarsening is a SECOND stage (bake full →
classify → downsample + repack), and the cache fingerprint (`atlas_layout_fingerprint` /
`section_input_hash`, `lightmap_layer.rs`) must reflect the post-coarsen layout, not the pre-bake
one. This is the riskiest assumption; the SH work handled an analogous post-classification
restructuring (compaction) with explicit ordering pins.

## Measurement seam and reuse

Honest seam = post-dilate `CompositedAtlas` (raw f32 irradiance + `coverage` mask) in the live path
`pipeline/lightmap_stage.rs::bake_fused_prepared`. Runtime already samples irradiance through a
linear (bilinear) sampler, so reconstruction of a downsampled band-limited chart is free. The
compiler already has a linear-space Mitchell-Netravali downsample (texture-mip path) — a reuse
candidate, semantics to confirm. Charts are 1:1 with faces; the packer (`pack_layers`,
`choose_layer_dim`, MaxRects) already handles heterogeneous chart sizes, so smaller charts repack
with no packer change.

## Direction review outcome (reshape)

`/validate-plan` returned *Not a spec (yet)* → owner took the **Reshape**: the mechanism, placement,
and supersession are sound and precedented, but the artifact over-built ahead of the measurement the
repo's own method runs first (`sh-probe-density-coarsenability-spike` gated `sh-adaptive-coarsening-v2`
on a findings note). Two gaps drove it: the payoff is unquantified, and — the sharper point — the
irradiance atlas may be a **minor VRAM term**. Floor numbers the reviewer cited: full stress-warren
irradiance ≈ 9.4 MB BC6H, while the SH composed atlas ≈ 55.9 MB and streams ≈ 2.4 GiB/frame — so the
term binding a 1660 may live in a different subsystem. Phase 1 must therefore report recovered
irradiance VRAM *in context of total lightmap + SH VRAM*, so the owner can judge materiality before
committing any shipping surface. The FGD `lightmap_protect_volume` entity was dropped as premature.

## Signal taxonomy (context for scope)

- Reconstruction-error gate (this brief) — the unifying signal; darkness/empty are sub-cases.
- Force-fine escape valve — DROPPED as a new FGD entity; if Phase 2 needs one it reuses the shipped
  `_lightmap_scale` lever or a CLI-only protect flag, not a new content surface.
- Per-surface material opt-out flag — not built; `BrushSide` has no surface flags. Cheap future add.
- Occlusion-cull never-seen faces — not built; bake already culls solid-facing (`face_extract.rs`)
  and sealed-exterior (`find_exterior_leaves`) leaves, but not occluded interior faces. Larger,
  riskier (a false cull is a black surface).
- True section-skip / zero-static runtime path — deferred. The runtime static-direct term is added
  unconditionally when its light-term bit is set, and the no-static-lights placeholder is WHITE
  (full light), with no present-gate wired today (forward.wgsl static_direct; the `present` flag and
  white placeholder are documented-unused fields in `lighting/lightmap.rs`). So dropping a black atlas to the placeholder would brighten it — coarsen-to-
  minimum avoids that entirely and reaches the same footprint end state.

## Illustrative measurements (NOT a gate)

Throwaway instrumentation over the pre-BC6H `CompositedAtlas`, default 0.04 m/texel, debug build.
Current content is exploratory (test fixtures / owner's in-progress maps), not indicative of a real
game's direction — these characterize where the feature acts, they do not justify it.

| Map | Coverage | map_p95 | median lum | Dark-flat share | Est. recoverable |
|---|---|---|---|---|---|
| gate-heavily-lit (lit edge) | 0.55 | 0.903 | 0.585 | 0.55% | 0.2% |
| campaign-test (representative) | 0.80 | 0.181 | 0.000 | 75.8% | ~46% |
| kinematic-platform (mover) | 0.67 | 0.331 | 0.000 | 89.6% | ~45% |
| closet-reveal (theatrical) | 0.73 | 0.000 | 0.000 | 100% | ~55% |

The lit-edge map coarsens almost nothing (metric refuses false positives); dark/soft maps recover a
large fraction. Recoverable % is optimistic (ignores chart padding, leaf-cohesion repack; dilation
inflates coverage). Static direct is often near-black here because the engine leans on SH indirect +
dynamic light — expected, and a reason the feature must behave across the whole spectrum, not just
this content.

## Ordering pins

Proof orderings the Acceptance rows cite by id. Added by `/review-brief` (rows lens); the brief's
Acceptance rows reference these by id, not by location.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| R1 | full-res dense bake finished, one leaf classifies coarsenable | classify runs on the composited post-dilate `CompositedAtlas` → per-leaf level chosen → downsample+repack → `atlas_layout_fingerprint` recomputed over the POST-coarsen `SharedAtlas` | the layer/section cache keys hash the post-coarsen chart extents + placements; a pre-coarsen fingerprint is never written or read |
| R2 | a leaf coarsened ≥1 level, adjacent to a different chart in the same atlas layer | downsample-then-dilate (re-establish ≥ `CHART_PADDING_TEXELS` gutter at reduced resolution) vs dilate-then-downsample | coarsened chart's edge/gutter texels carry only its own irradiance; a bilinear sample at the chart edge shows no neighbour-chart bleed |
| R3 | a leaf at the darkness floor / collapsed-to-minimum, face-adjacent to a leaf holding a sharp shadow terminator or hard spotlight edge (kept full-res) | protection/full-res pin applied before the ≤1-level fixpoint; the fixpoint refines the coarse/dark endpoint toward the pinned full-res one, never coarsens the pinned one | the sharp leaf stays full-res AND the ≤1-level bound holds — the dark neighbour is graded up, not the sharp leaf down |
| R4 | a row of leaves gating to (…,L2,L2,L2,L0) with one full-res pin | protection/pin before a monotone (levels only decrease) fixpoint sweep repeated to zero demotions; the coarser endpoint of any ≥2-gap pair demotes one step | fixpoint terminates; no adjacent pair differs by >1 level; a leaf demoted only by the seam is still a gate-valid representation |
| R5 | a leaf/map where every leaf classifies full-res (nothing coarsenable) | classify → every leaf at the finest level → downsample is identity → repack | the repacked pre-BC6H atlas is byte-identical to the current uniform-density bake, the encoded id-22 section round-trips within BC6H tolerance, and the atlas layout fingerprint is unchanged |
| R6 | a leaf carrying a mid-frequency band-limited gradient that coarsens ≥1 level | linear-space low-pass prefilter applied before decimation (not decimate-then-filter, not gamma-space) | emitted coarsened chart reconstructs the gradient within tolerance with no aliasing/banding; mean irradiance preserved (no gamma-space darkening) |
