# Per-Term Lighting Isolation

## Goal

Replace the two disjoint dev-tools lighting-debug dropdowns with one per-term
checkbox instrument (a bitmask) that gates each lighting term independently on
**every** draw path — world, entity, mover, sprite. Toggling a term shows
exactly which paths sample it, so designers and developers can see each term's
contribution to a scene directly instead of inferring it. Dev-tools only; the
default (all terms on) renders identically to today.

## Scope

### In scope

- A single `LightTermMask` bitmask replacing `LightingIsolation` (10-variant)
  and `DynamicDirectIsolation` (3-variant). One bit per canonical term.
- Per-term gating across all four draw paths: `forward.wgsl` (world),
  `skinned_mesh.wgsl` (entity), `kinematic_brush.wgsl` (mover),
  `billboard.wgsl` (sprite).
- Static-vs-animated separation of the baked terms:
  - World lightmap static vs animated — gated in-shader (already separable).
  - Indirect SH static vs animated — gated at compose time (`sh_compose`).
  - Baked direct SH static vs animated (entity/mover/sprite) — gated at compose
    time (`direct_sh_compose` / `animated_direct_sh_compose`).
- Debug-UI checkbox group replacing both ComboBoxes, in the existing "Lighting
  systems" header of the Lighting tab.
- Byte-layout assertion, gating, and parity tests; `rendering_pipeline.md`
  updates.

### Out of scope

- **Emissive term isolation.** Owner decision (2026-08-10): emissive is kept out.
  The instrument isolates terms that *light the scene* — every gated term is
  light a surface receives (ambient, indirect, baked/animated direct, dynamic,
  specular). An emissive surface lights **only itself**: it injects no light onto
  neighbors and does no GI (`emissive-surfaces-bloom` scope; the no-double-count
  boundary). So emissive is categorically outside the light-term set, not deferred
  work. It is landed and wire-able — the `_e.png` 4th material slot; `forward.wgsl`
  + `kinematic_brush.wgsl` add `emissive × material.emissive_strength` on world +
  mover only (absent on entity/sprite); demoable in `combat-demo.map` — but it
  belongs to a different category. Bit 7 stays reserved should that judgment
  change.
- **The SDF-shadow instruments** (`sdf_shadow_mode`, `sdf_force_visibility_one`,
  `sdf_shadow_flags`). They occupy separate uniform fields (`FrameUniforms`
  bytes 96..108) and their own UI controls, and gate shadow *visibility*, not
  lighting-term presence. Untouched.
- **Per-term scale.** The instrument is boolean per term. The existing
  `indirect_scale` and `dynamic_direct_scale` sliders stay as independent
  controls; this plan removes only the mode-coupled `indirect_scale`→1.0 forcing
  (see Rough sketch), not the sliders.
- **Fog-specific term gates.** Fog inherits the compose-isolated indirect atlas
  (it samples the composed SH total for ambient scatter) but gains no
  fog-specific checkboxes.
- **New PRL section or persisted state.** The mask is runtime dev-tools state;
  it is not baked, saved, or replicated.

## Direction

**Problem.** There is no single per-term lighting instrument that spans all
draw paths. Term isolation was built as forward/static-pass preset modes
(`LightingIsolation`), and the SH-lit paths were later given a separate, narrower
knob (`DynamicDirectIsolation`). On entities/movers/sprites the preset modes gate
only the runtime dynamic-direct term; the dominant baked SH terms answer to the
other instrument. So the mode dropdown transforms the world and leaves entities
looking the same — the observation that produced this plan.

**Prior commitments.**
- *No-double-count invariant* (`rendering_pipeline.md §4`): a physical light's
  contribution must never be double-counted on a receiver. The instrument is
  diagnostic-only and must not change the composed frame at the default mask.
  Preserved by making all-bits-on byte-identical to today (AC 1) and defaulting
  the compose mask to all-on so shipping/no-dev-tools builds are unaffected.
