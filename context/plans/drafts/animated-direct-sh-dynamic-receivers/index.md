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
- Renderer: extend the existing `direct_sh_compose` pass to **add** the animated
  term, per-frame, modulated by the shared compose animation descriptors — the
  same buffer `lm_anim` and the indirect SH delta already consume.
- Receivers: kinematic movers (required), skinned meshes, and billboards — all
  three already sample the composed direct atlas at binding 15, so the fix is
  producer-side only.
- Widen the `has_direct` gate and composed-atlas allocation so a map with only
  animated baked lights (no static `DirectShVolume`) still gets a direct atlas.
- Fixture: align `content/dev/maps/spawner-test.map` with the feature's premise —
  the tagged `alarm_light` becomes a baked animated `light_spot` in whose cone the
  closet-door mover sits.

### Out of scope

- **Promoting animated lights into the runtime shadow-map pool.** Kept separate by
  design (`is_promotable_base_light` already excludes `is_animated`/`animation.is_some()`).
  v1 delivers the baked static-occlusion SH quality tier only; runtime shadow-map
  quality for animated lights is a deliberate future escalation, not a v1 handoff.
- **Direction-animated mover direct.** The baked direct-SH delta encodes the light's
  authored (rest) cone direction. Brightness and RGB color animate the delta; a
  swept cone changes spatial coverage a fixed baked tile cannot represent, so
  direction animation does not rotate the mover-direct term in v1 (world `lm_anim`
  and indirect SH keep their own direction behavior). Documented limitation.
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
   writes the animated term they receive it for free. Billboards evaluate direct SH
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
   `active_without_animation`). No double-count: the light is absent from the
   `DirectShVolume` base (`StaticBakedLights` filters `animation.is_none()`), absent
   from the dynamic-direct `lights` buffer (baked lights never enter it), and absent
   from promotion selection (`is_promotable_base_light` excludes animated). The
   animated direct delta is its sole mover-direct source, by construction.

6. **Budgets.** One extra additive loop in the already-dispatched `direct_sh_compose`
   pass, over the direct probe atlas array (small — probe-atlas sized, not
   screen-sized). Dispatch cadence widens to run every frame while any animated
   baked light is active (curves change per frame), matching the indirect
   `sh_compose` cadence (which already runs unconditionally). **Zero-animated-light
   maps:** section 45 absent → the animated loop binds an empty CSR and the pass
   keeps its current promotion-only cadence (copy-through + while-any-weight-nonzero
   + settle). GPU memory: one new sparse f16 section, same footprint class as the
   indirect delta, clipped to each light's occlusion-tested direct reach (tighter
   than indirect bounce reach).

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
  direct_sh_compose (compute, pre-frame)
    composed = base(static direct, BC6H)
             − Σ_promo (selection_weight_i × promo_delta_i)   [id 41, existing]
             + Σ_anim  (descriptor_scale_j(t) × anim_delta_j) [id 45, NEW]
    clamp ≥ 0, write Rgba16Float composed direct atlas (binding 15)
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

- [ ] A kinematic mover fully inside a script-animated `light_spot`'s cone receives
      the light's animated direct illumination; when the alarm curve drives the
      light red, the mover reddens together with the adjacent static wall (no dark
      mover beside a lit wall).
- [ ] A skinned mesh and a billboard in the same cone receive the same animated
      direct term (shared composed atlas), with no per-receiver wiring beyond the
      compose pass.
- [ ] The animated light's direct is counted exactly once on each receiver: it is
      absent from the `DirectShVolume` base atlas, absent from the dynamic-direct
      light buffer, and absent from promotion selection — verified by a compiler
      test that a script-animated baked light produces an `AnimatedDirectShDeltaVolumes`
      entry and no `EntityShadowLights`/base-direct contribution.
- [ ] Script lifecycle on the mover-direct term: initial-active lights the mover
      from frame 0; initial-inactive (`startActive: false`) leaves it dark; a
      trigger-installed curve lights it on fire; a looping curve animates it each
      cycle; a one-shot settles and holds the final keyframe with no brightness pop;
      clearing (`setLightAnimation(null)`) holds authored radiance; despawn/reload
      drops the contribution cleanly.
- [ ] Brightness and RGB color animate the mover-direct term; direction animation
      does not rotate it (documented v1 limitation) and does not crash or corrupt
      the atlas.
- [ ] A map with **only** animated baked lights (no static `DirectShVolume`) still
      allocates a composed direct atlas, sets `has_direct`, and lights movers; a map
      with **zero** animated baked lights keeps the current promotion-only compose
      cadence and shows no per-frame compose dispatch attributable to this feature.
