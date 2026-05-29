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

The primary split is **bake participation** (full map: `architecture.md`). **Baked
tier** lights are fixed-position, authored as `light`/`light_spot`/`light_sun`, and
carry `_shadow_type ∈ {static_light_map, sdf}` (default `static_light_map`) — the
shadow type decides only how a baked light's *direct* shadow resolves. `static_light_map`
(lightmap, free) is the default and carries fixed fill lighting; `sdf` (runtime-traced)
gives a sparse set of those lights tweakable shadows with no re-bake. **Dynamic tier**
lights are unbaked, runtime-only, authored as separate entities (`light_dynamic` /
`light_dynamic_spot`), rationed by a 12-slot shadow-map pool (`SHADOW_POOL_SIZE`), and are the only tier
that can shadow *moving* entities (baked + sdf shadow static geometry only). Dynamic
buys flexibility: script-driven (gameplay) intensity/color — working today via live
forward-shader curve eval, the one current dynamic use case — plus light movement and
dynamic-entity (enemy) shadows, both planned. SDF earns its per-fragment march where the lightmap can't: runtime
tweaks that baking can't follow, on more lights than shadow maps scale to.

This slice is a **perf gate**. It lands the real per-light path for a sparse SDF
light set on real authored content and measures it on the 2020 MBP. The full
redesign follows only if the gate holds: if the per-fragment march can't make budget
for the intended case (a handful of SDF lights, the rest baked/dynamic), the
fail-floor stands and SDF reverts. The perf measurement is the primary deliverable,
alongside a visibly correct cast shadow.

## Post-measurement amendments

This spec was authored against the dominant-direction model. A manual playthrough —
the spec's manual-visual acceptance check — surfaced these architectural corrections:
the SH volumes rendered fully unlit, exposing that the disjoint-set filter had starved
indirect and the dominant-direction trace was structurally broken. The corrections are
independent of the **still-pending perf gate** — correct whether or not the SDF perf bet
passes, since they delete broken code and fix a double-count + double-shadow — and must
land before that gate so it measures the clean uniform path. The detail lives in the
sections below; this is the changelog pointer.

- **Two-tier model, one shadow-type axis.** Bake participation is the primary split:
  baked-tier lights are fixed-position and bake (SH always; + lightmap or static-occluder
  atlas per shadow type); dynamic-tier lights bake nothing. **Shadow type**
  (`static_light_map`/`sdf`) is a baked-tier sub-choice deciding only a baked light's
  direct shadow. "Animated" is a baked light on an intensity curve, not a tier.
- **Indirect is tier-independent.** SH base + delta are fed by **all** baked-tier
  (static-position) lights regardless of shadow type; tagging a light `sdf` moves only
  its direct term.
- **Namespace filters key on the position axis** (`!is_dynamic`), not shadow type —
  they also feed the SH/delta bakes. The shadow-type exclusion drops to the direct
  lightmap consumers only.
- **`_shadow_type` rename + `static_light_map` value.** The baked-tier KVP is
  `_shadow_type ∈ {static_light_map, sdf}` (was `_shadow_tech ∈ {baked, sdf, dynamic}`);
  `dynamic` is no longer a shadow-type value. Default `static_light_map`.
- **Dynamic-entity split.** Dynamic lights are authored as their own FGD entities
  (`light_dynamic` / `light_dynamic_spot`) carrying none of the bake KVPs; the parser
  sets `is_dynamic` from the dynamic CLASSNAME, not from a shadow-type value. Dynamic
  is the unbaked, runtime tier, rationed to ≤ 12 visible — the only tier that shadows
  moving entities. **Script-driven (gameplay) lighting is a current dynamic use case** (the
  campaign-test arena lights, restored to the `ce2b555` forward-curve path); the `_animated`
  bake-the-script detour is retired. This de-conflates authoring along the bake-participation line.
- **New foundational task, sequenced first** (Task 1): animated cleanup + uniform
  model. Delta SH becomes indirect-only; the runtime animated SDF factor and the
  animated dominant-direction trace are removed; K reclaims to 4.
- **K = 4** everywhere (was 3): the reclaimed animated channel carries a per-light slice.
- **Specular shadowed by the light's own technique** for sdf lights (folds into the
  forward task).
- **Removal-candidates cleanup** for dead code the corrections strand.