- *Latent separate-animated-delta term.* `forward.wgsl:902-903` notes mode 7
  (`AnimatedDeltaOnly`) "shows nothing useful intentionally … until Task E adds a
  separate animated-delta term." This plan delivers that separation and extends
  it to every path — an intended direction, not a divergence.
- *`DynamicDirectIsolation`'s purpose* was to keep the dynamic-vs-static parity
  comparison valid on the SH paths (`frame_uniforms.rs:156-157`). The new
  bitmask is a strict superset of that capability (independent static/animated
  indirect and direct bits), so retiring it loses no diagnostic power. Warranted:
  the 3 states (`Combined`/`DirectOnly`/`IndirectOnly`) are expressible as
  {indirect bits} × {direct bits} in the new mask.
- *Sampled-texture ceiling.* `REQUIRED_SAMPLED_TEXTURES = 16` is Metal's hard
  ceiling and is fully consumed (group 1 now carries diffuse, emissive, specular,
  normal; forward totals 16 with cube-array support — research.md). This
  forecloses binding a static base atlas alongside the composed atlas; SH
  static/animated separation must happen at compose time, where base and delta
  are still distinct inputs.

**Alternatives rejected.**
- *Extend the preset modes into the entity shaders* (keep `LightingIsolation`,
  add its `use_*` gates to `skinned_mesh`/`kinematic_brush`/`billboard`).
  Rejected: it forces per-class semantics for modes that have no entity analog
  (`LightmapOnly` on a lightmap-less entity), and presets can never show two
  terms at once or isolate one cleanly. Checkboxes are strictly more expressive
  and answer "which term contributes" directly.
- *Collapse static and animated into one indirect and one direct term.* Cheaper
  (no compose work) but blind to the animated contribution, which is exactly a
  term designers want to see; and it re-opens the uniform contract and the
  compose passes later to split them. Rejected against the "know the destination"
  aim.
- *Shader-side `composed − base` subtraction.* Impossible (base is compact/BC6H,
  not bound to render shaders, not texel-aligned; direct promotion has already
  subtracted static lights) and would blow the 16-texture ceiling even if it
  weren't. Rejected on both counts (research.md).
- *A second parallel instrument for entities.* Multiplies instruments — the very
  confusion that produced this plan. Rejected in favor of unification.

**Placement.** Renderer dev-tools instrument. The mask is engine-internal
(render-cpu `FrameUniforms` mirror + renderer uniforms + compose params); it is
**not** a scripting primitive or mod-authoring surface (no mod-facing contract),
and deliberately stays behind the diagnostics UI. The static/animated SH split
reaches the compose (compute) layer, so part of the instrument lives at the
renderer resource layer, not purely UI/shader — that placement is forced by the
not-separable-at-shader finding, not chosen.

**Foreclosures / one-way doors.** Low. Retiring the two enums replaces their
uniform semantics (a same-width `u32` → bitmask) — internal, dev-tools, no
persisted/wire/FGD surface. Undo cost is a revert of the UI + shader gates +
enum. Reserving bit 7 for emissive is the only cross-plan commitment. Nothing
material else.

## Acceptance criteria

- [ ] With all term bits set (the default), a scene renders byte-identical to
  the pre-change build on the world path (the group-0 128-byte stride is
  unchanged) and visually unchanged on the entity/mover/sprite paths — verified
  by the existing group-0 stride/offset tests plus a manual GPU A/B on a map with
  static, animated, dynamic, and emissive content (`combat-demo.map`).
- [ ] In a dev-tools build, toggling **Dynamic direct** off dims dynamic-lit
  world surfaces AND entities AND movers AND sprites; toggling it back on
  restores them.
- [ ] Toggling **Indirect — static** off removes the static SH indirect
  contribution on all four paths (and fog ambient scatter); **Indirect —
  animated** off removes the animated SH indirect contribution; each is
  independently observable on a map with animated lights.