- [ ] Static world surfaces render unchanged (still `lm_anim`-based); a scene with
      no animated baked lights is byte-identical on the direct atlas to pre-change.
- [ ] Loader rejects a malformed or partial section 45 by cleanly disabling the
      animated-direct term (no crash), mirroring the id-41 all-or-nothing clear.
- [ ] `POSTRETRO_GPU_TIMING=1` attributes the animated additive cost to the existing
      `direct_sh_compose` bracket; a dev-tools control isolates the animated-direct
      contribution for inspection.
- [ ] `content/dev/maps/spawner-test.map` recompiles with the alarm light as a baked
      animated `light_spot`; the closet door visibly reddens inside the cone when the
      plate fires, and the E18 spawner behavior (enemies spawn on the floor and walk
      out) is unaffected.

## Tasks

### Task 1: Wire format — `AnimatedDirectShDeltaVolumes` (id 45)

Add PRL section `SectionId::AnimatedDirectShDeltaVolumes = 45` in
`crates/level-format/src/lib.rs` (append after `TriggerVolumes = 44`; update
`from_u32`). Add module `animated_direct_sh_delta_volumes.rs` with struct
`AnimatedDirectShDeltaVolumesSection`, mirroring `DeltaShVolumesSection` (id 27)
field-for-field — `affinity_factor`, `affinity_dims`, `tile_dimension`,
`tile_border`, `delta_probe_f16_stride`, CSR `affinity_offsets`, flat
`affinity_lights`, `delta_subblocks` (dense 64-probe RGBA16F octahedral sub-block
per CSR entry), and the `animation_descriptor_indices` map — with a section-internal
version constant. `affinity_lights` entries are `AnimatedBakedLights` indices (same
space section 27 uses), NOT selection indices. Wire encode/decode + round-trip test.
Add loader validation in `crates/level-loader/src/prl_loader.rs` (sibling to
`validate_direct_sh_delta`): every referenced descriptor index resolvable; malformed
or partial → drop the whole section (animated-direct disabled), never a hard error.

### Task 2: Compiler bake — animated direct-SH delta

