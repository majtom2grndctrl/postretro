# Animated direct SH on dynamic receivers

## Goal

A script-animated baked `light_spot`/`light` lights static world surfaces (animated
lightmap `lm_anim`) and bounces through the animated indirect SH delta, but its
animated **direct** term never reaches kinematic movers, skinned meshes, or
billboards — they see only static indirect SH, so a door inside a pulsing alarm
cone reads much darker than the wall beside it. Bake each animated baked light's
direct transport as a sparse octahedral **direct-SH delta** — the additive twin
of the existing indirect delta (`DeltaShVolumes`, id 27) and the subtractive
promotion delta (`DirectShDeltaVolumes`, id 41) — and compose it per frame into
the direct atlas that all dynamic receivers already sample. This closes the
last direct-light gap for moving receivers under authored script animation.

## Scope

### In scope

- New PRL section `AnimatedDirectShDeltaVolumes` (id 45): per-animated-baked-light
  sparse direct-SH octahedral delta tiles, affinity-cell CSR, occlusion baked at
  compile time.
- Compiler bake keyed on the existing `AnimatedBakedLights` namespace
  (`!is_dynamic && animation.is_some()`), reusing the direct-SH primitives.
- Renderer: a second `direct_sh_compose` pass that **adds** the animated term
  per-frame, modulated by the shared compose animation descriptors — the same buffer
  `lm_anim` and the indirect SH delta already consume (split from the promotion
  compose to stay within the compute storage-buffer budget; see Task 3).
- Receivers: kinematic movers (required), skinned meshes, and billboards — all
  three already sample the composed direct atlas at binding 15, so the fix is
  producer-side only.
- Widen the `has_direct` gate and composed-atlas allocation so a map with only
  animated baked lights (no static `DirectShVolume`) still gets a direct atlas.
- Fixture: align `content/dev/maps/spawner-test.map` with the feature's premise —
  the tagged `alarm_light` becomes a baked animated `light_spot` in whose cone the
  closet-door mover sits, plus a `prop_mesh` (skinned-mesh receiver) and a
  `billboard_emitter` (billboard receiver) placed in the same cone. The later
  `capture-animated-direct-receiver-goldens` plan covers the mover and prop_mesh
  stills; the emitter's particle-produced billboard remains a manual-GPU check.

**Shipped beyond this spec's stated producer-side scope (undocumented at the time,
recorded here per dev guide §1.2):** the implementation also delivered two pieces of
shared animated-light infrastructure affecting **all** scripted animated lights, not
only animated-direct receivers — (a) a finite (`playCount`-bounded) endpoint-clamped
curve-sampling mode, marked by a negative packed period and decoded by the shared WGSL
`animation_curve_t` (`crates/renderer/src/shaders/curve_eval.wgsl`) and its CPU mirror
(`render-cpu/src/sh_compose.rs`); and (b) a `check_play_count_completion` settle-math
fix — `settled.intensity = final_brightness` became `settled.intensity *=
final_brightness`, preserving authored base intensity on settle. Both live in
`crates/postretro/src/scripting/systems/light_bridge.rs::pack_animation_descriptor` /
`check_play_count_completion` and are covered by tests.

### Out of scope

- **Promoting animated lights into the runtime shadow-map pool.** Kept separate by
  design (`is_promotable_base_light` already excludes `is_animated`/`animation.is_some()`).
  v1 delivers the baked static-occlusion SH quality tier only; runtime shadow-map
  quality for animated lights is a deliberate future escalation, not a v1 handoff.
- **Direction-animated mover direct — the dynamic tier's job, by design.** The baked
  direct-SH delta encodes the light's authored (rest) cone direction; brightness and
  RGB color animate the delta. A cone that must *rake* across moving geometry with a
  live direction is precisely what the dynamic tier exists for, and `light_dynamic_spot`
  already lights movers through the runtime loop with a real cone and shadows. So this
  is not a baked-tier limitation to close later — it is the "static-vs-dynamic is an
  authoring choice, not an engine rule" invariant (index.md §2) drawing its line: the
  baked animated tier serves the pulse/color theatrical vocabulary (alarm red,
  strobing); a swept searchlight over movers is authored dynamic. The rest-direction
  bake keeps world (`lm_anim`) and mover terms consistent for the brightness/color
  cases; it does not crash or corrupt the atlas when a direction curve is present.
- **Static world direct.** Unchanged — stays animated-lightmap-based (`lm_anim`).
- **Entity self-shadowing under an animated light.** The baked SH delta cannot
  encode a receiver's own geometry (probes know nothing of the mover). Self-shadow
  quality is the promotion tier, which is out of scope above.
- **New scripting primitive.** `setLightAnimation` and the compile-time light-
  membership reservation already exist; no SDK surface change.
- **Runtime level compilation.** The delta is baked offline by `prl-build`.

## Design decisions

The prompt's seven decisions, resolved:

1. **Receiver scope (v1).** Kinematic movers required. Skinned meshes and
   billboards **included**, not deferred — all three bind the composed direct atlas
   (`sh_direct_atlas`, binding 15) via `sample_sh_direct`, so once the compose pass
   writes the animated term they receive it for free. Verified: `sample_sh_direct` at
   binding 15 is read in `kinematic_brush.wgsl` (movers), `skinned_mesh.wgsl`, and
   `billboard.wgsl` — the "producer-side only" claim is anchored to these three call
   sites. Billboards evaluate direct SH
   per vertex (one sample per sprite); the animated pulse is coarse there, matching
   the already-accepted per-vertex approximation for their other lighting channels.

