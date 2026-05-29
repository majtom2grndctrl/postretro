# SDF Per-Light Static-Light Shadows — Vertical Slice (perf gate)

## Goal

Make SDF-traced shadows reproduce real per-light cast shadows for static lights —
the shadows the baked lightmap produces for free today, but runtime-resolved so
lights are tweakable without a shadow re-bake and without a shadow map per light.
This slice replaces the per-texel **single dominant-direction** trace (which
structurally cannot cast a specific light's shadow — one luminance-weighted mean
ray misses the occluder) with **per-light** tracing: for each SDF-tagged static
light influencing a fragment, evaluate its diffuse contribution and multiply by an
SDF visibility ray traced toward *that* light.

Shadowing is a **per-light author choice** across three techs, each on its own
niche. **Baked** (lightmap, free) is the default and carries fixed fill lighting.
**SDF** (runtime-traced) gives a sparse set of static lights tweakable shadows with
no re-bake. **Dynamic** (the existing shadow-map path) handles spots, moving, and
hero lights. SDF earns its per-fragment march precisely where the other two can't:
runtime tweaks that baking can't follow, on more lights than shadow maps scale to.

This slice is a **perf gate**. It lands the real per-light path for a sparse SDF
light set on real authored content and measures it on the 2020 MBP. The full
redesign follows only if the gate holds: if the per-fragment march can't make budget
for the intended case (a handful of SDF lights, the rest baked/dynamic), the
fail-floor stands and SDF reverts. The perf measurement is the primary deliverable,
alongside a visibly correct cast shadow.

## Background

Confirmed end-to-end this session (see `research.md`): with a correct
`--unshadowed-lightmap --bake-sdf` PRL and SDF Shadow Mode On, the `occlusion-test`
block casts **no** shadow — not in normal rendering, not in DirectOnly, not even in
the raw factor Visualize — while the baked lightmap casts it for free. Root cause is
architectural: `trace_shadow(world, static_dir)` fires one ray toward a per-texel
mean direction; with several static lights that mean points at no single light, so
the occluder is never hit. Four prior SDF specs polished plumbing around this
shortcut without addressing it.

All v1-authored lights are static (`_dynamic` retired, `quake_map.rs:1207`;
`filter_dynamic_lights`, `render/mod.rs:3838`). Static lights ride a culled
per-fragment loop via `spec_lights` + `chunk_grid` (`forward.wgsl:707-731`) — today
specular-only. This slice extends that loop to diffuse + per-light SDF visibility for
SDF-tagged lights.

## Scope

### In scope

- **Per-light shadow-tech tag** `_shadow_tech ∈ {baked, sdf, dynamic}`, default
  `baked`, authored in `sdk/TrenchBroom/postretro.fgd`, parsed to a `MapLight` field,
  carried through the PRL light entry to the runtime.
- **Disjoint light sets by tag (no double-count).** The compiler excludes `sdf` and
  `dynamic` lights from the lightmap bake (extends the existing
  `!is_dynamic && animation.is_none()` filter at `lightmap_bake.rs:110`), so `lm_irr`
  holds *only* baked-tagged lights. `dynamic` maps to `is_dynamic = true` (revives the
  retired routing onto the existing shadow-map path). `sdf` stays static but is flagged
  in `spec_lights` for the runtime path.
- **Per-light diffuse for SDF-tagged lights** in the forward `spec_lights`/`chunk_grid`
  loop (currently specular-only), gated by the tag flag so baked-tagged lights keep
  getting diffuse from `lm_irr` (untouched).
- **Per-light SDF visibility:** a half-res pass traces one shadow ray per SDF-tagged
  light toward the **K most-influential** at each tile (K = the engine's per-fragment
  SDF shadow budget; seed K = 4, matching the author rule-of-thumb of ≤ 2 SDF shadows
  per surface with headroom), writing a K-slice half-res target. Forward upsamples and
  multiplies each SDF light's diffuse by its visibility. Beyond K overlapping SDF lights
  at a fragment, extras are dropped (treated lit) and the compiler warns.
- Reuse the fine SDF field (`sample_fine_distance`, `sdf_atlas`) and the established
  closest-passing-distance soft-shadow march, unchanged in math (open-space skip seeded
  loose — this session found the 2.5 default suppresses the trace).
- A perf measurement on the 2020 MBP (`occlusion-test`, `campaign-test`) with
  `POSTRETRO_GPU_TIMING=1`, against a stated budget, on the SDF-intended case.

### Out of scope

- Removing the baked lightmap. It stays as the cheap default tech; a level may opt out
  entirely by tagging every light `sdf`/`dynamic`, but the bake pipeline is retained.
- The shadow-map machinery itself (unchanged) and full dynamic-light authoring beyond
  the `dynamic` tag → `is_dynamic` mapping.
- The animated (intensity/color) static-light runtime migration — animated lights keep
  the current baked `lm_anim` path. Unifying them onto runtime per-light curves is the
  follow-on.
