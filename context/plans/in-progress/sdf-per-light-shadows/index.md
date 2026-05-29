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
  `!is_dynamic && animation.is_none()` filter at `light_namespaces.rs:61`
  (`StaticBakedLights::from_lights`)), so `lm_irr`
  holds *only* baked-tagged lights. `dynamic` maps to `is_dynamic = true` (revives the
  retired routing onto the existing shadow-map path). `sdf` stays static but is flagged
  in `spec_lights` for the runtime path.
- **Per-light diffuse for SDF-tagged lights** in the forward `spec_lights`/`chunk_grid`
  loop (currently specular-only), gated by the tag flag so baked-tagged lights keep
  getting diffuse from `lm_irr` (untouched).
- **Baked-tag static lights use the shadowed bake.** With `sdf`/`dynamic` lights out of
  `lm_irr` and the static dominant-direction trace removed, a baked-tag light's shadow can
  only come from the bake — so these lights bake *shadowed* (the standard bake captures
  hard static shadows), not `--unshadowed`. Animated lights are unaffected: they keep the
  `lm_anim` path and their existing SDF shadow factor (the animated dominant-direction
  trace is retained, see Task 2).
- **Per-light SDF visibility:** a half-res pass traces one shadow ray per SDF-tagged
  light toward the **K most-influential** at each tile (K = the engine's per-fragment
  SDF shadow budget; seed K = 3 — the animated shadow factor retains one channel of the
  4-channel target, leaving three for per-light slices, see Rough sketch — which still
  clears the author rule-of-thumb of ≤ 2 SDF shadows per surface with headroom),
  writing a K-slice half-res target. Forward upsamples and
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
  never does indirect visibility.** *Known consequence:* the shadow modulates only the
  direct term; indirect (SH/DDGI) stays unmodulated by that invariant, so in
  indirect-dominated views the shadow is subtle by design. Flag for the follow-on's visual
  ACs — not a slice decision.
- Converting existing test-room content is a separate content change, tracked separately.
  The spot test room becomes `dynamic`: spots are SDF's worst case and shadow maps' best
  (one frustum each, already implemented), so the "right tool per light" invariant settles
  it. Re-tagging those lights `_shadow_tech dynamic` also keeps the room out of the SDF perf
  gate. Decided, not an open question.
- **`campaign-test.map` animated-spot room → `dynamic` (perf-gate prerequisite).** One room
  holds several animated spot lights whose placement would tank perf as SDF lights. They
  need scripted (runtime) animation, which the baked `lm_anim` path does not yet support, so
  they cannot stay baked-animated either. Re-tag them `_shadow_tech dynamic`: dynamic lights
  get pixel (shadow-map) shadows and accept runtime animation. The disjoint-set filter (Task
  1) then drops them from `lm_anim`. This content edit must land **before** the `campaign-test`
  perf measurement (Task 4), or the gate measures a polluted SDF set. Content change, tracked
  separately; the code dependency (animated-bake exclusion) is in scope.
- Scaling K past the seed, and K-selection quality tuning past "nearest/brightest."

## Acceptance criteria

Automated (host-side / shader-compile / unit — deterministic):

- [ ] `_shadow_tech` parses from FGD to a `MapLight` tag with `baked` default; an
      unknown value errors at compile (mirrors `_cast_entity_shadows` parsing).
- [ ] Disjoint sets: a bake-level test asserts `sdf`- and `dynamic`-tagged lights are
      excluded from **both** bake sets (`lm_irr` static and `lm_anim` animated), and
      `baked`-tagged lights are included — so neither baked atlas ever shares a light with
      the runtime SDF set.
- [ ] `dynamic`-tagged lights resolve to `is_dynamic == true` (route onto the shadow-map
      path); `sdf`- and `baked`-tagged resolve to `is_dynamic == false`.
- [ ] K-light selection is deterministic and bounded: a headless GPU-readback compute
      test (the `curve_eval_test.rs` precedent — dispatch the WGSL selection helper, read
      back the chosen indices, self-skip when no adapter) asserts the shared helper returns
      at most K indices and matches a Rust reference comparator's pinned total order for
      fixed light sets. Visibility-pass ↔ forward parity is **by construction** — both call
      the same verified helper — not separately tested. (The comparator is pinned to a single
      total order — influence descending, light index ascending; see Rough sketch.)
- [ ] The static-light forward loop computes a diffuse term gated by the SDF tag; a
      shader naga-validation test confirms it compiles and reads the K-slice target.
- [ ] The compiler emits a warning when more than K `sdf` lights cover a `chunk_grid` cell
      (host test).
- [ ] `cargo fmt` / `clippy` clean; full suite green.

Manual / visual (human running the engine — not machine-verified):