2. **Direct-light quality.** Baked static-occlusion SH approximation — directional
   L2 SH, occlusion ray-traced against static geometry at bake time, no receiver
   self-shadow. Same quality tier the static `DirectShVolume` base already gives
   non-animated baked lights. Promoted runtime shadow-map quality is explicitly not
   pursued for animated lights in v1 (see Out of scope).

3. **Data path.** A direct-SH animated delta **parallel to the indirect animated
   delta**. New section 45 mirrors `DeltaShVolumes` (id 27) field-for-field —
   affinity-cell CSR, dense 64-probe octahedral sub-blocks, one per (cell, light) —
   but carries **direct** unit-radiance transport (occlusion-tested cone/falloff)
   instead of indirect bounce, and reuses section 27's `animation_descriptor_indices`
   mapping (same `AnimatedBakedLights` index space). The runtime term is an
   **addition** into the composed direct atlas, distinguishing it from the promotion
   delta (id 41), which is a weighted subtraction. No promotion, no handoff.

4. **Compose exactly once.** The compose pass evaluates each animated light's curve
   through the existing shared descriptor + WGSL helper — the `animated_light_scale`
   pattern from `sh_compose.wgsl`: `intensity × (base_color or sampled color curve)
   × brightness curve`, gated by `is_active`, applied once to unit-radiance delta
   tiles. The delta tiles carry no radiance of their own, so brightness/RGB compose
   in exactly one place, identical to how `lm_anim` and the indirect delta apply
   the same descriptor. Direction is baked at rest (see Out of scope).

5. **Transition / no double-count.** In v1 there is **no** runtime-promoted path for
   animated lights, so there is no crossfade to manage and no pop. An animated baked
   light routes its direct through the animated compose path in every runtime state
   — initial, active, looping, settled, cleared — because its compose descriptor
   stays active reading authored (or settled) radiance (`pack_compose_animation_descriptor`,
   via the `active_without_animation` arg of `pack_animation_descriptor`). No
   double-count: the light is absent from the
   `DirectShVolume` base (`StaticBakedLights` filters `animation.is_none()`), absent
   from the dynamic-direct `lights` buffer (baked lights never enter it), and absent
   from promotion selection (`is_promotable_base_light` excludes animated). The
   animated direct delta is its sole mover-direct source, by construction.

6. **Budgets.** A second additive compute pass over the direct probe atlas array
   (small — probe-atlas sized, not screen-sized), split from the promotion compose to
   stay within the compute stage's storage-buffer budget (see Task 3). Structure is
   chosen at load time: a **45-present** map runs the Pass A/B pair on one widened
   dispatch predicate (promotion active **or** any section-45 descriptor active, plus
   copy-through); a **45-absent** map is Case 1 — the single unchanged
   `direct_sh_compose` pass, no Pass B, no intermediate, no dummy buffers, today's
   cadence. GPU memory: one new sparse f16 section, same footprint class as the indirect
   delta — same affinity-cell reach as id-27 (both decompose via
   `decompose_affinity_for_lights`'s falloff-sphere AABB, no cone clipping at the CSR
   level; cone/occlusion narrowing happens per-probe inside the bake, zeroing
   out-of-cone probes) — plus (45-present only) one probe-atlas-sized intermediate texture.

7. **Acceptance surfaces.** Compiler bake + section round-trip; PRL wire + loader
   validation with all-or-nothing clear; renderer compose extension + widened
   `has_direct`; scripting lifecycle (rides existing descriptors, no new primitive);
   FGD/docs note the new dynamic-receiver behavior; dev-tools isolation of the
   animated-direct term; fixture regression. Detailed in Acceptance criteria.

## Lifecycle

An animated baked light's direct term follows one path on movers across its whole
script lifecycle. The compose descriptor is the single seam; the mover never
branches on runtime state.

```
COMPILE TIME (prl-build)
  tagged light_spot reached by setLightAnimation
    → compile-time light membership reserves an AnimatedBakedLights slot
    → excluded from DirectShVolume base (StaticBakedLights: animation.is_none())
    → excluded from promotion selection (is_promotable_base_light: !is_animated)
    → bake_animated_direct_sh_delta: occlusion-tested DIRECT transport
      → AnimatedDirectShDeltaVolumes (id 45), sparse affinity-CSR, rest direction
    (parallel: lm_anim weight map [world direct], DeltaShVolumes 27 [indirect])

RUNTIME (per frame)  light_bridge → compose animation descriptor (shared buffer)
  ┌─────────────────────────────────────────────────────────────────────┐
  │ state           │ descriptor.is_active │ curve source                │
  ├─────────────────┼──────────────────────┼─────────────────────────────┤
  │ initial active  │ 1                    │ evaluated from frame 0       │
  │ initial inactive│ 0                    │ contributes 0 (start dark)   │
  │ trigger fires   │ 1                    │ curve installed, evaluated   │
  │ looping         │ 1                    │ fract(t/period+phase) wrap   │
  │ one-shot settle │ 1 (anim cleared)     │ authored radiance = final kf │
  │ cleared (null)  │ 1 (anim cleared)     │ authored static radiance     │
  │ despawn/reload  │ section 45 dropped   │ no contribution              │
  └─────────────────────────────────────────────────────────────────────┘
        │
        ▼
  direct_sh_compose (compute, pre-frame — structure by load-time case, Task 3)
    Case 1 (no id 45): one pass, base − Σ_promo, clamp ≥ 0 → binding 15 (unchanged)
    Case 2 (id 45 present): atomic Pass A/B pair
      Pass A: interm = base(static direct) − Σ_promo (id 41)          → intermediate
      Pass B: composed = clamp(interm + Σ_anim(scale_j(t)·anim_delta_j), ≥ 0) [id 45]
              → Rgba16Float composed direct atlas (binding 15)
        │
        ▼
  DYNAMIC RECEIVERS  sample_sh_direct(binding 15), gated by has_direct
    kinematic mover · skinned mesh · billboard  → animated direct term, no pop
```

