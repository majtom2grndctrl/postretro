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
  closet-door mover sits.

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
   via the `active_without_animation` arg of `pack_animation_descriptor`). No
   double-count: the light is absent from the
   `DirectShVolume` base (`StaticBakedLights` filters `animation.is_none()`), absent
   from the dynamic-direct `lights` buffer (baked lights never enter it), and absent
   from promotion selection (`is_promotable_base_light` excludes animated). The
   animated direct delta is its sole mover-direct source, by construction.

6. **Budgets.** A second additive compute pass over the direct probe atlas array
   (small — probe-atlas sized, not screen-sized), split from the promotion compose to
   stay within the compute stage's storage-buffer budget (see Task 3). Dispatch
   cadence: the animated pass runs every frame while any animated
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
  direct_sh_compose (compute, pre-frame — two passes, Task 3)
    Pass A: interm = base(static direct, BC6H)
                   − Σ_promo (selection_weight_i × promo_delta_i)  [id 41, existing]
    Pass B: composed = interm
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
field-for-field — its eight stored fields `affinity_factor`, `affinity_dims`,
`tile_dimension`, `tile_border`, `animation_descriptor_indices`, `affinity_offsets`,
`affinity_lights`, `delta_subblocks` (dense 64-probe RGBA16F octahedral sub-block per
CSR entry) — plus a section-internal `u32` version constant (initial value `1`).
Note `delta_probe_f16_stride` is a **derived method** on id 27, not a stored field —
carry it as the same derived accessor, do not serialize it. Section 45 serializes its
**own** `animation_descriptor_indices` copy (it does not reference section 27's, so it
loads independently of whether section 27 is present); the copy is keyed by
`AnimatedBakedLights` index, independent of the affinity CSR layout. `affinity_lights`
entries are `AnimatedBakedLights` indices (same space section 27 uses), NOT selection
indices. Wire encode/decode + round-trip test.
Add loader validation in `crates/level-loader/src/prl_loader.rs` (sibling to
`validate_direct_sh_delta`): every referenced descriptor index resolvable; malformed
or partial → drop the whole section (animated-direct disabled), never a hard error.

### Task 2: Compiler bake — animated direct-SH delta