- [ ] Toggling **Baked direct — static** off removes the world static lightmap
  AND the entity/mover/sprite static baked-direct-SH contribution; **Baked
  direct — animated** off removes the world animated lightmap AND the
  entity/mover/sprite animated-direct-SH contribution.
- [ ] Toggling **Ambient floor** off drops the constant fill on all four paths;
  a fully shadowed face with every other term off renders black.
- [ ] Toggling **Specular** off removes specular on world, mover, and sprite;
  skinned meshes are unchanged (they carry no specular term) — the unchanged
  result is the correct, informative answer.
- [ ] Changing the mask while paused re-composes the direct-SH atlas so the
  entity baked-direct change is visible without a level reload (the direct
  compose is not a per-frame pass; a mask change must dirty it).
- [ ] Only one lighting-term control is present in the Lighting tab; both former
  ComboBoxes are gone. No `LightingIsolation` or `DynamicDirectIsolation` symbol
  remains in the crate.
- [ ] The group-0 (128 B), group-2 mesh (16 B) and kinematic (32 B), and group-4
  `DynamicDirectParams` (16 B) strides are unchanged; their byte-layout
  assertion tests pass against the bitmask field.

## Tasks

### Task 1: Bitmask contract + world thin slice

Establish `LightTermMask` in `postretro-render-cpu` as a `u32` bitfield with the
bit vocabulary below, plus `ALL` (every wired bit set) and a `label()` per bit
for the UI. Replace `FrameUniforms.lighting_isolation` (the `LightingIsolation`
enum) with the mask `u32` at the same group-0 offset (bytes 88..92), keeping
`UNIFORM_SIZE == 128`. Add the mask to the `sh_compose` params so the indirect
compute pass can gate its accumulation. Wire one vertical slice end to end:
carry the mask in renderer state (replacing the `lighting_isolation` field);
write it into group-0 each frame; have `forward.wgsl` gate its in-shader terms
(ambient, world lightmap static via `lm_irr`, world lightmap animated via
`lm_anim`, dynamic, specular) by mask bits instead of the `use_*` mode
derivations; have `sh_compose` gate the static base vs the animated delta
accumulation (bit 1 vs bit 2) so `sh_total_atlas` carries only the selected
indirect terms; and replace **both** ComboBoxes ("Lighting Isolation" and
"Dynamic Direct Isolation") with one checkbox group driving the full mask (all
bits present). This slice crosses every seam — CPU mirror → group-0 uniform →
compose-pass uniform → render shader → UI — and must prove all-bits-on is
byte-identical to today. It does not yet touch the mesh/kinematic/billboard
shaders or the direct-SH compose; those consume the settled contract in Phase 2,
so the direct bits (3/4) do not yet visibly affect the entity/mover/sprite paths
until Task 4 — the checkbox exists from Task 1, its entity effect completes in
Task 4.
`sh_compose` runs unconditionally each frame, so no re-dispatch plumbing is
needed for the indirect gate; the mask reaches it through the same per-frame
write path that already uploads the compose params.

Bit vocabulary (`LightTermMask`):

| Bit | Term | Gate site |
|---|---|---|
| 0 | Ambient floor | in-shader (all paths) |
| 1 | Indirect — static | compose (`sh_compose`) |
| 2 | Indirect — animated | compose (`sh_compose`) |
| 3 | Baked direct — static | world: in-shader (`lm_irr`); entity/mover/sprite: compose (`direct_sh_compose`) |
| 4 | Baked direct — animated | world: in-shader (`lm_anim`); entity/mover/sprite: compose (`animated_direct_sh_compose`) |
| 5 | Dynamic direct | in-shader (all paths) |
| 6 | Specular | in-shader (world/mover/sprite; entity has none) |
| 7 | (reserved) Emissive | unwired |

### Task 2: Entity + mover SH paths