Because the compose descriptor stays active with authored/settled radiance whenever
the animation is absent, one-shot completion and clearing hold the light at its
current radiance on movers exactly as they do on the world lightmap — no brightness
pop, no state-dependent branch on the receiver.

## Receiver-by-light-mode matrix (DIRECT term)

Columns are light modes; rows are receivers; cells name the technique that
delivers **direct** light. "—" means the mode does not apply to that receiver.
Bold marks what this plan adds.

| Receiver         | Static baked (non-anim) | **Animated baked**            | Dynamic tier (runtime)      | Promoted static           |
|------------------|-------------------------|-------------------------------|-----------------------------|---------------------------|
| Static world     | lightmap `lm_irr`       | animated lightmap `lm_anim`   | — (dynamic never bakes)     | lightmap + shadowmask     |
| Kinematic mover  | DirectShVolume base     | **anim direct delta (id 45)** | dynamic-direct loop         | `(1−w)` SH + `w` pool map  |
| Skinned mesh     | DirectShVolume base     | **anim direct delta (id 45)** | dynamic-direct loop         | `(1−w)` SH + `w` pool map  |
| Billboard        | DirectShVolume base     | **anim direct delta (id 45)** | dynamic-direct loop (per-vtx) | per-vtx unshadowed        |
| Fog              | — (binds, never samples)| — (binds, never samples)      | spot/point scatter          | excluded from scatter     |

Every cell routes each physical light's direct term through exactly one technique
per receiver — the no-double-count invariant holds column by column.

## Acceptance criteria

Each AC is tagged with how it is verified: `[unit]` runnable test, `[loader unit]`
byte-feeding loader test, `[golden]` GPU-adapter-gated threshold image, `[review]`
grep/read gate, `[manual GPU]` env-var run. Not every AC is a unit test — the tag tells
the executor which gate applies.

- [ ] **AC1** `[golden]` A kinematic mover fully inside a script-animated
      `light_spot`'s cone receives the light's animated direct illumination; the authored-red
      `spawner-test` capture scene in `capture-animated-direct-receiver-goldens` AC2 confirms
      the closet door reddens with the adjacent cone-lit wall (no dark mover beside a lit wall).
- [ ] **AC2** `[golden]` (prop_mesh) + `[manual GPU]` (billboard) + `[review]` The
      `prop_mesh` (skinned mesh) and `billboard_emitter` (billboard) added to the Task 5
      fixture cone receive the same animated direct term as the door mover (shared composed
      atlas). The authored-red `spawner-test` capture scene in
      `capture-animated-direct-receiver-goldens` AC2 confirms the prop_mesh reddens with the
      wall; the billboard remains a manual-GPU check because its sprite requires a particle
      simulation tick. "No per-receiver wiring beyond the compose pass" is a review gate
      (consumers unchanged, still sample binding 15).
- [ ] **AC3** `[unit]` The animated light's direct is counted exactly once on each
      receiver: absent from the `DirectShVolume` base atlas, absent from the dynamic-direct
      light buffer, absent from promotion selection — a compiler namespace-partition test
      that a script-animated baked light produces an `AnimatedDirectShDeltaVolumes` entry
      and no `EntityShadowLights`/base-direct contribution. (Certifies pre-existing
      namespace exclusion; kept as a guard.)
- [ ] **AC4** `[unit]` (per-state scale math via the CPU scale seam) + `[golden]`
      (baked start-active rest baseline) + `[manual GPU]` (no brightness pop, despawn/reload
      drops cleanly): the no-authored-state `spawner-test` capture scene in
      `capture-animated-direct-receiver-goldens` AC3 records the fixture's baked rest
      descriptor and distinguishes it from the authored-red AC2 frame. Initial-active lights
      the mover from frame 0; initial-inactive (`startActive: false`) dark; a trigger-installed
      curve lights it on fire; a looping curve animates each cycle; a one-shot settles/holds
      the final keyframe with no pop; clearing (`setLightAnimation(null)`) holds authored
      radiance; despawn/reload drops the contribution cleanly. The unit test covers the
      per-state scale math; multi-frame no-pop / clean-drop visuals remain the manual-GPU
      check.