Add module `crates/level-compiler/src/animated_direct_sh_bake.rs` (new file — do
**not** extend the 2137-line `direct_sh_bake.rs`). Key on the existing
`AnimatedBakedLights` namespace (`light_namespaces.rs`, `!is_dynamic &&
animation.is_some()`). For each animated baked light, per reaching probe, bake its
**direct** unit-radiance transport reusing the same `pub(crate)` primitives the
indirect delta bake (`delta_sh_bake.rs`) reuses: `sh_bake::bake_probe_direct_rgb`
with the single light, then `sh_bake::pack_octahedral_irradiance_tile`. Occlusion
(against the static-geometry BVH) and the cosine-lobe convolution happen **inside**
`bake_probe_direct_rgb` — the `soft_visibility`/`segment_clear`/`apply_cosine_lobe_rgb`
helpers are module-private (`sh_bake.rs`/`lightmap_bake.rs`) and reached through it,
not called directly. Unit-radiance means the bake omits the light's authored
intensity/color (the runtime descriptor applies them once); bake at the light's
**authored cone direction** (rest). Reach-cull with the direct-reach
`decompose_affinity_for_lights` (cone/falloff + portal reach), the same tighter reach
the base direct bake uses — not the broader indirect bounce reach. Emit section 45
with its own `animation_descriptor_indices` (same `AnimatedBakedLights` index space
section 27 uses). Wire into `pipeline.rs` to run whenever `AnimatedBakedLights` is
non-empty. Tests: bake determinism (seeded soft visibility); per-light separability
(single-light sub-block equals that light's share); **no-double-count** — a
script-animated baked light produces a section-45 entry and contributes nothing to
the `DirectShVolume` base atlas or `EntityShadowLights` selection (delivers AC 3);
**direction-safe** — a direction-animated light bakes a clean rest-direction delta
with no panic or NaN (delivers half of AC 5).

### Task 3: Renderer — animated-direct additive compose (second pass)

**Why a second pass, not an extended loop.** A single extended `direct_sh_compose`
would bind 10 storage buffers in the compute stage — the existing 4 (promotion
`delta_subblocks` @20, `affinity_offsets` @21, `affinity_lights` @24,
`selection_weights` @26) plus section-45's CSR ×3 and the shared `descriptors` @22,
`anim_samples` @23, `animation_descriptor_indices` @25 — over the fixed
`max_storage_buffers_per_shader_stage = 8` the renderer must not raise (§10). Split
the animated addition into its own compute pass so each stays within budget. (Note:
`direct_composed_atlas` is a storage *texture*, not a storage buffer, so it is off
this budget.)

- **Pass A (unchanged):** the existing `direct_sh_compose` writes `base − Σ_promo`
  into an intermediate `Rgba16Float` composed view (4 storage buffers).
- **Pass B (new):** `crates/renderer/src/render/direct_sh_compose.rs` +
  `shaders/direct_sh_compose.wgsl` gain a sibling pass that samples Pass A's output
  and writes the **final** composed view (`direct_atlas_view`, binding 15) as
  `passA + Σ_anim(animated_light_scale_j(t) × anim_delta_j)`, clamped `≥ 0`. Its
  storage buffers (6, within budget): section-45 `delta_subblocks`/`affinity_offsets`/
  `affinity_lights` at fresh numbers, plus the shared `descriptors` @22,
  `anim_samples` @23, `animation_descriptor_indices` @25 — the **same handles** the
  indirect `sh_compose` binds (owned by the renderer's SH-compose resources; thread
  them in). Those three numbers are free in the compose group, so `animated_light_scale`
  ports **verbatim** from `sh_compose.wgsl` (with `curve_eval.wgsl` appended); it
  applies `intensity × (base_color | color curve) × brightness` once, gated by
  `is_active`. The intermediate composed view is one extra probe-atlas-sized texture
  (small); use a sampled read of it in Pass B to stay downlevel-safe (no read_write
  storage textures).

Plumbing:
- Widen `has_direct` (and both composed-view allocations in `ShVolumeResources::new`,
  `render/sh_volume.rs`) to true when section 35 **or** section 45 is present; when
  only 45 is present, Pass A's base is a zero atlas and Pass B adds onto zero.
- Widen `direct_compose_should_dispatch` so Pass B (and thus Pass A) dispatch every
  frame while any section-45 descriptor is active — OR the animated-active predicate
  into the existing promotion predicate. When section 45 is absent, Pass B is skipped
  entirely and Pass A keeps its current promotion-only cadence.
- No-animated / promotion-only maps must still bind valid buffers: Pass B is skipped,
  so its section-45 CSR and shared descriptor bindings need no dummy buffers; Pass A
  is byte-for-byte unchanged.
- Add a runtime lifecycle test (headless — assert at the descriptor→scale seam, no GPU
  context) covering the composed-atlas animated term for each descriptor state:
  initial-active, initial-inactive (`is_active == 0` → 0), looping mid-cycle, one-shot
  settle, cleared, despawn/reload — delivers AC 4. Confirm a scene with no animated
  baked lights leaves the direct atlas byte-identical to pre-change (delivers AC 7).
- The mover/skinned/billboard consumers are unchanged — they already sample the final
  `direct_atlas_view` at binding 15.

### Task 4: Diagnostics

Add a dev-tools isolation for the animated-direct contribution, reusing the existing
debug-override shape (Rust `DirectShDebugOverride` / WGSL `DebugOverride` at binding
27) so a single animated light's added term can be viewed in isolation (parallel to
the promotion selection override). Confirm `POSTRETRO_GPU_TIMING=1` attributes the
new Pass B to a timing bracket — extend the existing `direct_sh_compose` bracket to
span both passes, or add a sibling bracket (§12). Extend the
forward/mesh lighting-isolation modes only if the existing direct-SH isolation mode
does not already cover the composed atlas (it does — verify, don't duplicate).

### Task 5: Fixture + docs

Convert `content/dev/maps/spawner-test.map` entity 7 from `light_dynamic_spot` to a
baked animated `light_spot` (keep `_tags "alarm_light"`, set `_cone`/`_cone2`/`angles`
so the closet-door mover sits inside the cone, set `light`/`_falloff_range`). The
`turnRed` `setLightAnimation` reaction in `content/dev/scripts/spawner-test.ts` is
unchanged (queries `component: "light"`, matches the spot). Recompile the `.prl` (command in the map
header). Add a headless/frame-capture regression asserting the door fragment inside
the cone reddens with the wall after the plate fires. At promotion, update
`context/lib/rendering_pipeline.md` §4 (new "Animated direct SH for dynamic
receivers" paragraph + the receiver matrix), `context/lib/build_pipeline.md` PRL
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
| Descriptor (per-frame) | `pack_compose_animation_descriptor` output | shared 48-byte `AnimationDescriptor` | `AnimationDescriptor` | authored via `setLightAnimation` |
| Curve modulation | (CPU curves in `anim_samples`) | `f32` sample buffer | `animated_light_scale` / `sample_*_catmull_rom` | brightness / color / (direction rest) |
| Composed atlas | `direct_atlas_view` / `direct_composed_storage_view` | n/a | `sh_direct_atlas` (binding 15) | n/a |
| Direct gate | `has_direct` (widen to 35 ∨ 45) | n/a | `DynamicDirectParams.has_direct` (binding 16) | n/a |
| Dispatch predicate | `direct_compose_should_dispatch` (widen) | n/a | n/a | n/a |

## Wire format

`AnimatedDirectShDeltaVolumes` (id 45) mirrors `DeltaShVolumes` (id 27) exactly:
little-endian; a section-internal `u32` version prefix, initial value `1`;
`affinity_factor = 4`, `affinity_dims = ceil(base grid_dimensions / 4)`;
`tile_dimension = 6`, `tile_border = 1`; its own `animation_descriptor_indices`
(keyed by `AnimatedBakedLights` index, independent of section 27); CSR
`affinity_offsets` length `affinity_cell_count + 1` (trailing total); flat
`affinity_lights` grouped by cell, entries are **`AnimatedBakedLights` indices**;
`delta_subblocks` one dense 64-probe `Rgba16Float` (f16×4) octahedral tile sub-block
per CSR entry, x-fastest in-cell order `local = lx + ly*4 + lz*16`, index-parallel to
`affinity_lights`. The per-probe f16 stride is derived (a method, as on id 27), not
serialized.
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

- **CSR: bake the independent, tighter direct-reach index for section 45** (not
  shared with the indirect delta's id-27 CSR). This is leaner, not merely simpler:
  sharing would carry direct sub-blocks for bounce-only cells the occlusion-tested
  direct cone never reaches, spending VRAM against the compatibility-floor budget for
  no benefit. It also matches the codebase's measure-before-pivoting culture (global
  BVH, clustered-forward deferral): independent-tighter is the correct lean default;
  a shared index is the speculative variant to reach for only if a measured combined
  footprint ever demands it. Escape hatch noted, decision made.

- **Direction animation on movers is the dynamic tier's job, by design** (see Out of
  scope). Not a limitation to close later: a live-direction cone raking moving
  geometry is authored `light_dynamic_spot`, which already works; the baked animated
  tier owns the pulse/color vocabulary. The docs task carries this as one line of
  authoring guidance rather than a backlog item.

No residual open questions block implementation. Fixture cone/aim tuning is ordinary
Task-5 implementation detail.