- [ ] In `occlusion-test` with the block's light tagged `sdf`, SDF Mode On: the block
      casts a visible solid shadow on the floor and the left wall, matching the
      baked-lightmap reference (this session's image #9) in placement, present in both
      Normal and DirectOnly isolation, tracking the occluder across camera angles.
- [ ] With SDF visibility forced to 1.0 (dev toggle), the scene matches the pre-change
      render — no brightening from double-counted light. The underlying no-double-count
      *math* is machine-guarded by the disjoint-set test above; this is the visual
      integration check, since there is no full-frame render-readback harness (only the
      isolated-compute precedent).

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
`_cast_entity_shadows`, `quake_map.rs:279`), default `baked`. Carry it as a `shadow_tech`
field on the **compiler-side** `MapLight` (`map_data.rs:190`, the struct that has
`animation: Option<LightAnimation>` at `:218` and is read by the bake filter) AND thread
it separately through the **runtime** `MapLight` (`prl.rs:137`, which has no `animation`
field — it carries `animated_slot: Option<u32>` instead). In the compiler: map `dynamic
→ is_dynamic = true`; extend **both** bake light filters
(`StaticBakedLights::from_lights`, `light_namespaces.rs:61`, feeding `lm_irr`; and
`AnimatedBakedLights::from_lights`, `:102`, feeding `lm_anim`) to exclude `sdf`- and
`dynamic`-tagged lights — so a `dynamic`-tagged *animated* light (e.g. the migrated
campaign-test spots) actually leaves `lm_anim` and routes to the shadow-map path; warn
where the runtime would drop a light — count `sdf` lights whose
influence covers a `chunk_grid` cell and warn when a cell exceeds K (the warning's unit is
the `chunk_grid` cell, mirroring the runtime K-selection; `sdf` lights are excluded from the
bake, so a "baked texel" is the wrong frame); bump the lightmap stage's `STAGE_VERSION`
so stale cache entries don't serve an old lightmap. In `pack_spec_lights` (`spec_buffer.rs`): flag `sdf`-tagged lights so the
forward loop knows which get the runtime diffuse + visibility path.

### Task 2: Per-light visibility pass (K-slice half-res trace)

Restructure the half-res SDF pass (`SdfShadowPass`, `render/sdf_shadow.rs:120`;
`shaders/sdf_shadow.wgsl`) to trace **per-light**. Per half-res pixel: reconstruct
world position (existing), select up to K influential `sdf`-tagged lights from
`chunk_grid` (shared selection helper — see Rough sketch), trace one `trace_shadow`
ray toward each, write K visibility factors to the existing 4-channel `Rgba8Unorm` shadow
factor (currently R=static shadow, G=animated shadow, B/A reserved). The **G channel stays
the animated shadow factor** (the animated path is unchanged); the static dominant-direction
trace is removed, freeing R, so the K per-light slices pack into R/B/A — seed K=3, see Rough
sketch. Drop only the **static** dominant-direction binding (`static_lm_direction`); **keep
`animated_lm_direction`** — the retained animated trace still reads it. The per-light static
trace keys on light position; the animated trace keeps its baked-direction read. March math
unchanged; open-space skip seeded loose. Register the new half-res SDF visibility pass in the
`POSTRETRO_GPU_TIMING` pass set so the perf gate can read its per-pass time.

Keep the full-res lightmap-UV gbuffer MRT in the depth pre-pass (the `lightmap_uv_view` slot
this pass consumes, `sdf_shadow.rs:152`/`:156`): the retained animated dominant-direction
trace still indexes the per-texel `animated_lm_direction` atlas through it, so it is **not**
dead. (It would only become removable once animated lights migrate off the baked trace — the
follow-on.)

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
light set (`light_namespaces.rs:61`, `StaticBakedLights::from_lights`); adding the
`sdf`/`dynamic` exclusion makes `lm_irr`
and the runtime SDF set disjoint by construction, so the forward simply *adds* the SDF
lights' contribution with no re-weighting. The "SDF forced to 1.0 ⇒ baseline" AC guards
this.

**Author rule ↔ K budget.** "≤ 2 SDF shadows per surface" is the per-fragment SDF trace
budget. Seed K = 3 (the animated factor keeps one of the four channels, leaving three —
see K-slice target below) gives headroom of one over the ≤2 rule; beyond K overlapping SDF
lights, extras drop (lit) and the compiler warns. The authoring guideline and the perf knob
are one value.