## Background

Confirmed end-to-end this session (see `research.md`): with an SDF-baked PRL (under the
now-retired `--unshadowed-lightmap --bake-sdf` flags — see Scope) and SDF Shadow Mode On, the `occlusion-test`
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

- **Lighting tiers (primary split: bake participation).** Full map: `architecture.md`.
  **Baked tier** — fixed-position lights (`light`/`light_spot`/`light_sun`) that bake
  into at least one layer (SH always; + lightmap or static-occluder atlas per shadow
  type). **Dynamic tier** — unbaked, runtime-only lights, rationed by the **12-slot
  shadow-map pool** (`SHADOW_POOL_SIZE`, `lighting/spot_shadow.rs`; `LightSpaceMatrices` in
  `forward.wgsl`) — lights ranked by influence, the rest render unshadowed; the only tier
  that can shadow *moving* entities (baked + sdf resolve against
  static geometry only). Dynamic buys flexibility: **script-driven (gameplay)
  intensity/color** — working today via live forward-shader curve evaluation, the one
  current dynamic use case — plus light **movement** and **dynamic-entity (enemy) shadows**,
  both planned. `is_dynamic` is set by the dynamic-tier CLASSNAME, not by a shadow-type value.
- **Dynamic lights are separate FGD entities.** Dynamic lights are authored as
  `light_dynamic` / `light_dynamic_spot` (their own `@PointClass`, off a base that
  **omits** every bake KVP — `style`, `*_curve`, `_bake_only`, `_shadow_type`,
  `_animated`). The parser sets `is_dynamic = true` from the dynamic CLASSNAME (the
  existing static classes are `light`/`light_spot`/`light_sun`; dispatch in
  `quake_map.rs`, `LIGHT_CLASSNAMES`). The entity is **minimal** — geometric/color KVPs
  only. Do **not** design new dynamic-only KVPs (movement, entity-shadow opt-in) now;
  those arrive when those features are built.
- **Per-light shadow-type tag** `_shadow_type ∈ {static_light_map, sdf}`, default
  `static_light_map`, authored on the baked-tier entities in
  `sdk/TrenchBroom/postretro.fgd`, parsed to a `MapLight` field, carried through the PRL
  light entry to the runtime. The shadow type decides only how a baked light's *direct*
  shadow resolves; `dynamic` is **not** a shadow-type value.
- **Disjoint *direct* sets by shadow type (no double-count).** Disjoint means across the
  **direct** techniques — `lm_irr`/`lm_anim` vs runtime SDF vs shadow-map — never across
  SH. The namespace filters (`StaticBakedLights`/`AnimatedBakedLights`) key on the
  **position** axis (`!is_dynamic`) — reverting the shadow-type predicate the
  earlier committed work introduced — because those namespaces also feed the
  SH/delta bakes, which need every baked-tier light. The shadow-type exclusion (drop
  `sdf`) moves **down** to the direct lightmap consumers only — the static
  lightmap bake and the animated weight-map bake — so `lm_irr`/`lm_anim` hold *only*
  `static_light_map`-tagged lights while SH still sees all baked-tier lights. Dynamic-tier
  lights (`is_dynamic = true`) are excluded from every bake and routed to the shadow-map
  path. `sdf` stays baked-tier but is flagged in `spec_lights` for the runtime path.
- **Indirect is fed by all baked-tier lights, shadow-type-independent.** SH base + delta
  are fed by every baked-tier (static-position) light regardless of shadow type —
  **not** type-gated. Tagging a light `sdf` moves only its **direct** term to runtime; its
  indirect bounce still bakes into SH. Indirect goes dark only if a scene has zero
  baked-tier lights. **SDF never does indirect visibility** — SDF shadow rays resolve
  direct static-occluder shadows only, never modulating indirect.
- **Per-light diffuse for SDF-tagged lights** in the forward `spec_lights`/`chunk_grid`
  loop (currently specular-only), gated by the tag flag so `static_light_map`-tagged
  lights keep getting diffuse from `lm_irr` (untouched).
- **`static_light_map`-tagged lights use the shadowed bake.** With `sdf` lights out of
  the direct lightmap and the static dominant-direction trace removed, a
  `static_light_map`-tagged light's shadow can only come from the bake — so these lights
  bake *shadowed* (the standard bake captures hard static shadows), not `--unshadowed`.
  `LightmapMode` is permanently `Shadowed`.
