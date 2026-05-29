# SDF Per-Light Static-Light Shadows — Vertical Slice (perf gate)

## Goal

Make SDF-traced shadows reproduce real per-light cast shadows for static lights —
the shadows the baked lightmap produces for free today, but runtime-resolved so
lights are tweakable without a shadow re-bake and without a shadow map per light.
This slice replaces the per-texel **single dominant-direction** trace (which
structurally cannot cast a specific light's shadow — one luminance-weighted mean
ray misses the occluder) with **per-light** tracing: for each influential static
light, evaluate its diffuse contribution and multiply by an SDF visibility ray
traced toward *that* light.

This is a **vertical slice and a perf gate**, not the full redesign. It lands the
real per-light path for a bounded light count, measured on the 2020 MBP target. If
the per-fragment march cost can't hold frame budget here, the fail-floor stands and
SDF reverts to baked shadows — so the perf measurement is the primary deliverable,
alongside a visibly-correct cast shadow.

## Background

Confirmed end-to-end this session (see `research.md`): with a correct
`--unshadowed-lightmap --bake-sdf` PRL and SDF Shadow Mode On, the block in
`occlusion-test` casts **no** shadow — not in normal rendering, not in DirectOnly,
not even in the raw factor Visualize. The baked lightmap (`main`'s path) casts a
clean solid shadow for free. Root cause is architectural: `trace_shadow(world,
static_dir)` fires one ray toward a per-texel mean direction; with several static
lights, that mean points at no single light, so the occluder is never hit. The
four prior SDF specs polished plumbing around this shortcut without addressing it.

All v1-authored lights are static (`is_dynamic == false`, `filter_dynamic_lights`
at `render/mod.rs:3838`), so the forward dynamic-light loop is empty in v1 content.
Static lights already ride a culled per-fragment loop via `spec_lights` +
`chunk_grid` (`forward.wgsl:707-731`) — today specular-only. This slice extends
that loop to diffuse + per-light SDF visibility.

## Scope

### In scope

- Per-light diffuse for static lights in the forward shader, computed in the
  existing `spec_lights` / `chunk_grid` loop (currently specular-only).
- Per-light SDF visibility: a half-res pass traces one shadow ray per light toward
  the **K most-influential** static lights at each tile (K bounded, seed K = 4),
  selected from the same `chunk_grid` cull, writing a K-slice half-res visibility
  target. Forward upsamples and multiplies each light's diffuse by its visibility.
- Keep the baked `lm_irr` as a **fallback** term for lights beyond K / beyond the
  cull, scaled so total direct light does not double-count (see Rough sketch). This
  bounds the slice's blast radius — no bake-pipeline removal yet.
- Reuse the fine SDF field (`sample_fine_distance`, `sdf_atlas`) and the
  established closest-passing-distance soft-shadow march, unchanged in math.
- A perf measurement on the 2020 MBP target (`occlusion-test`, `campaign-test`),
  with `POSTRETRO_GPU_TIMING=1` per-pass numbers, against a stated budget.

### Out of scope

- Removing the baked direct lightmap (`lm_irr`) and the animated weight-map bake.
  Deferred to a follow-on **gated on this slice's perf result** — only worth doing
  if the slice proves viable.
- The animated (intensity/color) static-light runtime migration. Animated lights
  keep their current baked `lm_anim` path in the slice; unifying them onto runtime
  per-light curves is the follow-on.
- Dynamic (enemy / moving) lights and shadow maps. Untouched; SDF stays on static
  occluders + static lights. **SDF never does indirect visibility** — indirect
  stays on DDGI/SH.
- Scaling K beyond the seed bound, and any K-selection quality tuning past
  "nearest/brightest first."
- Wire-format / PRL section changes. `spec_lights` may gain a field (see Boundary)
  but that is a runtime GPU buffer, not a PRL section.

## Acceptance criteria

Automated (host-side / shader-compile / unit — deterministic):

- [ ] The static-light forward loop computes a diffuse term (not specular-only):
      a unit test on the CPU-side packing asserts every static light carries the
      fields diffuse needs (position, range, color × intensity), and a shader
      naga-validation test confirms the per-light diffuse + visibility multiply
      compiles and reads the K-slice visibility target.
- [ ] K-light selection is deterministic and bounded: a host test on the
      tile→light selection asserts it returns at most K indices and prefers
      nearer/brighter lights given a fixed light set.
- [ ] With the SDF factor forced to 1.0 (visibility disabled), runtime per-light
      diffuse + `lm_irr` fallback reproduces the pre-change lit result within a
      tolerance (no brightness regression from the term reshuffle) — guards the
      double-count math.
- [ ] `cargo fmt` / `clippy` clean; full suite green.

Manual / visual (human running the engine — not machine-verified):

- [ ] In `occlusion-test`, `--unshadowed-lightmap --bake-sdf`, SDF Mode On: the
      block casts a visible solid shadow on the floor and the left wall, matching
      the baked-lightmap reference (this session's image #9) in placement.
- [ ] The shadow tracks the occluder from multiple camera angles and is present in
      both Normal and DirectOnly lighting isolation.

Perf gate (human, measured — the primary deliverable):

- [ ] Frame time on the 2020 MBP target holds the stated budget at K = 4 on
      `occlusion-test` and `campaign-test` at 1080p, with `POSTRETRO_GPU_TIMING=1`
      per-pass numbers recorded in `research.md`. If it does not hold and cannot
      be brought into budget by the bounded knobs (K, half-res, march steps,
      cull radius), the slice **reports the fail-floor** rather than masking it.

## Tasks

### Task 1: Per-light visibility pass (K-slice half-res trace)

Restructure the half-res SDF pass (`SdfShadowPass`, `render/sdf_shadow.rs:120`;
`shaders/sdf_shadow.wgsl`) to trace **per-light**. For each half-res pixel:
reconstruct world position (existing), select up to K influential static lights
from `chunk_grid` (mirror the forward cull at `forward.wgsl:707-731`), trace one
`trace_shadow` ray toward each selected light's position, and write K visibility
factors to a K-slice half-res target (replacing the 2-channel
static/animated factor). Drop the dominant-direction atlases (`static_lm_direction`,
`animated_lm_direction`) and their bindings — this pass no longer reads a baked
direction. The march math (`sample_fine_distance`, closest-passing penumbra,
open-space early-out) is unchanged; the open-space skip default is revisited only
if it suppresses the trace (this session found it does — seed it loose).

### Task 2: Per-light diffuse + visibility in forward

Extend the static-light loop (`forward.wgsl:707-731`) to add a Lambert diffuse term
per light (it currently computes only `blinn_phong` specular), multiply each
light's diffuse by its upsampled K-slice visibility, and sum into `static_direct`.
Re-weight `lm_irr` to a fallback role for lights not in the K set / beyond cull so
direct light is neither lost nor double-counted (Rough sketch). `lm_anim` and the
animated path are unchanged. Bind the K-slice visibility target where the old
`sdf_shadow_factor` (`@group(5) @binding(3)`) was; adjust `upsample_shadow_factor`
to sample a slice.

### Task 3: Perf measurement + fail-floor report

Run the perf gate (above) on the 2020 MBP, record per-pass GPU timing in
`research.md`, and state explicitly whether the slice holds budget at K = 4 — and
if not, what the bounded knobs can recover, and whether the result trips the
fail-floor.

## Sequencing

**Phase 1 (sequential):** Task 1 — produces the K-slice visibility target the
forward consumes.
**Phase 2 (sequential):** Task 2 — consumes Task 1's target; reshapes the forward
direct term.
**Phase 3 (sequential):** Task 3 — measures the assembled path; gates the follow-on
redesign.

## Rough sketch

**Double-count avoidance (Task 2).** Today `static_direct = lm_irr * scale *
static_sdf + lm_anim * animated_sdf` (`forward.wgsl:675`). `lm_irr` is the baked
*sum* of all static lights' diffuse. The slice computes runtime diffuse for the K
selected lights and must not also count them in `lm_irr`. Cleanest correct seam for
the slice: the K selected lights contribute `runtime_diffuse_k * visibility_k`, and
`lm_irr`'s contribution is suppressed for those lights. Since `lm_irr` can't be
un-summed per light at runtime, the slice's honest options are (a) when K covers all
influential lights at a fragment, drop `lm_irr` entirely there and rely on runtime;
(b) keep `lm_irr` only where the fragment's influential-light count exceeds K. Pick
(a) for the slice with K sized to cover `occlusion-test`'s local light count;
validate via the "SDF forced to 1.0 ⇒ no brightness regression" AC. Document the
(b) generalization as the follow-on's job.

**K-slice target.** Replace the `Rgba8Unorm` 2-channel `shadow_factor` with a
half-res `R8Unorm` texture array of K slices (or a packed `RgbaXUnorm` for K ≤ 4 —
decide in Task 1 by what `upsample_shadow_factor`'s bilateral filter can sample
cheaply). Each slice = one selected light's visibility.

**Light selection parity.** Task 1's per-tile K-selection and Task 2's per-fragment
cull must select the **same** lights in the same order, or the visibility slice
won't line up with the diffuse term. Factor the selection into one shared WGSL
helper both call, keyed on the `chunk_grid` cell. This is the load-bearing
correctness seam.

**Established technique.** The per-light ray-marched soft shadow against a distance
field is standard (UE distance-field shadows; closest-passing-distance penumbra).
The novelty here is only the K-bounded per-tile selection + half-res amortization;
the march itself is the existing `trace_shadow`.

## Boundary inventory

| Name | Rust (`pack_spec_lights`, `render/mod.rs:2133`) | WGSL (`SpecLight`, `forward.wgsl:93`) |
|---|---|---|
| static light position + range | `position_and_range: [f32;4]` | `position_and_range: vec4<f32>` |
| static light color × intensity | `color_and_pad: [f32;4]` | `color_and_pad: vec4<f32>` (w currently 0) |

The slice's diffuse needs position, range, and color × intensity — all already
packed. If a falloff *model* selector is needed for diffuse parity with the baker,
it rides the currently-unused `color_and_pad.w` (no buffer-size change). Confirm
against the baker's diffuse falloff before assuming linear `1 - d/range` suffices.

## Open questions

- **Perf topology if the gate is marginal.** If half-res K-slice tracing is close
  to budget, the fallbacks are: lower K, lower march steps, tighter cull radius, or
  trace in forward inline (no separate pass) to skip the K-slice storage. Decide
  from Task 3's numbers, not up front.
- **K sizing vs `occlusion-test` / `campaign-test` local light density.** Seed K =
  4; confirm it covers the worst local count in the test maps, else option (b)
  fallback math is needed earlier than the follow-on.
- **Disposition of the superseded specs.** This slice replaces the trace mechanism
  of `sdf-static-occluder-shadows`, `sdf-shadow-fine-atlas-trace` (in-progress,
  uncommitted), and `sdf-shadow-unshadowed-direction` (draft). On this slice
  landing: mark `sdf-shadow-fine-atlas-trace` superseded (its fine-field sampler is
  *kept*, its dominant-direction trace is *replaced*), drop the
  `sdf-shadow-unshadowed-direction` draft (its intent — per-light visibility — is
  subsumed), and rewrite `sdf_shadows.md`'s dominant-direction framing to the
  per-light model at promotion. `sdf-shadow-lightmap-uv-prepass`'s lightmap-UV MRT
  is **not** needed by per-light tracing (the trace keys on light position, not a
  baked per-texel direction) — confirm and retire that binding if unused.
- **Indirect dominance.** Even correct, the shadow modulates only the direct term;
  indirect (SH/DDGI) is added unmodulated. In indirect-dominated views the shadow
  stays subtle by design. Not this slice's problem, but note for the follow-on's
  visual ACs.