- [ ] **AC5** `[unit]` (direction-safe/no-NaN bake) + `[manual GPU]` (brightness/RGB
      animate): brightness and RGB color animate the mover-direct term; direction animation
      does not rotate it (documented v1 limitation, not a test) and does not crash or
      corrupt the atlas. The bake-safety unit test is automated; the brightness/RGB visual
      is the manual-GPU check (automated capture golden deferred to the follow-up spec).
- [ ] **AC6** `[unit]` (dispatch predicate is a pure fn) + `[manual GPU]` (allocation /
      `has_direct` need an adapter) + `[review]` (no-new-dispatch is a code-path/timing gate,
      not a pixel golden — a captured PNG cannot expose dispatch counts): a map with
      **only** animated baked lights (no static `DirectShVolume`) still allocates a composed
      direct atlas, sets `has_direct`, and lights movers; a map with **zero** animated baked
      lights keeps the promotion-only compose cadence (Case-1 predicate unchanged) and issues
      no per-frame compose dispatch attributable to this feature.
- [ ] **AC7** `[golden]` (visible scene color) + `[review]` (Case-1 code path): Static
      world surfaces render unchanged (still `lm_anim`-based) — the visible-scene golden
      covers this. The stronger "byte-identical on the direct atlas" claim is a `[review]`
      code-path gate, not a pixel golden: the direct atlas is an internal GPU texture the
      capture harness never reads back, so atlas identity is guaranteed by leaving the
      Case-1 branch (section 45 absent) byte-for-byte unchanged, verified by review, not by
      diffing a PNG.
- [ ] **AC8** `[loader unit]` Loader rejects a malformed or partial section 45 by cleanly
      disabling the animated-direct term (no crash), mirroring the id-41 soft-drop.
- [ ] **AC9** `[manual GPU]` + `[review]` `POSTRETRO_GPU_TIMING=1` attributes the animated
      additive cost to the `direct_sh_compose` bracket (TIMESTAMP_QUERY GPU); a dev-tools
      control isolates the animated-direct contribution for inspection.
- [ ] **AC10** `[unit]` (prl-build recompile) + `[golden]` (door reddens) + `[review]`
      (E18 unaffected): `content/dev/maps/spawner-test.map` recompiles with the alarm light
      as a baked animated `light_spot`; the authored-red capture scene in
      `capture-animated-direct-receiver-goldens` AC2 confirms the closet door reddens inside
      the cone, and the E18 spawner behavior (enemies spawn on the floor and walk out) is
      unaffected.
- [ ] **AC11** `[review]` `context/lib/rendering_pipeline.md` §4 (new paragraph + the
      receiver matrix), `context/lib/build_pipeline.md` PRL section table (id 45), and
      the FGD comment on the baked-`Light` base class are updated per Task 5.

## Tasks

### Task 1: Wire format — `AnimatedDirectShDeltaVolumes` (id 45)

Add PRL section `SectionId::AnimatedDirectShDeltaVolumes = 45` in
`crates/level-format/src/lib.rs` (append after `TriggerVolumes = 44`; update
`from_u32`). Add module `animated_direct_sh_delta_volumes.rs` with struct
`AnimatedDirectShDeltaVolumesSection`, mirroring `DeltaShVolumesSection` (id 27)
field-for-field — its eight stored fields `affinity_factor`, `affinity_dims`,
`tile_dimension`, `tile_border`, `animation_descriptor_indices`, `affinity_offsets`,
`affinity_lights`, `delta_subblocks` (dense 64-probe RGBA16F octahedral sub-block per
CSR entry) — plus a section-internal `u8` version constant
`ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION = 1`, written as the first payload byte
exactly as id-27's `DELTA_SH_VOLUMES_VERSION: u8` is (a true field-for-field mirror).
Note `delta_probe_f16_stride` is a **derived method** on id 27, not a stored field —
carry it as the same derived accessor, do not serialize it. Section 45 serializes its
**own** `animation_descriptor_indices` copy (it does not reference section 27's, so it
loads independently of whether section 27 is present); the copy is keyed by
`AnimatedBakedLights` index, independent of the affinity CSR layout. `affinity_lights`
entries are `AnimatedBakedLights` indices (same space section 27 uses), NOT selection
indices. Wire encode/decode + round-trip test.
Add loader validation in `crates/level-loader/src/prl_loader.rs`. Mirror the **id-27**
`validate_delta_sh` for the self-describing struct decode (NOT `validate_direct_sh_delta`,
whose base-grid + selection-count cross-checks section 45 does not share), but wire the
loader block with the **id-41 soft-drop** pattern — warn + clear, never id-27's hard `?`
(a hard error would brick the load and violate AC 8). Validate internal consistency only, using bounds the section carries in itself (the
loader has no cross-section `AnimatedBakedLights` count to check against): CSR
`affinity_offsets` monotone with trailing total equal to `affinity_lights.len()`, and
every `affinity_lights` entry `< animation_descriptor_indices.len()` (the in-section
per-animated-light index space). Do **not** add a hard check that
`animation_descriptor_indices` length equals some external light count — no in-section
source exists for it. Cross-section descriptor resolvability is likewise not a hard load
check — an out-of-range descriptor index is a no-op at runtime via the shader's existing
`INVALID_DESCRIPTOR_INDEX` (`0xffffffff`) sentinel guard. Malformed or partial →
drop the whole section (animated-direct disabled). Tests: wire encode/decode + round-trip
test; a loader soft-drop test feeding malformed/partial section-45 bytes, asserting the
section is cleared and the load still succeeds (delivers AC 8).