Migrate `skinned_mesh.wgsl` + `kinematic_brush.wgsl` and their group-2 params
(`MeshLightParams`, `KinematicLightParams`) and the shared group-4
`DynamicDirectParams` to the mask. Replace the `lighting_isolation` `u32` in both
group-2 structs with the mask `u32` at the same offset (8..12), preserving the
16 B (mesh) and 32 B (kinematic) strides. Gate ambient (bit 0) and dynamic
direct (bit 5) in both shaders by mask bits. Remove the in-shader
`dynamic_direct.isolation` SH branch from both shaders (`skinned_mesh.wgsl:626-631`,
`kinematic_brush.wgsl:358-363`): the SH indirect and baked-direct terms are now
isolated upstream in the compose atlases, so each shader samples the single
composed indirect and composed direct atlas unchanged. Retire the `isolation`
field from `DynamicDirectParams` (`render-cpu/src/sh_volume.rs`), reclaiming its
4 bytes as pad, keeping the 16 B stride; `scale` and `has_direct` stay. This is
one task because the group-4 struct is shared by both shaders — splitting it
would put two agents in the same byte contract.

### Task 3: Sprite path

Wire `billboard.wgsl` to the mask for its in-shader terms — ambient (bit 0),
dynamic direct / dynamic diffuse (bit 5), and static specular (bit 6) — which it
currently gates by nothing. Remove its `dynamic_direct_isolation` branch
(`billboard.wgsl:278-282`); its SH indirect + baked direct now arrive
pre-isolated from the compose atlases. Retire the `dynamic_direct_isolation`
`u32` from the group-0 tail (`FrameUniforms` bytes 112..116), reclaiming it as
pad and keeping `UNIFORM_SIZE == 128`. The billboard path reads the mask from the
same group-0 buffer forward does; it must read the mask field at bytes 88..92,
which it ignores today.

### Task 4: Direct-SH compose static/animated gate

Gate the direct-SH compose by the mask so the composed direct atlas (binding 15)
carries only the selected direct terms. Thread the mask into the
`direct_sh_compose` and `animated_direct_sh_compose` param uniforms.
`direct_sh_compose` starts its accumulator at the base (`base − Σ promoted·w`)
only when bit 3 is set, else zero; `animated_direct_sh_compose` adds the
animated-direct delta only when bit 4 is set. Because the direct compose is not a
per-frame pass (it dispatches on load / nonzero weight / return-to-zero), a mask
change must dirty a re-dispatch — add the mask to the dirty condition alongside
the existing weight triggers. Define the promotion-subtraction rule: the
`Σ promoted·w` subtraction that keeps a promoted static light from
double-counting against the runtime pool term applies only when the **Dynamic
direct** bit (5) is also set — with dynamic isolated off, the compensated-for
runtime term is absent, so the static-direct bit shows the full un-subtracted
baked direct.

### Task 5: Enum retirement, tests, docs

After all consumers are on the mask, delete the `LightingIsolation` and
`DynamicDirectIsolation` enums, their `ALL_VARIANTS`/`cycle`/`label`, and the
renderer state fields/setters/getters they backed. Update every byte-layout
assertion test to the bitmask field: `frame_uniforms.rs` CPU-offset tests
(88..92 mask; the retired 112..116 tail is now pad), `shader_tests.rs` group-0
stride (135) and billboard loop-bound (152), `mesh_pass.rs` (2093/2106),
`kinematic_brush.rs` (1120/1140), `sh_volume.rs` DynamicDirectParams layout
(1775). Add behavioral gating tests: each bit clears its term's contribution on
the applicable paths; all-bits-on equals the pre-change composition (parity).
Add a compose-gate test that a mask change dirties the direct-SH re-dispatch.
Update `rendering_pipeline.md` §4 (retire the "10 lighting isolation modes"
sentence and the `DynamicDirectIsolation` description; describe the unified
per-term mask and its compose-time vs in-shader gate split), §7.1 step 5 (the
compose passes honor the mask), and §9 (the mesh group-2/group-4 params carry the
mask, not the two isolation enums).

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice, falsifies the boundary
assumptions (bit vocabulary, `u32` stride preservation, UI→state→group-0+compose
flow, compose-time indirect gating, in-shader gating). Blocks everything.