- **Per-light SDF visibility:** a half-res pass traces one shadow ray per SDF-tagged
  light toward the **K most-influential** at each tile (K = the engine's per-fragment
  SDF shadow budget; seed K = 4 — Task 1 removes the animated dominant-direction trace
  that consumed the fourth channel, so all four channels of the half-res target carry
  per-light slices, see Rough sketch — which clears the author rule-of-thumb of ≤ 2 SDF
  shadows per surface with headroom). Forward upsamples and multiplies each SDF light's
  diffuse by its visibility. Beyond K overlapping SDF lights at a fragment, extras are
  dropped (treated lit) and the compiler warns.
- **Specular shadowed by the light's own technique.** An `sdf`-tagged light's **specular**
  multiplies by the same per-light SDF visibility slice its diffuse uses — the slice is
  already sampled, so the cost is near-zero and it removes specular-through-walls for sdf
  lights. **Known limitation:** `static_light_map`-tagged lights' specular stays unshadowed (they carry no
  runtime visibility; baked = free), and dynamic lights compute no specular term today.
- Reuse the fine SDF field (`sample_fine_distance`, `sdf_atlas`) and the established
  closest-passing-distance soft-shadow march, unchanged in math (open-space skip seeded
  loose — this session found the 2.5 default suppresses the trace).
- **Compiler defaults treat SDF as first-class, not opt-in.** Remove the `--bake-sdf`
  and `--unshadowed-lightmap` flags. The SDF occluder atlas bakes **automatically
  whenever the map contains any `sdf`-typed light** — content-driven, the same way the
  lightmap bakes because lights exist — so an `sdf`-typed light can never ship without
  the atlas it needs (the no-atlas-silent-no-shadow footgun is removed by construction).
  Lightmaps **always** bake shadowed: `BakeMode::Unshadowed` has no consumer in the
  uniform model (`LightmapMode` is permanently `Shadowed`) and is deleted with its flag.
  SDF is a standard bake output alongside the lightmap and SH volume — runtime SDF Shadow
  Mode (default On) and the dev On/Off/Visualize toggle remain the only places SDF is
  switchable, and those are runtime, not compile-time.
- **Removal-candidates cleanup.** Delete the dead `make_direction_view` (no caller since
  the static dominant-direction trace was removed — the `lm_irr` direction *binding* stays
  live for bumped-Lambert), the dead animated-weight-map `STAGE_VERSION` const advertising
  a non-existent cache key, and the stale `!is_dynamic`/`shadow_type` filter comments on the
  lightmap and weight-map bakes. No spec language calls animated "the last `Unshadowed`
  consumer" — there is no unshadowed-lightmap consumer now. Also flag the **`_animated` flag
  and the `f6bf69e` script→animated-baked compose bridge** — dead once script-driven lights are
  authored as dynamic-tier entities (gameplay-script lighting is unbakeable and belongs on the
  dynamic forward-curve path, not a baked compose route).
- A perf measurement on the 2020 MBP (`occlusion-test`, `campaign-test`) with
  `POSTRETRO_GPU_TIMING=1`, against a stated budget, on the SDF-intended case.

### Out of scope

- Removing the baked lightmap. It stays as the cheap default tech; a level may opt out of
  the baked **direct** lightmap by tagging baked-tier lights `sdf` (indirect bounce still
  bakes into SH) or by authoring them as dynamic-tier entities, but the bake pipeline is
  retained.
- The shadow-map machinery itself (unchanged) and full dynamic-light authoring beyond
  the minimal `light_dynamic` / `light_dynamic_spot` entities (classname → `is_dynamic`,
  geometric/color KVPs only). Dynamic-only KVPs for movement and entity-shadow opt-in
  arrive when those features are built.
- **Baked-curve** animated baked-tier lights (fixed loops from `style`/`brightness_curve`)
  keep the baked `lm_anim` direct path (Task 1 cleans it: delta SH indirect-only, runtime
  animated SDF factor and dominant-direction trace removed). **Script-driven** (gameplay)
  animation is *not* baked — those lights are dynamic-tier (see above). Any future migration
  of baked-curve lights onto runtime per-light curves is the follow-on.