Add module `crates/level-compiler/src/animated_direct_sh_bake.rs` (new file — do
**not** extend the 2137-line `direct_sh_bake.rs`). Key on the existing
`AnimatedBakedLights` namespace (`light_namespaces.rs`, `!is_dynamic &&
animation.is_some()`). For each animated baked light, per reaching probe, bake its
**direct** unit-radiance transport with the shared primitives already used by the
static direct bake: `sh_bake::bake_probe_direct_rgb` with the single light,
occlusion via `soft_visibility`/`segment_clear` against the static-geometry BVH,
`apply_cosine_lobe_rgb`, `pack_octahedral_irradiance_tile`. Unit-radiance means the
bake omits the light's authored intensity/color (the runtime descriptor applies them
once); bake at the light's **authored cone direction** (rest). Reach-cull with the
direct-reach `decompose_affinity_for_lights` (cone/falloff + portal reach), the same
tighter reach the base direct bake uses — not the broader indirect bounce reach.
Emit section 45 with `animation_descriptor_indices` parallel to section 27's, so the
runtime descriptor mapping is shared. Wire into `pipeline.rs` to run whenever
`AnimatedBakedLights` is non-empty. Bake determinism test (seeded soft visibility)
and a per-light-separability test (single-light sub-block equals that light's share).

### Task 3: Renderer — additive animated term in `direct_sh_compose`

Extend `crates/renderer/src/render/direct_sh_compose.rs` and
`crates/renderer/src/shaders/direct_sh_compose.wgsl`. In the shader, after the
existing promotion-subtraction loop, add an **additive** loop over the section-45
affinity cell: read the animated delta sub-block and multiply by
`animated_light_scale(light_index)` — port the helper verbatim from `sh_compose.wgsl`
(reads the shared compose `descriptors` + `anim_samples` + `curve_eval.wgsl`
`sample_curve_catmull_rom`/`sample_color_catmull_rom`, gated by `is_active`, applies
`intensity × (base_color | color curve) × brightness` once). Bind the new section-45
buffers + the shared compose descriptor/anim-sample buffers (already produced each
frame by the light bridge for `sh_compose`) at fresh, non-colliding bindings on the
compose BGL. Clamp `≥ 0` after both loops. Plumbing:
- Widen `has_direct` (and the `direct_composed_storage_view` allocation in
  `ShVolumeResources::new`, `render/sh_volume.rs`) to true when section 35 **or**
  section 45 is present; when only 45 is present, the base atlas is a zero atlas and
  the pass adds the animated term onto zero.
- Widen `direct_compose_should_dispatch` to also dispatch every frame while any
  section-45 descriptor is active — OR the animated-active predicate into the
  existing promotion predicate. Keep the promotion-only cadence when section 45 is
  absent.
- Feed the per-frame compose descriptor + `anim_samples` buffers into the compose
  pass. These already exist for the indirect `sh_compose`; thread the same handles.
- The mover/skinned/billboard consumers are unchanged — they already sample the
  composed `direct_atlas_view` at binding 15.

### Task 4: Diagnostics

Add a dev-tools isolation for the animated-direct contribution, reusing the existing
`DirectShDebugOverride` shape (`direct_sh_compose.wgsl` binding 27) so a single
animated light's added term can be viewed in isolation (parallel to the promotion
selection override). Confirm `POSTRETRO_GPU_TIMING=1` attributes the added loop to
the existing `direct_sh_compose` bracket (§12) — no new bracket needed. Extend the
forward/mesh lighting-isolation modes only if the existing direct-SH isolation mode
does not already cover the composed atlas (it does — verify, don't duplicate).

### Task 5: Fixture + docs

Convert `content/dev/maps/spawner-test.map` entity 7 from `light_dynamic_spot` to a
baked animated `light_spot` (keep `_tags "alarm_light"`, set `_cone`/`_cone2`/`angles`
so the closet-door mover sits inside the cone, set `light`/`_falloff_range`). The
`turnRed` `setLightAnimation` reaction in `spawner-test.ts` is unchanged (queries
`component: "light"`, matches the spot). Recompile the `.prl` (command in the map
header). Add a headless/frame-capture regression asserting the door fragment inside
the cone reddens with the wall after the plate fires. At promotion, update
`context/lib/rendering_pipeline.md` §4 (new "Animated direct SH for dynamic
receivers" paragraph + the receiver matrix), `context/lib/build_pipeline.md` PRL
section table (id 45), and the FGD comment on the baked-`Light` base class noting
that script-animated baked lights now reach moving receivers' direct term.

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
| Descriptor (per-frame) | `pack_compose_animation_descriptor` output | shared 48-byte `AnimationDescriptor` | `AnimationDescriptor` | authored via `setLightAnimation` |
| Curve modulation | (CPU curves in `anim_samples`) | `f32` sample buffer | `animated_light_scale` / `sample_*_catmull_rom` | brightness / color / (direction rest) |
| Composed atlas | `direct_atlas_view` / `direct_composed_storage_view` | n/a | `sh_direct_atlas` (binding 15) | n/a |
| Direct gate | `has_direct` (widen to 35 ∨ 45) | n/a | `DynamicDirectParams.has_direct` (binding 16) | n/a |
| Dispatch predicate | `direct_compose_should_dispatch` (widen) | n/a | n/a | n/a |

## Wire format

`AnimatedDirectShDeltaVolumes` (id 45) mirrors `DeltaShVolumes` (id 27) exactly:
little-endian; a section-internal `u32`/`u8` version prefix per the id-27 precedent;
`affinity_factor = 4`, `affinity_dims = ceil(base grid_dimensions / 4)`;
`tile_dimension = 6`, `tile_border = 1`; CSR `affinity_offsets` length
`affinity_cell_count + 1` (trailing total); flat `affinity_lights` grouped by cell,
entries are **`AnimatedBakedLights` indices**; `delta_subblocks` one dense 64-probe
`Rgba16Float` (f16×4) octahedral tile sub-block per CSR entry, x-fastest in-cell
order `local = lx + ly*4 + lz*16`, index-parallel to `affinity_lights`;
`animation_descriptor_indices` parallel to section 27's (same descriptor slots).
Empty list encodes as a zero-count CSR (`affinity_offsets = [0]`, empty
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

## Open questions

- **Fixture: convert in place vs. add a second light.** Recommendation: convert
  entity 7 in place to a baked animated `light_spot`. It keeps a single alarm light
  matching the narrative; the E18 spawner assertions are light-tier-agnostic. A
  second baked light sharing the `alarm_light` tag would have `setLightAnimation`
  target both and muddy the fixture. Decide before implementing Task 5.
- **CSR sharing with the indirect delta (id 27).** The animated indirect delta and
  the new animated direct delta key on the same `AnimatedBakedLights` set, but their
  affinity reach differs (indirect bounce reaches farther than the occlusion-tested
  direct cone). v1 bakes an independent, tighter direct-reach CSR for section 45.
  Sharing section 27's CSR would waste sub-blocks on bounce-only cells; revisit only
  if the two sections' combined footprint is measured to matter.
- **Direction-animated mover direct.** Left as a rest-direction approximation in v1.
  If a swept-cone mover-direct case arises, the escalation is either promotion (real
  runtime cone) or a small set of direction-keyframe-indexed deltas — both larger
  than this plan; do not fold into v1.