**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — independent consumers of the
Phase 1 contract. Task 2 owns group-2 (mesh/kinematic) + group-4; Task 3 owns
the group-0 tail; Task 4 owns the direct-SH compose. No shared files across the
three.

**Phase 3 (sequential):** Task 5 — consumes the completed migration; removes the
enums only once no consumer references them, and lands the test + doc sweep
against the integrated result.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Default mask (all wired bits set) renders identically to pre-change | Task 1 (`ALL` default; group-0 offset unchanged) | Every task's gate must be a no-op at its bit set; compose default all-on | AC 1, AC 8 |
| Group-0 128 B / group-2 16 B & 32 B / group-4 16 B strides unchanged | Task 1, 2, 3 (same-width `u32` replacement; retired fields become pad) | Any field width change breaks the shared shader byte contract | AC 8 |
| No-double-count holds under isolation | Task 4 (promotion subtraction tracks bit 5) | Static-direct bit on + dynamic bit off must not subtract the absent runtime term | AC 4, AC 2 |
| One term = one contribution, gated once | Task 1 (compose for SH indirect), Task 4 (compose for SH direct), Tasks 1–3 (in-shader ambient/dynamic/specular/lightmap) | A term gated in two places would double-toggle; SH terms move fully to compose, removing the in-shader SH branch | AC 2, AC 3, AC 4 |

## Rough sketch

- `LightTermMask`: a `u32` newtype (or `bitflags`) in `render-cpu`, bits per the
  Task 1 table, `ALL` = every wired bit. Same 4 bytes as the retired
  `lighting_isolation` enum ordinal, so `build_uniform_data` and the group-2
  serializers change type, not offset.
- Forward in-shader gates become direct bit tests replacing `use_lightmap`
  /`use_indirect`/`use_specular`/`use_dynamic` (`forward.wgsl:901-906`). Split the
  former single `use_lightmap` into static (`lm_irr`, bit 3) and animated
  (`lm_anim`, bit 4). Drop the mode-coupled `indirect_scale`→1.0 forcing
  (`forward.wgsl:910`): with independent bits and a separate `indirect_scale`
  slider the user controls presence and scale independently.
- Compose gate (indirect): in `sh_compose.wgsl:260-283`, start `accum` at the
  base only when bit 1 is set, and run the delta accumulation loop only when bit
  2 is set. Mask arrives via the existing compose params uniform.
- Compose gate (direct): mirror in `direct_sh_compose.wgsl` (bit 3) and the
  `animated_direct_sh_compose` add pass (bit 4), per Task 4.
- UI: a set of `ui.checkbox` bound to mask bits, replacing the two ComboBoxes in
  the "Lighting systems" header (`debug_ui/mod.rs:320-323`). Checkboxes only, no
  presets (owner decision: with ≤8 terms, independent checkboxes make each term's
  contribution directly visible; presets would be overkill).
- The `POSTRETRO_*` env seeding pattern the old modes used (if any) maps to a
  default-`ALL` mask; a headless run keeps every term on.

## Resolved decisions

- **Checkboxes only, no presets** (owner, 2026-08-10). ≤8 terms; presets overkill.
- **Emissive stays out** (owner, 2026-08-10). Bit 7 reserved; see Out of scope.
- **Emissive has landed.** `emissive-surfaces-bloom` code is merged (the plan
  folder is still in `in-progress/`, a bookkeeping lag), so this plan is not
  blocked on it. Both edit `forward.wgsl` + `kinematic_brush.wgsl`, but emissive
  is already in the current source this plan is grounded against — no ordering
  dependency remains.