- Animated lights with runtime SDF-traced shadows (`_shadow_type sdf` on an animated
  baked-tier light). Visibility is geometry-static and intensity rides the curve, so this
  folds into the static per-light machinery later, behind the perf gate. Until added,
  animated lights are lightmap-shadowed (`static_light_map`) only.
- *Known consequence (not out of scope, a design property):* an sdf light's shadow
  modulates only its **direct** term; its indirect bounce stays in SH unmodulated (the
  invariant: SDF never does indirect visibility), so in indirect-dominated views the
  shadow is subtle by design. Flag for the follow-on's visual ACs — not a slice decision.
- Converting existing test-room content is a separate content change, tracked separately.
  The spot test room becomes dynamic-tier: spots are SDF's worst case and shadow maps' best
  (one frustum each, already implemented), so the "right tool per light" invariant settles
  it. Re-authoring those lights as `light_dynamic_spot` also keeps the room out of the SDF
  perf gate. Decided, not an open question.
- **Content migration — campaign-test's arena lights (out of scope, tracked separately).**
  The 17 arena lights (`arena_1_light` spots, `arena_wave_2` points) are **script-driven**: a
  gameplay script animates their intensity in response to wave state. Script-driven gameplay
  lighting is **dynamic** (unbaked for creative flexibility — a script curve can't be baked),
  so they re-author to `light_dynamic` / `light_dynamic_spot` and **drop the `_animated 1`
  flag** (the failed bake-the-script detour), restoring the robust live forward-curve
  implementation from `ce2b555`. (An audit confirmed all 17 are script-driven, not
  position-moving; "moving" was the wrong criterion — `dynamic` here means
  *unbaked-for-flexibility*, which script-driven gameplay lighting is.) The re-author must land
  **before** the `campaign-test` perf measurement (Task 6), or the gate measures a polluted SDF
  set. Content change, tracked separately.
- Scaling K past the seed, and K-selection quality tuning past "nearest/brightest."

## Acceptance criteria

Automated (host-side / shader-compile / unit — deterministic):

- [ ] `_shadow_type` parses from FGD to a `MapLight` tag with `static_light_map` default;
      an unknown value errors at compile (mirrors `_cast_entity_shadows` parsing). Only
      `static_light_map` and `sdf` are valid values.
- [ ] Dynamic-entity classnames (`light_dynamic` / `light_dynamic_spot`) set
      `is_dynamic == true` from the classname (not from a shadow-type value) and omit the
      bake KVPs; baked-tier classnames (`light`/`light_spot`/`light_sun`) resolve to
      `is_dynamic == false`.
- [ ] Disjoint *direct* sets: a bake-level test asserts `sdf`-typed and dynamic-tier lights
      are excluded from the **direct** lightmap sets (`lm_irr` static and `lm_anim` animated)
      only, and `static_light_map`-typed lights are included — so neither baked atlas ever
      shares a light with the runtime SDF set.
- [ ] Indirect not starved: a bake-level test asserts `sdf`-typed **and** animated
      baked-tier lights **remain** in the SH base and delta bake light sets — the
      namespace filter keys on the position axis, so the shadow type never drops a light from
      indirect.
- [ ] K-light selection is deterministic and bounded: a headless GPU-readback compute
      test (the `curve_eval_test.rs` precedent — dispatch the WGSL selection helper, read
      back the chosen indices, self-skip when no adapter) asserts the shared helper returns
      at most K (= 4) indices and matches a Rust reference comparator's pinned total order for
      fixed light sets. Visibility-pass ↔ forward parity is **by construction** — both call
      the same verified helper — not separately tested. (The comparator is pinned to a single
      total order — influence descending, light index ascending; see Rough sketch.)
- [ ] The static-light forward loop computes a diffuse term gated by the SDF tag; a
      shader naga-validation test confirms it compiles and reads the K-slice target.
- [ ] An `sdf`-typed light's specular term reads the same per-light visibility slice as
      its diffuse; a shader naga-validation test confirms the specular term reads the
      visibility slice for sdf lights.
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
      shadows per surface) and the rest `static_light_map`/dynamic, frame time holds the stated
      budget on `occlusion-test` and `campaign-test`; per-pass `POSTRETRO_GPU_TIMING=1`
      numbers recorded in `research.md`. If it cannot be brought into budget by the
      bounded knobs (K, half-res, march steps, cull radius), the slice **reports the
      fail-floor** rather than masking it.