- Indirect lighting. The SH/DDGI volume is independent of the direct lightmap and is
  untouched — opting out of baked *direct* light still leaves indirect GI intact. **SDF
  never does indirect visibility.**
- Converting existing test-room content is a separate content change (see Open
  questions: the spot room should become `dynamic`, both to use the right tool and to
  keep it out of the SDF perf gate).
- Scaling K past the seed, and K-selection quality tuning past "nearest/brightest."

## Acceptance criteria

Automated (host-side / shader-compile / unit — deterministic):

- [ ] `_shadow_tech` parses from FGD to a `MapLight` tag with `baked` default; an
      unknown value errors at compile (mirrors `_cast_entity_shadows` parsing).
- [ ] Disjoint sets: a bake-level test asserts `sdf`- and `dynamic`-tagged lights are
      excluded from the lightmap bake's light set, and `baked`-tagged lights are
      included — so `lm_irr` and the runtime SDF set never share a light.
- [ ] `dynamic`-tagged lights resolve to `is_dynamic == true` (route onto the shadow-map
      path); `sdf`- and `baked`-tagged resolve to `is_dynamic == false`.
- [ ] K-light selection is deterministic and bounded: a host test asserts the per-tile
      selection returns at most K indices and prefers nearer/brighter lights for a fixed
      set; the per-tile (Task: visibility) and per-fragment (Task: forward) selection
      agree for the same cell.
- [ ] The static-light forward loop computes a diffuse term gated by the SDF tag; a
      shader naga-validation test confirms it compiles and reads the K-slice target.
- [ ] With SDF visibility forced to 1.0, total direct light equals the pre-change result
      within tolerance (guards the disjoint-set / no-double-count math).
- [ ] `cargo fmt` / `clippy` clean; full suite green.

Manual / visual (human running the engine — not machine-verified):