### Task 2: Compiler bake — animated direct-SH delta

Add module `crates/level-compiler/src/animated_direct_sh_bake.rs` (new file — do
**not** extend the 2137-line `direct_sh_bake.rs`). Key on the existing
`AnimatedBakedLights` namespace (`light_namespaces.rs`, `!is_dynamic &&
animation.is_some()`). For each animated baked light, per reaching probe, bake its
**direct** unit-radiance transport reusing the same `pub(crate)` primitives the
indirect delta bake (`delta_sh_bake.rs`) reuses: `sh_bake::bake_probe_direct_rgb`
with the single light (it takes a `light_global_indices: &[u64]` arg the indirect variant
lacks — pass the light's global index as a one-element slice to seed soft-visibility),
then `sh_bake::pack_octahedral_irradiance_tile`. Occlusion
(against the static-geometry BVH) and the cosine-lobe convolution happen **inside**
`bake_probe_direct_rgb` — the `soft_visibility`/`segment_clear`/`apply_cosine_lobe_rgb`
helpers are module-private (`sh_bake.rs`/`lightmap_bake.rs`) and reached through it,
not called directly. Unit-radiance means the bake omits the light's authored
intensity/color (the runtime descriptor applies them once); bake at the light's
**authored cone direction** (rest). Reach-cull with `decompose_affinity_for_lights`
(falloff-sphere AABB + portal reach — no cone clipping at this stage); this yields the
same per-light cell set `decompose_affinity` (the id-27 bake's thin wrapper over the
same function) would produce for this light, not a tighter reach — the call-site
difference is a single-light slice vs. the full animated-light envelope, not a
narrower reach test. Cone/occlusion narrowing happens per-probe inside
`bake_probe_direct_rgb` (it zeroes out-of-cone probes' radiance), not at the
CSR/affinity level. Emit section 45
with its own `animation_descriptor_indices` (same `AnimatedBakedLights` index space
section 27 uses). Wire into `pipeline.rs` to run whenever `AnimatedBakedLights` is
non-empty. **Own the emission seam:** the section is only written to the `.prl` by
`pack::build_prl` (`crates/level-compiler/src/pack.rs`) — add a new
`Option<&AnimatedDirectShDeltaVolumesSection>` parameter to `build_prl`, thread the baked
section from `pipeline.rs` into it, and add a matching
`append_optional_section(SectionId::AnimatedDirectShDeltaVolumes, …)` call, mirroring how
`delta_sh_volumes` (id 27) and `direct_sh_delta_volumes` (id 41) are threaded and appended
there. Without this the section is baked but never serialized, and every downstream loader
and golden test silently sees an absent section. Tests: bake determinism (seeded soft visibility); per-light separability
(single-light sub-block equals that light's share); **no-double-count** — a
script-animated baked light produces a section-45 entry and contributes nothing to
the `DirectShVolume` base atlas or `EntityShadowLights` selection (delivers AC 3);
**direction-safe** — a direction-animated light bakes a clean rest-direction delta
with no panic or NaN (delivers half of AC 5).

### Task 3: Renderer — animated-direct additive compose

**Why a second pass, and why the structure is chosen at load time.** A single extended
`direct_sh_compose` would bind 10 storage buffers in the compute stage — the existing 4
(promotion `delta_subblocks` @20, `affinity_offsets` @21, `affinity_lights` @24,
`selection_weights` @26) plus section-45's 4 buffers and 2 shared — over the fixed
`max_storage_buffers_per_shader_stage = 8` the renderer must not raise (§10). Split the
animated addition into a second pass. The single-vs-pair choice is made at **load time**
by whether section 45 is present, so binding 15 is always written and promotion-only
maps stay byte-for-byte unchanged. (`direct_composed_atlas` @1 is a storage *texture*,
off the buffer budget.)

- **Case 1 — section 45 absent (promotion-only or no-direct map).** Exactly today: one
  `direct_sh_compose` pass reads `direct_base_atlas_view` (BC6H base) and writes+clamps
  `direct_composed_storage_view` — the storage view of the final texture the sampled
  `direct_atlas_view` (binding 15) reads. No intermediate, no Pass B, no new dispatch.
  Byte-for-byte unchanged (delivers AC 7).

- **Case 2 — section 45 present.** Allocate one extra intermediate `Rgba16Float` texture
  with a storage view (`direct_intermediate_storage_view`, Pass A write) and a sampled
  view (`direct_intermediate_sampled_view`, Pass B read). Pass A and Pass B are an
  **atomic pair** — always dispatched together, so binding 15 is written every time Pass
  A runs.
  - **Pass A** — the existing `direct_sh_compose` shader, unchanged; only its output bind
    group is repointed to `direct_intermediate_storage_view`. Writes `base − Σ_promo`
    (4 storage buffers). **Its construction gate must widen.** `DirectShComposeResources::new`
    today returns `disabled()` unless the id-35 base and id-41 promotion delta are present,
    so a 45-only map (AC 6) would have no Pass A and no intermediate producer. Construct
    Pass A whenever the composed atlas is needed — id 35 **or** id 41 **or** section 45
    present — tolerating missing inputs: an absent id-35 base binds the existing dummy-zero
    direct texture (the `has_direct == false` path) so `base = 0`, and an absent id-41 delta
    means `Σ_promo = 0`. On a 45-only map Pass A thus writes a zero intermediate onto which
    Pass B adds `Σ_anim`.
  - **Pass B (new)** — `direct_sh_compose.rs` + `direct_sh_compose.wgsl` gain a sibling
    pass reading `direct_intermediate_sampled_view` and writing+clamping
    `direct_composed_storage_view` (binding 15's texture) as
    `clamp(intermediate + Σ_anim(animated_light_scale_j(t) × anim_delta_j), ≥ 0)`. Its 6
    storage buffers, within budget: section 45's **own** `delta_subblocks`,
    `affinity_offsets`, `affinity_lights`, and its **own** `animation_descriptor_indices`
    (built from the section-45 copy — the indirect `sh_compose`'s
    `animation_descriptor_indices` is section-27-specific and would misresolve a 45-only
    map), plus the genuinely shared `descriptors` @22 and `anim_samples` @23 threaded from
    `ShVolumeResources.animation` (`sh.animation.descriptors`/`anim_samples`). 22/23 are
    free in the compose group, so `animated_light_scale` ports **verbatim** from
    `sh_compose.wgsl` (with `curve_eval.wgsl` appended); it applies
    `intensity × (base_color | color curve) × brightness` once, gated by `is_active`. When
    Σ_anim is zero this frame (all descriptors inactive), Pass B clamps+copies the
    intermediate to final. Use a sampled read of the intermediate (no read_write storage
    textures — not downlevel-safe).

Plumbing:
- **Atlas dimensions.** The composed/intermediate atlas and the compose grid uniform
  currently source dimensions from `DirectShVolumeSection` (id 35). Section 45 (mirroring
  `DeltaShVolumes`) carries none, so for a 45-only map derive them from the
  `OctahedralShVolume` base grid (id 34) — whose direct-atlas layout id 35 already matches
  byte-identically, and which is always present when section 45 is (its delta tiles are
  keyed to id-34 base probes).
- **`has_direct` + allocation.** Widen to true when section 35 **or** 45 is present. Case
  2 allocates the intermediate texture in `ShVolumeResources::new`; Case 1 does not.
- **Section-45 threading.** Thread the decoded `AnimatedDirectShDeltaVolumesSection` from
  the loader through `World` into `ShVolumeResources::new` / the Pass-B resource, mirroring
  how the id-41 `direct_delta_section` param is threaded into `DirectShComposeResources::new`.
- **Dispatch.** The `active` arg of `direct_compose_should_dispatch` (`(active,
  pending_copy_through, was_active)`) is computed at the renderer frame call site
  (`renderer_render_frame.rs`, where `promoted_static_weights` currently feeds it). Widen
  that computed `active` to also cover "any section-45 descriptor active" — a **new CPU
  accessor** over the section-45 descriptor-index set (`AnimatedLightBuffers` has no
  any-active query today): it iterates section-45's `animation_descriptor_indices` and
  tests each resolved descriptor's `is_active` against the same shared `descriptors`
  state the compose pass reads (the state `AnimatedLightBuffers` receives the
  section-45 index set into). The Case-2 pair shares this one predicate — both passes fire
  together, including the initial copy-through. Case 1 keeps the unchanged promotion-only
  predicate. A 45-present map with all descriptors inactive and no promotion change does not
  dispatch (final retains its last value); a 45-absent map dispatches nothing beyond today's
  cadence (delivers AC 6).
- **CPU scale seam + lifecycle test.** No CPU `animated_light_scale` exists in `render-cpu`
  today; author one as the single source the WGSL `animated_light_scale` traces to (same
  `intensity × (base_color | color curve) × brightness`, `is_active` gate), and add a
  headless test (no GPU) over it covering each descriptor state: initial-active,
  initial-inactive (`is_active == 0` → 0), looping mid-cycle, one-shot settle, cleared,
  despawn/reload — delivers AC 4's per-state scale math. The GPU-visible "no brightness pop"
  and "despawn/reload drops cleanly" are integration properties left to the manual-GPU /
  review gate (see AC 4), not this unit test.
- The mover/skinned/billboard consumers are unchanged — they sample `direct_atlas_view`
  at binding 15.

### Task 4: Diagnostics

Add a dev-tools isolation for the animated-direct contribution: a **new Pass-B override**
uniform buffer (like the existing `DebugOverride`, not a storage buffer — it stays off the
compute stage's storage-buffer budget so Pass B remains at 6 of 8), modeled on the existing
debug-override shape (Rust `DirectShDebugOverride` / WGSL `DebugOverride`), keyed by
`AnimatedBakedLights` index, isolating one animated light's added term. It is a second
override living on Pass B — not the same binding-27 buffer, which is promotion-selection-keyed
on Pass A. Confirm `POSTRETRO_GPU_TIMING=1` attributes the
new Pass B to a timing bracket — extend the existing `direct_sh_compose` bracket to
span both passes, or add a sibling bracket (§12). Extend the
forward/mesh lighting-isolation modes only if the existing direct-SH isolation mode
does not already cover the composed atlas (it does — verify, don't duplicate).

### Task 5: Fixture + docs

Convert `content/dev/maps/spawner-test.map` entity 7 from `light_dynamic_spot` to a
baked animated `light_spot` (keep `_tags "alarm_light"`, set `_cone`/`_cone2`/`angles`
so the closet-door mover sits inside the cone, set `light`/`_falloff_range`). The
`turnRed` `setLightAnimation` reaction in `content/dev/scripts/spawner-test.ts` is
unchanged (queries `component: "light"`, matches the spot). Add two more dynamic
receivers inside the same cone so AC2 exercises all three receiver classes: a `prop_mesh` (skinned-mesh
receiver — set `model` to an existing dev asset, e.g.
`models/decraniated_low_poly_retro_pixel/scene.gltf`, and `origin`/`angles` so it stands
in the cone beside the door) and a `billboard_emitter` (billboard receiver — `sprite`
`smoke_puff` or similar, `origin` in the cone). Cone/aim tuning so all three receivers
sit inside the cone is ordinary Task-5 detail; keep both new receivers clear of the
`entity_spawner` origin and the doorway walk-out path so the E18 spawner behavior (AC10)
stays unaffected. Recompile the `.prl` (command in the map
header). The later authored-red `spawner-test` capture scene in
`capture-animated-direct-receiver-goldens` AC2 checks the door and prop_mesh with the
cone-lit wall, while its no-authored-state scene in AC3 records the baked rest baseline.
The billboard stays a **manual-GPU check**: run the engine on `spawner-test.prl`, fire the
closet plate, and confirm its particle-produced sprite reddens with the wall. That same
manual-GPU run also confirms: (AC4) no brightness pop on one-shot settle, the settled
light holds its final keyframe, and despawn/reload drops the contribution cleanly; (AC5)
brightness and RGB color visibly pulse the receivers; (AC6) a 45-only map (no static
`DirectShVolume`) still allocates the composed atlas and lights the movers. At promotion, update
`context/lib/rendering_pipeline.md` §4 (new "Animated direct SH for dynamic
receivers" paragraph + the receiver matrix; also correct §4's sampler list, which
currently names only skinned meshes and billboards, to include the kinematic mover as
a direct-SH atlas (binding 15) sampler), `context/lib/build_pipeline.md` PRL
section table (id 45), and the FGD comment on the baked-`Light` base class noting
that script-animated baked lights now reach moving receivers' direct term. Add one
line of authoring guidance (FGD comment and/or `docs/`): pulse/color animation →
baked animated light (cheap, reaches movers via this feature); a cone that must
sweep its direction across movers → author `light_dynamic_spot` (the dynamic tier
owns live-direction cones on moving geometry).

## Sequencing

**Phase 1 (sequential):** Task 1 — the wire format blocks the bake and the renderer.
**Phase 2 (concurrent):** Task 2 (compiler bake), Task 3 (renderer compose) —
independent modules, both build against Task 1's format; Task 3 develops against the
section shape and consumes Task 2's output only at runtime.
**Phase 3 (concurrent):** Task 4 (diagnostics), Task 5 (fixture + docs) — both
consume the shipped runtime behavior from Phase 2.

## Boundary inventory

| Name | Rust | Wire / serde | WGSL | FGD KVP |
|---|---|---|---|---|
| Section id | `SectionId::AnimatedDirectShDeltaVolumes` | `45` (u32) | n/a | n/a |
| Section struct | `AnimatedDirectShDeltaVolumesSection` | mirrors `DeltaShVolumesSection` | n/a | n/a |
| Light namespace | `AnimatedBakedLights` (`!is_dynamic && animation.is_some()`) | n/a | n/a | baked `Light` + `setLightAnimation` reservation |
| Bake module | `animated_direct_sh_bake::bake_animated_direct_sh_delta_volumes` | n/a | n/a | n/a |
| Descriptor (per-frame) | `pack_compose_animation_descriptor` output; shared `descriptors`/`anim_samples` from `sh.animation` | shared 48-byte `AnimationDescriptor` | `AnimationDescriptor` (@22/@23) | authored via `setLightAnimation` |
| Descriptor index map | section-45's **own** `animation_descriptor_indices` (not id 27's) | serialized in section 45 | Pass B storage buffer | n/a |
| Curve modulation | (CPU curves in `anim_samples`) | `f32` sample buffer | `animated_light_scale` / `sample_*_catmull_rom` | brightness / color / (direction rest) |
| Final composed atlas | `direct_composed_storage_view` (Pass write) / `direct_atlas_view` (sampled) | n/a | `sh_direct_atlas` (binding 15) | n/a |
| Intermediate atlas (Case 2) | `direct_intermediate_storage_view` (Pass A) / `direct_intermediate_sampled_view` (Pass B) | n/a | Pass A output / Pass B input | n/a |
| Atlas dimensions | id 35 layout; id 34 base grid when 45-only | from `DirectShVolume`/`OctahedralShVolume` | n/a | n/a |
| Direct gate | `has_direct` (widen to 35 ∨ 45) | n/a | `DynamicDirectParams.has_direct` (binding 16) | n/a |
| Dispatch predicate | `direct_compose_should_dispatch` (`active` widened to include any active section-45 descriptor; Case-2 pair shares it) | n/a | n/a | n/a |

## Wire format

`AnimatedDirectShDeltaVolumes` (id 45) mirrors `DeltaShVolumes` (id 27) exactly:
little-endian; a section-internal `u8` version byte (first payload byte, as id 27),
`ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION = 1`;
`affinity_factor = 4`, `affinity_dims = ceil(base grid_dimensions / 4)`;
`tile_dimension = 6`, `tile_border = 1`; its own `animation_descriptor_indices`
(keyed by `AnimatedBakedLights` index, independent of section 27); CSR
`affinity_offsets` length `affinity_cell_count + 1` (trailing total); flat
`affinity_lights` grouped by cell, entries are **`AnimatedBakedLights` indices**;
`delta_subblocks` one dense 64-probe `Rgba16Float` (f16×4) octahedral tile sub-block
per CSR entry, x-fastest in-cell order `local = lx + ly*4 + lz*16`, index-parallel to
`affinity_lights`. The per-probe f16 stride is derived (a method, as on id 27), not
serialized.
Empty list encodes as a zero-count CSR
(`affinity_offsets = vec![0; affinity_cell_count + 1]`, empty
`affinity_lights`/`delta_subblocks`) — the loader treats it as "no animated direct."
The section is emitted only when `AnimatedBakedLights` is non-empty. It mirrors id 27
for the payload shape and id 41 for the compose-pass integration, so no new
octahedral/tile convention is introduced.

## Script syntax examples

No new SDK surface. An author animates a baked light exactly as today; the direct
term now reaches movers automatically. The fixture's existing reaction (unchanged):

```typescript
// spawner-test.ts — alarm_light is a baked animated light_spot after the fixture change
const alarmLights = world.query({ component: "light", tag: "alarm_light" });
const turnRed = defineReaction("closet.turnRed", {
  sequence: alarmLights.map((light) => ({
    id: light.id,
    primitive: "setLightAnimation" as const,
    args: { periodMs: 200, phase: null, playCount: 1, startActive: true,
            brightness: null, color: [{ x: 1, y: 0, z: 0 }], direction: null },
  })),
});
// The door (kinematic_mover) inside the cone now reddens with the wall on fire.
```

## Decisions grounded in project goals

Three questions that first read as open resolve once measured against the project's
own values — theatrical scripted reveals as first-class, lean/baked-over-computed,
and "static-vs-dynamic is an authoring choice, not an engine rule." Each is a
decision, not a deferral.

- **Fixture: convert entity 7 in place** to a baked animated `light_spot`. The
  fixture *is* a monster-closet set-piece, and the northstar makes those first-class;
  a single alarm light that is the animated thing is the honest set-piece and the
  natural authoring pattern a modder should copy. A second tag-sharing light is an
  engine-testing artifact that would also make `setLightAnimation` target both. The
  E18 spawner assertions are light-tier-agnostic, so nothing blocks the conversion.

- **CSR: bake an independent affinity-CSR for section 45**, not shared with the
  indirect delta's id-27 CSR. Its cell set equals id-27's — both decompose via the
  same falloff-sphere AABB reach test (`light_aabb` in `affinity_grid.rs`, no cone
  clipping); cone/occlusion narrowing happens per-probe inside the bake, not at the
  CSR level, so the composed result stays correct because out-of-cone probes bake to
  ~zero. The independence is justified by **load-independence from section 27**
  (section 45 loads and composes correctly whether or not section 27 is present) and
  by keeping the bake a field-for-field mirror of `DeltaShVolumesSection` — not by a
  smaller VRAM footprint. Cone-clipped reach tightening (skipping falloff cells the
  occlusion-tested cone never lights) remains an available future optimization if a
  measured footprint ever demands it — v1 does not pursue it.

- **Direction animation on movers is the dynamic tier's job, by design** (see Out of
  scope). Not a limitation to close later: a live-direction cone raking moving
  geometry is authored `light_dynamic_spot`, which already works; the baked animated
  tier owns the pulse/color vocabulary. The docs task carries this as one line of
  authoring guidance rather than a backlog item.

- **Compose: two passes, not a merged single-pass CSR** (the storage-buffer choice in
  Task 3). The renderer's values decide it, not just the 8-buffer limit. What a single
  merged pass would save — one dispatch and one probe-atlas-sized intermediate texture
  — is below the manual perf-check floor: screen-sized fragment and shadow-map work
  dominate the GTX 1660 / Radeon Pro 5500M envelope, not a small pre-frame compute
  dispatch. The merge spends what the renderer treats as scarce: it pins the compose
  stage at exactly 8 storage buffers (zero headroom for queued lighting features) and
  abandons the id-27 field-for-field CSR mirror. Two passes conform to the established
  compose-pass idiom (the indirect `sh_compose` already runs every frame), preserve
  headroom, and keep the merge as a measured-pivot escape — consolidate later if a
  profile ever shows dispatch count matters (it won't at probe-atlas size); a shipped
  merged CSR can't be un-shipped without a wire migration. Same measure-before-
  optimizing discipline as clustered-forward and global-BVH deferral.

No residual open questions block implementation. Fixture cone/aim tuning is ordinary
Task-5 implementation detail.