**Light-selection parity (load-bearing).** Task 2's per-tile K-selection and Task 3's
per-fragment use must pick the **same** SDF lights in the same order, or the visibility
slice won't line up with the diffuse term. Factor selection into one shared WGSL helper
both call, keyed on the `chunk_grid` cell. The helper's contract pins a single
deterministic total order: sort selected `sdf` lights by influence descending (intensity
attenuated by distance — reuse the engine's existing per-light influence metric, do not
invent a new one), tie-break by light index ascending. "By construction" parity holds
only if the helper is **literally one source string** concatenated into both the
visibility pass and the forward shader at pipeline creation (the §8 shared-WGSL-helper
pattern — declare no buffers, consumers bind their own) — not copy-pasted text, which can
drift silently. The GPU-readback AC verifies that single source; two copies would defeat
both the test and the claim.

**K-slice target.** Repurpose the existing 4-channel `Rgba8Unorm` `shadow_factor`
(currently R=static shadow, G=animated shadow, B/A reserved; defined as
`SHADOW_FACTOR_FORMAT` at `sdf_shadow.rs:32`, at `@group(5) @binding(3)`) in place as
the K-slice target. The constraints converge: group 5 has no spare binding and K>3 is out
of scope, so the per-light slices live in this target. The **G channel stays the animated
shadow factor** (animated path unchanged); removing the static dominant-direction trace
frees R, so the three channels R/B/A carry the K=3 per-light slices — no texture array, no
new binding, no shared-BGL ripple across forward/billboard/fog. The implementer assigns
which channel holds which slice when they land Task 2. K>3 would need a separate target
(texture array), which is out of scope.

**Established technique.** The per-light ray-marched soft shadow against a distance
field is standard (UE distance-field shadows; closest-passing-distance penumbra). The
novelty is only the K-bounded per-tile selection + half-res amortization; the march is
the existing `trace_shadow`.

## Boundary inventory

| Name | FGD KVP | Rust (`MapLight`) | Wire (PRL light entry) | WGSL (`spec_lights`) |
|---|---|---|---|---|
| shadow tech | `_shadow_tech` = `baked`\|`sdf`\|`dynamic` (default `baked`) | `shadow_tech: ShadowTech` enum | `u8` 0=baked,1=sdf,2=dynamic | `color_and_pad.w` flag: encoded 1.0/0.0; decode `w > 0.5` ⇒ sdf-tagged |

`baked`/`dynamic` need no runtime `spec_lights` flag (baked → `lm_irr`; dynamic →
shadow-map path via `is_dynamic`). Only the `sdf` distinction must reach the forward
loop, so the currently-unused `color_and_pad.w` (`forward.wgsl:95`) carries it — no
buffer-size change.

## Wire format

The PRL light section (`AlphaLightsSection`, section ID 18,
`crates/level-format/src/alpha_lights.rs`) gains a `shadow_tech: u8` (0=baked, 1=sdf,
2=dynamic) appended at the record tail in `to_bytes` (`:146`), mirroring how
`casts_entity_shadows` is encoded.

The section today has **no version field** — backward compatibility rides record-stride
detection (`ALPHA_LIGHT_RECORD_SIZE = 73` at `:104` vs `ALPHA_LIGHT_RECORD_SIZE_LEGACY =
72` at `:109`, disambiguated at `:171`). Rather than extend that to a third stride (74) —
a collision-prone heuristic the moment a future field lands a same-length record — **add a
section-internal version field** to `AlphaLightsSection`, mirroring the `SH_VOLUME_VERSION`
precedent (a per-section version, distinct from the PRL header `CURRENT_VERSION = 4`,
bumped when the record layout changes so the loader rejects stale `.prl` with a clear
error). Bump it for this change; the deserializer reads it and decodes `shadow_tech` only
when present, defaulting older records to `baked`. Pre-release, recompiling maps from
`.map` source is the expected path (the build cache invalidates on the stage version), so
a clean version field beats carrying the stride heuristic forward. Retire the 72/73 stride
detection in the same pass if the version field subsumes it.

## Open questions

- **Perf topology if the gate is marginal — the measurement is the irreducible open
  item.** The bounded knobs (K, half-res scale, march steps, cull radius) are the gate's
  tuning envelope and stay open pending Task 4 numbers. Note: tracing inline in forward (no
  K-slice storage) is **not** an in-gate fallback — it is a separate architecture. Do not
  reach for that redesign mid-gate; the gate measures the K-slice path as specified.
- **Disposition of superseded specs.** This replaces the trace mechanism of
  `sdf-static-occluder-shadows`, `sdf-shadow-fine-atlas-trace` (in-progress; its fine
  field is *kept*, its dominant-direction trace *replaced*), and
  `sdf-shadow-unshadowed-direction` (draft; its per-light-visibility intent is subsumed —
  drop it). Rewrite `sdf_shadows.md` to the per-light / hybrid model at promotion.