- [ ] In `occlusion-test` with the block's light tagged `sdf`, SDF Mode On: the block
      casts a visible solid shadow on the floor and the left wall, matching the
      baked-lightmap reference (this session's image #9) in placement, present in both
      Normal and DirectOnly isolation, tracking the occluder across camera angles.

Perf gate (human, measured — the primary deliverable):

- [ ] On the 2020 MBP at 1080p, with a *sparse* SDF light set (author rule: ≤ 2 SDF
      shadows per surface) and the rest baked/dynamic, frame time holds the stated
      budget on `occlusion-test` and `campaign-test`; per-pass `POSTRETRO_GPU_TIMING=1`
      numbers recorded in `research.md`. If it cannot be brought into budget by the
      bounded knobs (K, half-res, march steps, cull radius), the slice **reports the
      fail-floor** rather than masking it.

## Tasks

### Task 1: Per-light shadow-tech contract + disjoint bake sets

Add `_shadow_tech {baked|sdf|dynamic}` to the FGD and the `quake_map` parser (mirror
`_cast_entity_shadows`, `quake_map.rs:279`), default `baked`. Carry it as a `MapLight`
field (`prl.rs:137`) through the PRL light entry (Wire format) to the runtime. In the
compiler: map `dynamic → is_dynamic = true`; extend the lightmap bake's light filter
(`lightmap_bake.rs:110`) to also exclude `sdf`-tagged lights; warn when more than K
`sdf` lights overlap a baked texel's region. In `pack_spec_lights`
(`render/mod.rs:2133`): flag `sdf`-tagged lights so the forward loop knows which get
the runtime diffuse + visibility path.

### Task 2: Per-light visibility pass (K-slice half-res trace)

Restructure the half-res SDF pass (`SdfShadowPass`, `render/sdf_shadow.rs:120`;
`shaders/sdf_shadow.wgsl`) to trace **per-light**. Per half-res pixel: reconstruct
world position (existing), select up to K influential `sdf`-tagged lights from
`chunk_grid` (shared selection helper — see Rough sketch), trace one `trace_shadow`
ray toward each, write K visibility factors to a K-slice half-res target (replacing the
2-channel factor). Drop the dominant-direction atlas bindings (`static_lm_direction`,
`animated_lm_direction`) — the pass no longer reads a baked direction. March math
unchanged; open-space skip seeded loose.

### Task 3: Per-light diffuse + visibility in forward

Extend the static-light loop (`forward.wgsl:707-731`) to add a Lambert diffuse term for
`sdf`-tagged lights (gated by the Task 1 flag), multiply each by its upsampled K-slice
visibility, and sum into `static_direct`. `lm_irr` (baked-tagged lights) and `lm_anim`
are untouched — disjoint sets mean no re-weighting is needed. Bind the K-slice target
where `sdf_shadow_factor` (`@group(5) @binding(3)`) was; adjust `upsample_shadow_factor`
(`forward.wgsl:486`) to sample a slice.

### Task 4: Perf gate + fail-floor report

Run the perf gate on the 2020 MBP against the SDF-intended case, record per-pass GPU
timing in `research.md`, and state explicitly whether the slice holds budget — and if
not, what the bounded knobs recover and whether it trips the fail-floor.

## Sequencing

**Phase 1 (sequential):** Task 1 — the tag + disjoint sets are the contract every other
task rides.
**Phase 2 (sequential):** Task 2 — consumes the `spec_lights` SDF flags; produces the
K-slice target.
**Phase 3 (sequential):** Task 3 — consumes Task 2's K-slice target.
**Phase 4 (sequential):** Task 4 — measures the assembled path; gates the follow-on.

## Rough sketch

**Disjoint sets kill the double-count.** A light is baked into `lm_irr` *or* evaluated
at runtime (sdf) *or* shadow-mapped (dynamic) — never two. The bake already filters its
light set (`lightmap_bake.rs:110`); adding the `sdf`/`dynamic` exclusion makes `lm_irr`
and the runtime SDF set disjoint by construction, so the forward simply *adds* the SDF
lights' contribution with no re-weighting. The "SDF forced to 1.0 ⇒ baseline" AC guards
this.

**Author rule ↔ K budget.** "≤ 2 SDF shadows per surface" is the same number as the
per-fragment SDF trace budget. Seed K = 4 gives headroom; beyond K overlapping SDF
lights, extras drop (lit) and the compiler warns. The authoring guideline and the perf
knob are one value.

**Light-selection parity (load-bearing).** Task 2's per-tile K-selection and Task 3's
per-fragment use must pick the **same** SDF lights in the same order, or the visibility
slice won't line up with the diffuse term. Factor selection into one shared WGSL helper
both call, keyed on the `chunk_grid` cell.

**K-slice target.** Replace the `Rgba8Unorm` 2-channel `shadow_factor` with a half-res
K-slice target (texture array, or packed `RgbaXUnorm` for K ≤ 4 — decide in Task 2 by
what the bilateral upsample samples cheaply).

**Established technique.** The per-light ray-marched soft shadow against a distance
field is standard (UE distance-field shadows; closest-passing-distance penumbra). The
novelty is only the K-bounded per-tile selection + half-res amortization; the march is
the existing `trace_shadow`.

## Boundary inventory

| Name | FGD KVP | Rust (`MapLight`) | Wire (PRL light entry) | WGSL (`spec_lights`) |
|---|---|---|---|---|
| shadow tech | `_shadow_tech` = `baked`\|`sdf`\|`dynamic` (default `baked`) | `shadow_tech: ShadowTech` enum | `u8` 0=baked,1=sdf,2=dynamic | `color_and_pad.w` flag: 1.0 ⇒ sdf-tagged (else 0.0) |

`baked`/`dynamic` need no runtime `spec_lights` flag (baked → `lm_irr`; dynamic →
shadow-map path via `is_dynamic`). Only the `sdf` distinction must reach the forward
loop, so the currently-unused `color_and_pad.w` (`forward.wgsl:95`) carries it — no
buffer-size change.

## Wire format

The PRL light section's per-entry layout gains a `shadow_tech: u8` (0=baked, 1=sdf,
2=dynamic), little-endian like the section's existing integer fields. Confirm the exact
field position against the `postretro_level_format` light-section serialization in
Task 1 (append at the entry tail, mirroring how `casts_entity_shadows` is encoded).
Pre-release, this is an acceptable additive wire change; bump the section/format version
so stale caches invalidate. Empty/legacy entries decode as `baked`.

## Open questions

- **Perf topology if the gate is marginal.** Fallbacks: lower K, fewer march steps,
  tighter cull, or trace inline in forward (no K-slice storage). Decide from Task 4
  numbers.
- **Content: convert the spot test room to `dynamic`.** The room of spot lights is SDF's
  worst case and shadow maps' best (one frustum each, already implemented). Re-tag those
  lights `_shadow_tech dynamic` — both to use the right tool and to keep the room out of
  the SDF perf gate. Recommended content change, tracked separately from this slice's
  code.
- **Disposition of superseded specs.** This replaces the trace mechanism of
  `sdf-static-occluder-shadows`, `sdf-shadow-fine-atlas-trace` (in-progress; its fine
  field is *kept*, its dominant-direction trace *replaced*), and
  `sdf-shadow-unshadowed-direction` (draft; its per-light-visibility intent is subsumed —
  drop it). The lightmap-UV MRT (`sdf-shadow-lightmap-uv-prepass`) is **not** needed by
  per-light tracing (the trace keys on light position, not a baked per-texel direction) —
  confirm and retire that binding if unused. Rewrite `sdf_shadows.md` to the per-light /
  hybrid model at promotion.
- **Indirect dominance.** The shadow modulates only the direct term; indirect (SH/DDGI)
  is unmodulated, so in indirect-dominated views the shadow stays subtle by design. Note
  for the follow-on's visual ACs.