## Tasks

### Task 1: Animated cleanup + uniform model (foundation, gate-independent)

Land the uniform two-tier model before anything else. This task deletes broken code and
fixes a double-count + double-shadow; it is **correct on its own even if SDF later reverts**.

- **Delta SH volume becomes indirect-only** (was full direct+indirect). This is a conscious
  amendment to the `done/` plan `perf-animated-sh-light-culling`'s output contract — call the
  cross-plan revision out explicitly. The `lm_anim` direct term and the delta SH no longer
  both carry the animated light's direct contribution, removing the `lm_anim`↔delta-SH direct
  double-count. The `delta_scale` dev knob is retired.
- **Remove the runtime animated SDF shadow factor.** The animated shadow is already baked,
  occlusion-tested, into `lm_anim` (verified) — the runtime factor double-shadows it.
- **Remove the animated dominant-direction trace entirely.** Drop the
  `animated_lm_direction` atlas, drop `SDF_SHADOW_FLAG_ANIMATED`, and free the
  lightmap-UV gbuffer MRT (the per-light SDF trace keys on light **position**, not lightmap
  UV — the MRT existed only for the animated trace).
- **Reclaim K = 4.** With the animated channel of the 4-channel half-res target freed, all
  four channels carry per-light slices.

### Task 2: Tier split — dynamic FGD entities + `_shadow_type` two-value restriction (gate-independent)

De-conflate authoring along the bake-participation line. This task is **gate-independent**
— it is correct even if SDF later reverts, because the tier split is orthogonal to how a
baked light's direct shadow resolves.

- **Dynamic FGD entities.** Add `light_dynamic` / `light_dynamic_spot` as their own
  `@PointClass` in `sdk/TrenchBroom/postretro.fgd`, off a base that **omits** every bake
  KVP (`style`, `*_curve`, `_bake_only`, `_shadow_type`, `_animated`). The entities are
  **minimal** — geometric/color KVPs only. Do **not** add dynamic-only KVPs (movement,
  entity-shadow opt-in) now; those land with their features.
- **Classname dispatch sets `is_dynamic`.** The `quake_map` parser sets `is_dynamic = true`
  from the dynamic classnames, not from a shadow-type value. Extend the existing static
  dispatch (`light`/`light_spot`/`light_sun`, `LIGHT_CLASSNAMES` in `quake_map.rs`).
- **Restrict `_shadow_type` to two values.** `_shadow_type ∈ {static_light_map, sdf}`
  only — `dynamic` is **not** a shadow-type value (the dynamic tier is selected by
  classname). Authored only on the baked-tier entities.

### Task 3: Per-light shadow-type contract + disjoint direct sets

Add `_shadow_type {static_light_map|sdf}` to the baked-tier FGD entities and the
`quake_map` parser (mirror `_cast_entity_shadows`, `quake_map.rs:279`), default
`static_light_map`. Carry it as a `shadow_type`
field on the **compiler-side** `MapLight` (`map_data.rs:190`, the struct that has
`animation: Option<LightAnimation>` at `:218` and is read by the bake filter) AND thread
it separately through the **runtime** `MapLight` (`prl.rs:137`, which has no `animation`
field — it carries `animated_slot: Option<u32>` instead). The dynamic tier reaches the
runtime via `is_dynamic` (Task 2's classname dispatch), not via a shadow-type value.
**Key the namespace filters on the position axis** —
`StaticBakedLights::from_lights` (`light_namespaces.rs:61`) and
`AnimatedBakedLights::from_lights` (`:102`) must use `!is_dynamic`; **revert the
shadow-type predicate the earlier committed work introduced** (it starved SH),
because both namespaces also feed the SH/delta bakes, which need every baked-tier light. Push the
shadow-type exclusion **down** to the direct lightmap consumers only: the static lightmap bake
(drops `sdf`, leaving `lm_irr` `static_light_map`-only) and the animated weight-map bake (drops
`sdf` and, by `is_dynamic`, dynamic-tier lights — e.g. the migrated campaign-test
spots — leaves `lm_anim` and routes to the shadow-map path). Indirect (SH base + delta) keeps
seeing all baked-tier lights, sdf and animated included. Warn where the runtime would drop a
light — count `sdf` lights whose influence covers a `chunk_grid` cell and warn when a cell
exceeds K (the warning's unit is the `chunk_grid` cell, mirroring the runtime K-selection;
`sdf` lights are excluded from the direct lightmap, so a "baked texel" is the wrong frame);
bump the lightmap stage's `STAGE_VERSION` so stale cache entries don't serve an old lightmap.
In `pack_spec_lights` (`spec_buffer.rs`): flag `sdf`-typed lights so the forward loop knows
which get the runtime diffuse + visibility path.

### Task 4: Per-light visibility pass (K-slice half-res trace)

Restructure the half-res SDF pass (`SdfShadowPass`, `render/sdf_shadow.rs:120`;
`shaders/sdf_shadow.wgsl`) to trace **per-light**. Per half-res pixel: reconstruct
world position (existing), select up to K influential `sdf`-typed lights from
`chunk_grid` (shared selection helper — see Rough sketch), trace one `trace_shadow`
ray toward each, write K=4 visibility factors to the existing 4-channel `Rgba8Unorm` shadow
factor. With Task 1's removal of the static **and** animated dominant-direction traces, all
four channels (R/G/B/A) are free, so the K=4 per-light slices fill the target — no texture
array, no new binding. Drop the dominant-direction bindings (`static_lm_direction`,
`animated_lm_direction`); the per-light SDF trace keys on light position. March math
unchanged; open-space skip seeded loose. Register the new half-res SDF visibility pass in the
`POSTRETRO_GPU_TIMING` pass set so the perf gate can read its per-pass time.

The lightmap-UV gbuffer MRT in the depth pre-pass (the `lightmap_uv_view` slot,
`sdf_shadow.rs:152`/`:156`) is freed in Task 1 — the per-light trace does not consume it.

### Task 5: Per-light diffuse + specular + visibility in forward

Extend the static-light loop (`forward.wgsl:707-731`) to add a Lambert diffuse term for
`sdf`-typed lights (gated by the Task 3 flag), multiply each by its upsampled K-slice
visibility, and sum into `static_direct`. **Multiply each `sdf` light's specular by the same
visibility slice** (the slice is already sampled — near-zero cost, removes
specular-through-walls for sdf lights). `static_light_map`-typed lights' specular stays unshadowed and
dynamic lights compute no specular term (known limitation). `lm_irr` (`static_light_map`-typed lights)
and `lm_anim` are untouched — disjoint *direct* sets mean no re-weighting is needed. Bind the
K-slice target where `sdf_shadow_factor` (`@group(5) @binding(3)`) was; adjust
`upsample_shadow_factor` (`forward.wgsl:486`) to sample a slice.

### Task 6: Perf gate + fail-floor report

Run the perf gate on the 2020 MBP against the SDF-intended case, record per-pass GPU
timing in `research.md`, and state explicitly whether the slice holds budget — and if
not, what the bounded knobs recover and whether it trips the fail-floor.

## Sequencing

**Phase 1 (sequential):** Task 1 — the animated cleanup + uniform model. Gate-independent;
deletes broken code and fixes the double-count/double-shadow. Lands first so every other
task rides the corrected model.
**Phase 2 (sequential):** Task 2 — the tier split (dynamic FGD entities + `_shadow_type`
two-value restriction). Gate-independent; de-conflates authoring along the bake-participation
line. Pairs with the contract task — it establishes the `is_dynamic`-by-classname and the
two-value shadow-type axis the contract task then threads.
**Phase 3 (sequential):** Task 3 — the shadow-type contract + disjoint *direct* sets are the
contract the runtime path rides.
**Phase 4 (sequential):** Task 4 — consumes the `spec_lights` SDF flags; produces the
K=4-slice target.
**Phase 5 (sequential):** Task 5 — consumes Task 4's K-slice target (diffuse + specular).
**Phase 6 (sequential):** Task 6 — measures the assembled path; gates the follow-on.

## Implementation notes (orchestration)

What's unusual about *this* build — standard practice (dispatch via `implement-task`,
inline skill discipline, worktree isolation + no destructive git for any concurrent
agents, decompose-at-creation) still applies and is not restated here.

- **This is a correction of committed code, not greenfield.** The original dominant-direction
  tasks shipped; most tasks here *revert and delete* (the `shadow_type`-keyed filter, the
  animated dominant-direction trace, delta-SH-full→indirect-only, the `_animated` bridge).
  Read the current committed state first; expect diffs that are deletions as much as
  additions. The removal candidates are **intentional removals, not bugs to repair** — do not
  "helpfully" preserve them.
- **Sequential by necessity.** Multiple tasks touch the same files (`forward.wgsl`,
  `light_namespaces.rs`, `sdf_shadow.*`). The phases are ordered for dependencies *and* to
  avoid shared-file contention — do not parallelize them without worktree isolation.
- **Human-only ACs are hard handoffs.** The visual ACs and the perf gate (2020 MBP) are not
  agent-verifiable. An agent completes the automated ACs and **stops** — it must never claim
  visual or perf success.
- **Trace runtime paths end-to-end; don't trust static reading.** The `_animated` bridge
  *looked* fully wired but never delivered. For runtime-touching tasks, capture `RUST_LOG`
  and verify behavior, not just compilation.

## Rough sketch

**Disjoint *direct* sets kill the double-count.** A light's **direct** term arrives through
exactly one technique — baked into `lm_irr`/`lm_anim` *or* evaluated at runtime (sdf) *or*
shadow-mapped (dynamic) — never two. Disjoint is across the **direct** techniques, never
across SH: every static-position light still feeds indirect. The namespace filters
(`light_namespaces.rs:61`/`:102`) key on the position axis (`!is_dynamic`) — reverting the
shadow-type predicate that starved SH — because they also feed the SH/delta bakes; the `sdf` (and, by `is_dynamic`, dynamic-tier) exclusion lives **down** at
the direct lightmap consumers (static lightmap bake, animated weight-map bake), making the
direct lightmap and the runtime SDF set disjoint by construction. The forward simply *adds*
the SDF lights' contribution with no re-weighting. The "SDF forced to 1.0 ⇒ baseline" AC
guards this.

**Author rule ↔ K budget.** "≤ 2 SDF shadows per surface" is the per-fragment SDF trace
budget. Seed K = 4 (Task 1 removes the dominant-direction traces, freeing all four channels
— see K-slice target below) gives headroom of two over the ≤2 rule; beyond K overlapping SDF
lights, extras drop (lit) and the compiler warns. The authoring guideline and the perf knob
are one value.

**Light-selection parity (load-bearing).** Task 4's per-tile K-selection and Task 5's
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
(defined as `SHADOW_FACTOR_FORMAT` at `sdf_shadow.rs:32`, at `@group(5) @binding(3)`) in
place as the K-slice target. The constraints converge: group 5 has no spare binding and
K>4 is out of scope, so the per-light slices live in this target. Task 1 removes the static
**and** animated dominant-direction traces, freeing all four channels (R/G/B/A), so the four
channels carry the K=4 per-light slices — no texture array, no new binding, no shared-BGL
ripple across forward/billboard/fog. The implementer assigns which channel holds which slice
when they land Task 4. K>4 would need a separate target (texture array), which is out of scope.

**Established technique.** The per-light ray-marched soft shadow against a distance
field is standard (UE distance-field shadows; closest-passing-distance penumbra). The
novelty is only the K-bounded per-tile selection + half-res amortization; the march is
the existing `trace_shadow`.

## Boundary inventory

| Name | FGD KVP | Rust (`MapLight`) | Wire (PRL light entry) | WGSL (`spec_lights`) |
|---|---|---|---|---|
| shadow type | `_shadow_type` = `static_light_map`\|`sdf` (default `static_light_map`) | `shadow_type: ShadowType` enum | `u8` 0=static_light_map, 1=sdf (2 values) | `color_and_pad.w` flag: encoded 1.0/0.0; decode `w > 0.5` ⇒ sdf-typed |
| tier | dynamic-tier classname (`light_dynamic`/`light_dynamic_spot`) | `is_dynamic: bool` (set by classname) | `is_dynamic` flag (separate field) | not on `spec_lights` (dynamic lights take the shadow-map path) |

`static_light_map` needs no runtime `spec_lights` flag (→ `lm_irr`); the dynamic tier
reaches the runtime via `is_dynamic` (a separate field, set by classname — **not** the
shadow-type u8) and takes the shadow-map path. Only the `sdf` distinction must reach the
forward loop, so the currently-unused `color_and_pad.w` (`forward.wgsl:95`) carries it — no
buffer-size change.

### Light-entity KVPs — what this slice touches

This slice changes **exactly two** light-entity KVPs: `_shadow_tech` → `_shadow_type`
(rename; values `static_light_map`/`sdf`) and `_animated` (**removed** — script-driven
lights become dynamic-tier entities). **Every other light KVP is pre-existing and
load-bearing — do not strand it** under the completeness contract. The full set (FGD
`@BaseClass = Light`): intensity/color (`light`, `_color`, `_fade`, `delay`), geometry
(`origin`, `angles`/`mangle`/`angle`, `_cone`/`_cone2`), baked-curve animation (`style`,
`_phase`, `brightness_curve`/`color_curve`/`direction_curve`, `period_ms`, `_curve_phase`),
and flags (`_bake_only`, `_cast_entity_shadows`, `_start_inactive`, `_tags`).

`_tags` is load-bearing: scripts query lights by tag (e.g. the arena script). So the
**dynamic-tier entities must retain `_tags`** plus the shared non-bake KVPs
(intensity/color/geometry) — they omit only the bake-specific keys (`style`, `_phase`,
`*_curve`, `period_ms`, `_curve_phase`, `_bake_only`, `_shadow_type`, `_animated`).

## Wire format

The PRL light section (`AlphaLightsSection`, section ID 18,
`crates/level-format/src/alpha_lights.rs`) gains a `shadow_type: u8` carrying **2 values**
(0=static_light_map, 1=sdf), appended at the record tail in `to_bytes` (`:146`), mirroring
how `casts_entity_shadows` is encoded. The dynamic-tier distinction does **not** ride this
u8 — it reaches the runtime via the separate `is_dynamic` field (set by classname), so the
shadow-type u8 stays two-valued.

The section today has **no version field** — backward compatibility rides record-stride
detection (`ALPHA_LIGHT_RECORD_SIZE = 73` at `:104` vs `ALPHA_LIGHT_RECORD_SIZE_LEGACY =
72` at `:109`, disambiguated at `:171`). Rather than extend that to a third stride (74) —
a collision-prone heuristic the moment a future field lands a same-length record — **add a
section-internal version field** to `AlphaLightsSection`, mirroring the `SH_VOLUME_VERSION`
precedent (a per-section version, distinct from the PRL header `CURRENT_VERSION = 4`,
bumped when the record layout changes so the loader rejects stale `.prl` with a clear
error). Bump it for this change; the deserializer reads it and decodes `shadow_type` only
when present, defaulting older records to `static_light_map`. Pre-release, recompiling maps from
`.map` source is the expected path (the build cache invalidates on the stage version), so
a clean version field beats carrying the stride heuristic forward. Retire the 72/73 stride
detection in the same pass if the version field subsumes it.

## Open questions

- **Resolved — SH vs. indirect.** Indirect is shadow-type-independent: SH base + delta are
  fed by all baked-tier (static-position) lights regardless of shadow type (the namespace
  filters key on the position axis). SH is **indirect-only** — base and delta both store
  bounce only. Tagging a light `sdf` moves only its direct term to runtime.
- **Resolved — delta-SH ↔ `lm_anim` double-count.** Delta SH is now indirect-only (Task 1),
  so the animated light's direct contribution lives only in `lm_anim`; the delta volume no
  longer re-counts it. The `delta_scale` knob is retired.
- **Perf topology if the gate is marginal — the measurement is the irreducible open
  item.** The bounded knobs (K, half-res scale, march steps, cull radius) are the gate's
  tuning envelope and stay open pending Task 6 numbers. Note: tracing inline in forward (no
  K-slice storage) is **not** an in-gate fallback — it is a separate architecture. Do not
  reach for that redesign mid-gate; the gate measures the K-slice path as specified.
- **Disposition of superseded specs.** This replaces the trace mechanism of
  `sdf-static-occluder-shadows`, `sdf-shadow-fine-atlas-trace` (in-progress; its fine
  field is *kept*, its dominant-direction trace *replaced*), and
  `sdf-shadow-unshadowed-direction` (draft; its per-light-visibility intent is subsumed —
  drop it). Rewrite `sdf_shadows.md` to the per-light / hybrid model at promotion.
