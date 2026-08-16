# Per-Term Lighting Isolation

## Goal

Replace the two disjoint dev-tools lighting-debug dropdowns with one per-term
checkbox instrument (a bitmask) that gates each lighting term independently on
**every** draw path — world, entity, mover, sprite — and in the volumetric fog
pass for the terms fog samples. Toggling a term shows exactly which paths sample
it, so designers and developers can see each term's contribution to a scene
directly instead of inferring it. Dev-tools only; the default (all terms on)
renders identically to today.

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
- Fog volumetric scatter participates in the terms it samples: dynamic spot-beam
  and point-light scatter gated by the Dynamic-direct bit (read from the group-0
  `Uniforms` the fog raymarch already binds); ambient scatter already tracks the
  indirect bits via the composed SH atlas (no fog change needed for those).
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
- **The shadow-visibility dev toggles** (`sdf_shadow_mode`,
  `sdf_force_visibility_one`, `sdf_shadow_flags`, and `spec_shadowmask_force_one`).
  They occupy separate `FrameUniforms` fields (bytes 96..108 and 124..128) and
  their own UI controls, and gate shadow *visibility*, not lighting-term presence.
  Untouched.
- **Per-term scale.** The instrument is boolean per term. The existing
  `indirect_scale` and `dynamic_direct_scale` sliders stay as independent
  controls; this plan removes only the mode-coupled `indirect_scale`→1.0 forcing
  (see Rough sketch), not the sliders.
- **Fog gates for terms fog does not sample.** Fog scatter is ambient (indirect)
  plus dynamic spot/point beams; it carries no ambient-floor, baked-direct, or
  specular term, so those bits have no fog effect. Fog's indirect and dynamic
  terms ARE gated (see In scope) — this bullet only marks the absent terms.
- **New PRL section or persisted state.** The mask is runtime dev-tools state;
  it is not baked, saved, or replicated. It lives in per-init renderer state
  (`full`), so a level reload, renderer full-init, or surface recreation resets
  it to `ALL` — rebuilding both the mask and the compose atlases at the default,
  consistently. The instrument is intentionally not preserved across those
  boundaries.

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
  by the group-0 stride/offset tests (updated for the bitmask field in Task 6)
  plus a manual GPU A/B on a map with static, animated, dynamic, and emissive
  content (`combat-demo.map`).
- [ ] In a dev-tools build, toggling **Dynamic direct** off dims dynamic-lit
  world surfaces AND entities AND movers AND sprites AND fog spot-beam +
  point-light scatter; toggling it back on restores them.
- [ ] Toggling **Indirect — static** off removes the static SH indirect
  contribution on all four paths; **Indirect — animated** off removes the
  animated SH indirect contribution; fog ambient scatter reads the composed
  indirect atlas, so it tracks **both** bits; each is independently observable on
  a map with animated lights.
- [ ] Toggling **Baked direct — static** off removes the world static lightmap
  AND the entity/mover/sprite static baked-direct-SH contribution; **Baked
  direct — animated** off removes the world animated lightmap AND the
  entity/mover/sprite animated-direct-SH contribution.
- [ ] Toggling **Ambient floor** off drops the constant fill on all four paths;
  a fully shadowed **non-emissive** face with every other term off renders black.
  (Emissive geometry stays self-lit under every mask state — emissive is not a
  gated term; see Out of scope.)
- [ ] Toggling **Specular** off removes specular on world, mover, and sprite;
  skinned meshes are unchanged (they carry no specular term) — the unchanged
  result is the correct, informative answer.
- [ ] Changing the mask while paused re-composes the direct-SH atlas so the
  entity baked-direct change is visible without a level reload (the direct
  compose is render-frame-driven, not sim-tick-gated; pause halts sim ticks only,
  and a checkbox toggle issues a redraw that produces the recomposing frame;
  `freeze_time` does not suppress the compose dispatch).
- [ ] Only one lighting-term control is present in the Lighting tab; both former
  ComboBoxes are gone. No `LightingIsolation` or `DynamicDirectIsolation` symbol
  remains in the crate.
- [ ] The group-0 (128 B), group-2 mesh (16 B) and kinematic (32 B), and group-4
  `DynamicDirectParams` (16 B) strides are unchanged; their byte-layout
  assertion tests pass against the bitmask field.

## Tasks

### Task 1: Bitmask contract + world thin slice

Establish `LightTermMask` in `postretro-render-cpu` as a `u32` bitfield with the
bit vocabulary below, plus `ALL` (every **wired** bit set = bits 0–6 = `0x7F`;
reserved bit 7 is excluded until wired) and a `label()` per bit for the UI.
Replace `FrameUniforms.lighting_isolation` (the `LightingIsolation` enum) with
the mask `u32` at the same group-0 offset (bytes 88..92), keeping
`UNIFORM_SIZE == 128`. Wire one vertical slice end to end: carry the mask in
renderer state (replacing the `lighting_isolation` field); write it into group-0
each frame; have `forward.wgsl` gate its in-shader terms (ambient, world lightmap
static via `lm_irr`, world lightmap animated via `lm_anim`, dynamic, specular) by
mask bits instead of the `use_*` mode derivations; have `sh_compose` gate the
static base vs the animated delta accumulation (bit 1 vs bit 2) so
`sh_total_atlas` carries only the selected indirect terms. The indirect compose
already binds the per-frame group-0 `FrameUniforms` bind group, so it reads the
mask from the **same group-0 field (bytes 88..92)** that forward reads — NOT from
`sh_compose`'s GridDims params, which are built once at construction and never
rewritten; sourcing the mask there would freeze indirect gating at
construction-time until a level reload. Then replace **both** ComboBoxes
("Lighting Isolation" and
"Dynamic Direct Isolation") with one checkbox group driving the full mask (all
bits present). This slice crosses every seam — CPU mirror → group-0 uniform →
`sh_compose` (reads group-0) → render shader → UI — and must prove all-bits-on is
byte-identical to today. It does not yet touch the mesh/kinematic/billboard
shaders or the direct-SH compose; those consume the settled contract in Phase 2,
so the direct bits (3/4) do not yet visibly affect the entity/mover/sprite paths
until Task 4 — the checkbox exists from Task 1, its entity effect completes in
Task 4.
`sh_compose` runs unconditionally each frame and rebinds group-0 each frame, so
no re-dispatch plumbing is needed for the indirect gate — the mask is already
live in the group-0 uniform it binds.

**One per-frame mask snapshot.** `update_per_frame_uniforms` (which builds group-0)
runs *before* the dev-tools egui UI mutates the mask field; the compute/entity
consumers run *after* the UI, inside scene recording. So a toggle would land one
frame ahead on any consumer that reads the live `full.light_term_mask` post-UI,
while group-0 readers still show the pre-toggle value — a one-frame cross-consumer
skew. Fix it structurally: capture the mask into one per-frame renderer-owned
field at `update_per_frame_uniforms` (the CPU value group-0 is built from), and
have **every** consumer this plan adds read that field, never the live
`full.light_term_mask` — group-0
(forward/billboard/`sh_compose`/fog/direct Pass B) already does by construction;
group-2 params (Task 2) and direct-compose Pass A + its dirty trigger (Task 4)
must read the same snapshot. A toggle then takes effect uniformly on the next
frame (atomic N+1), no skew. This mirrors the existing group-4
`write_dynamic_direct_params`, already written at `update_per_frame_uniforms`.

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
(`MeshLightParams` in `crates/renderer/src/render/mesh_pass.rs`,
`KinematicLightParams` in `crates/renderer/src/render/kinematic_brush.rs`) and
the shared group-4 `DynamicDirectParams` to the mask. Replace the
`lighting_isolation` `u32` in both group-2 structs with the mask `u32` at the
same offset (8..12), preserving the 16 B (mesh) and 32 B (kinematic) strides. The
value written is the per-frame mask snapshot (Task 1's "one per-frame mask
snapshot"), not the live `full.light_term_mask` — so the entity/mover in-shader
terms land on the same frame as the world path. Gate ambient (bit 0) and dynamic
direct (bit 5) in both shaders by mask bits.
**Split the mover specular lobe:** `kinematic_brush.wgsl` computes `blinn_phong`
specular *inside* `accumulate_dynamic_direct`, coupled to the dynamic term today;
gate that specular contribution by bit 6 **independently** of bit 5 (bit 5 gates
the whole runtime term, bit 6 gates only the specular lobe within it) so AC 6
holds on the mover. (Skinned meshes carry no specular term, so bit 6 is a no-op
there.) Remove the in-shader `dynamic_direct.isolation` SH branch from both
shaders (`skinned_mesh.wgsl:626-631`, `kinematic_brush.wgsl:358-363`): the SH
indirect and baked-direct terms are now isolated upstream in the compose atlases,
so each shader samples the single composed indirect and composed direct atlas
unchanged. Retire the `isolation` field from `DynamicDirectParams`
(`render-cpu/src/sh_volume.rs`), reclaiming its 4 bytes as pad, keeping the 16 B
stride; `scale` and `has_direct` stay. Retiring the field also changes the
`write_dynamic_direct_params(scale, isolation)` signature
(`crates/renderer/src/render/sh_volume.rs`; `has_direct` is `self.has_direct`, not
an argument) and its per-frame caller in `renderer_frame.rs` (which passes
`full.dynamic_direct_isolation as u32`) — drop the `isolation` argument at both. This is one task because the group-4 struct is shared by
both shaders — splitting it would put two agents in the same byte contract.

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
carries only the selected direct terms. `direct_sh_compose` Pass A (promotion)
starts its accumulator at the base (`base − Σ promoted·w`) only when bit 3 is
set, else zero; `animated_direct_sh_compose` Pass B adds the animated-direct
delta only when bit 4 is set.

**Mask channel (per pass).** Pass B already binds the per-frame group-0
`FrameUniforms` bind group, so it reads the mask from group-0 (bytes 88..92).
Pass A does **not** bind group-0 — its `@group(0)` is a private custom BGL — so
it cannot read the mask there; give Pass A a per-frame mask input co-located with
the dispatch (extend its per-frame `debug_override` uniform write, or add a
dedicated tiny per-frame uniform), written **before** the dispatch on every frame
it may run. The value written is the per-frame mask **snapshot** (Task 1's "one
per-frame mask snapshot"), the same value group-0 (Pass B, world `lm_irr`) reads
that frame — so bits 3/4 never skew from the world path. The load copy-through
dispatch (fires the first frame after construction) must compose against that
snapshot, not a stale default. Passes A and B are
dispatched together in one `dispatch_if_needed` call, so bits 3 and 4 always
apply in the same frame (pin this; never a frame where one applied and the other
lagged).

**Dirty trigger.** The direct compose is not per-frame (it dispatches on load /
nonzero promotion weight / return-to-zero). Add a mask term as an **inequality
against the last-composed mask**: cache `last_composed_mask` in the direct-compose
resources, dirty when `snapshot != last_composed_mask` (the same per-frame mask
snapshot, not the live field), and assign `last_composed_mask = snapshot` only
after a dispatch. This is a
level-triggered predicate, NOT `mask != ALL` — a re-check that returns the mask to
`ALL` must dirty like any other change, or the atlas sticks isolated forever while
the world in-shader bit restores instantly. Because it is level-triggered by
inequality, a mask change on a frame where the direct compose is skipped (e.g.
`render_world == false`) is not dropped — the next world frame recomputes the
inequality and dispatches.

**Promotion-subtraction rule.** The `Σ promoted·w` subtraction that keeps a
promoted static light from double-counting against the runtime pool term applies
only when the **Dynamic direct** bit (5) is also set — with dynamic isolated off,
the compensated-for runtime term is absent, so the static-direct bit shows the
full un-subtracted baked direct.

### Task 5: Fog dynamic-scatter gate

Gate the fog raymarch's dynamic spot-beam and point-light scatter by the
Dynamic-direct bit (5). The fog raymarch pipeline already binds the group-0
`Uniforms` bind group (`fog_pass.rs` group-0 = `camera_bgl`;
`renderer_render_frame.rs` binds `uniform_bind_group` to group 0 of the raymarch
pass) — `fog_volume.wgsl` simply does not *declare* `@group(0)` today. So read
the mask from group-0 like every other in-shader gate: declare a prefix
`Uniforms` struct in `fog_volume.wgsl` through byte 92 (eliding the tail, as the
fog composite already does for `FogParams`), and gate the spot loop
(`fog.spot_count`) and point loop (`fog.point_count`) by bit 5 — force the loop
bound to 0 when it is off, the way forward zeroes its loop counts when a term is
disabled — so the beams contribute nothing. This needs no `FogParams` field, no
`FOG_PARAMS_SIZE` change, and no `upload_params` or `renderer_render_frame.rs`
edit; Task 5 owns `fog_volume.wgsl` alone. Fog ambient scatter (indirect) is
untouched: it reads the composed `sh_total_atlas` (group 3), so bits 1/2 already
reach it at compose time. No dirty-tracking is needed — the fog raymarch runs per
frame and reads the group-0 mask snapshot (see Invariants: one per-frame mask
snapshot), landing atomically with the world path.

### Task 6: Enum retirement, tests, docs

After all consumers are on the mask, delete the `LightingIsolation` and
`DynamicDirectIsolation` enums, their `ALL_VARIANTS`/`cycle`/`label`, and the
renderer state fields/setters/getters they backed. Update every byte-layout
assertion test to the bitmask field (cite by test name, not line — they drift):
the `frame_uniforms.rs` CPU-offset tests (mask at 88..92; the retired 112..116
tail is now pad), the `shader_tests.rs` group-0 stride test (`Uniforms` span ==
`UNIFORM_SIZE`) and its billboard loop-bound test, `mesh_pass.rs`
(`mesh_light_params_is_sixteen_bytes`,
`write_light_params_places_ambient_floor_at_bytes_twelve_to_sixteen`),
`kinematic_brush.rs` (its byte-layout + WGSL-layout tests), and
`sh_volume.rs` `dynamic_direct_params_pack_layout` (`crates/render-cpu/src/sh_volume.rs`).
(Fog needs no byte-layout change — Task 5 reads the existing group-0 mask, not a
new `FogParams` field.) Add behavioral gating tests — each bit clears its
term's contribution on the applicable paths (including the Dynamic-direct bit
clearing fog spot/point scatter), and all-bits-on equals the pre-change
composition (parity) — run headless via the Epic-20 frame-capture path
(`rendering_pipeline.md` §7.8): one capture-A/B per bit. Add a compose-gate test
that a mask change dirties the direct-SH re-dispatch, and one that a
return-to-`ALL` recomposes the direct atlas back to base (the
`last_composed_mask` inequality, per Task 4). The test task covers the
ordering-scenario rows T1–T9 (see Ordering scenarios); reference the rows rather
than restating them. Update `rendering_pipeline.md` §4 (retire the "10 lighting
isolation modes" sentence and the `DynamicDirectIsolation` description; describe
the unified per-term mask and its compose-time vs in-shader gate split), §7.1
step 5 (the compose passes honor the mask), §7.5 (fog dynamic scatter honors the
Dynamic-direct bit read from group-0; while there, correct the stale
`FOG_PARAMS_SIZE` figure — source is 112 bytes, not the documented 176 /
`prev_view_proj`), and §9 (the mesh group-2/group-4 params carry the mask, not
the two isolation enums).

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice, falsifies the boundary
assumptions (bit vocabulary, `u32` stride preservation, UI→state→group-0+compose
flow, compose-time indirect gating, in-shader gating). Blocks everything.

**Phase 2 (concurrent):** Task 2, Task 3, Task 4, Task 5 — independent consumers
of the Phase 1 contract. Task 2 owns group-2 (mesh/kinematic) + group-4; Task 3
owns the group-0 tail; Task 4 owns the direct-SH compose; Task 5 owns
`fog_volume.wgsl` alone (reads the existing group-0 mask). No shared files across
the four.

**Phase 3 (sequential):** Task 6 — consumes the completed migration; removes the
enums only once no consumer references them, and lands the test + doc sweep
against the integrated result.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Default mask (all wired bits set) renders identically to pre-change | Task 1 (`ALL` default; group-0 offset unchanged) | Every task's gate must be a no-op at its bit set; compose default all-on | AC 1, AC 8 |
| Group-0 128 B / group-2 16 B & 32 B / group-4 16 B strides unchanged | Task 1, 2, 3 (same-width `u32` replacement; retired fields become pad) | Any field width change breaks the shared shader byte contract | AC 8 |
| No-double-count holds under isolation | Task 4 (promotion subtraction tracks bit 5) | Static-direct bit on + dynamic bit off must not subtract the absent runtime term | AC 4, AC 2 |
| One term = one contribution, gated once | Task 1 (compose for SH indirect), Task 4 (compose for SH direct), Tasks 1–3 (in-shader ambient/dynamic/specular/lightmap) | A term gated in two places would double-toggle; SH terms move fully to compose, removing the in-shader SH branch | AC 2, AC 3, AC 4 |
| Any mask change (incl. return to `ALL`) recomposes the direct-SH atlas | Task 4 (`last_composed_mask` inequality dirty; Pass A/B dispatched together) | `mask != ALL` predicate would strand the atlas isolated; skipped non-world frame must not drop the change | AC 7, Ordering T1/T4/T8 |
| All mask consumers read one per-frame snapshot; a toggle lands atomically at N+1 | Task 1 (snapshot at `update_per_frame_uniforms`) | Any consumer reading the live `full.light_term_mask` post-UI leads group-0 by one frame (fog, group-2, direct Pass A) | AC 2, Ordering T10/T11 |

## Ordering scenarios

Rows the mask-consuming tasks (2, 4, 5) and the test task (Task 6) must satisfy.
Test task cites these rows; do not restate them in prose. "This/same frame" below
means one atomic upload: because every consumer reads the per-frame snapshot
(Task 1), a UI toggle on frame N takes effect together on frame N+1, never split
across consumers.

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| T1 | Isolate then restore a direct bit | mask `ALL` (atlas = base, idle) → UI clears bit 3 → later UI sets bit 3 (mask `ALL` again) | recompose isolated, then recompose **back to base**. Dirty is `snapshot != last_composed_mask`, not `mask != ALL`. World `lm_irr` and entity atlas match at every step. |
| T2 | Indirect bit toggled at runtime, no reload | UI clears bit 2 | same frame's `sh_compose` (reads group-0) drops the animated delta; forward/mesh/billboard/fog change this frame. Mask NOT sourced from static GridDims. |
| T3 | Direct bit; world + entity agree same frame | UI clears bit 3; order: group-0 write → direct dispatch → forward draw | world in-shader `lm_irr` (bit 3) and entity composed-direct (bit 3, Pass A) both reflect bit-3-off in the same frame. No world/entity skew. |
| T4 | Bits 3 and 4 changed together | UI clears bit 3 and bit 4 | Pass A (private uniform) and Pass B (group-0) dirtied and dispatched in one call; never one bit applied and the other lagging. |
| T5 | Mask changed while paused | sim halted; UI clears bit 5; redraw issued | direct atlas recomposes, frozen scene updates without reload. Compose is render-frame-driven; `freeze_time` does not suppress it. |
| T6 | Level reload / full-init with isolation active | bit 3 off → reload (`full` rebuilt) | mask resets to `ALL`; mask and freshly-built atlas both at default, consistently. Prior isolation intentionally lost; no stale-atlas/uniform mismatch. |
| T7 | Load copy-through vs snapshot mask | new direct-compose resources (copy-through) with mask isolated | copy-through composes against the per-frame **snapshot** (or `last_composed_mask` seeded ≠ snapshot so frame 1 self-heals); never default-mask-while-uniform-isolated. |
| T8 | Mask changed on a non-world frame | frame N `render_world==false`, UI clears bit 4; frame N+1 `render_world==true` | honored on N+1 (dirty is level-triggered by inequality; a skipped frame cannot drop it). |
| T9 | Same-frame load + mask change + nonzero promotion weight | one frame: reconstruction (copy-through), `active`, mask changed | single dispatch composes against the snapshot mask + live weights; no double dispatch, no default-mask compose. |
| T10 | Fog dynamic bit vs world dynamic bit on the toggle frame | frame N: `update_per_frame_uniforms` snapshots mask + writes group-0 → UI clears bit 5 → scene recording: fog raymarch + forward | fog `spot`/`point` scatter and forward's group-0 dynamic bit reflect the **same** snapshot on frame N (fog reads the group-0 mask, not the live field). No fog-leads-world skew; change lands N+1. |
| T11 | Direct Pass A / dirty trigger vs group-0 (Pass B, world `lm_irr`) on the toggle frame | frame N toggle; Pass A private uniform + dirty trigger read inside scene recording (post-UI) | Pass A's mask input and the dirty `snapshot` equal the group-0 snapshot Pass B / world `lm_irr` read, so bits 3/4 land together across Pass A, Pass B, and world. |

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
  2 is set. Mask read from the per-frame group-0 `FrameUniforms` bind group
  `sh_compose` already binds (bytes 88..92) — not the static GridDims params.
- Compose gate (direct): mirror in `direct_sh_compose.wgsl` (bit 3) and the
  `animated_direct_sh_compose` add pass (bit 4), per Task 4.
- UI: a set of `ui.checkbox` bound to mask bits, replacing the two ComboBoxes in
  the "Lighting systems" header (`debug_ui/mod.rs:320-323`). Checkboxes only, no
  presets (owner decision: with ≤8 terms, independent checkboxes make each term's
  contribution directly visible; presets would be overkill).
- The `POSTRETRO_*` env seeding pattern the old modes used (if any) maps to a
  default-`ALL` mask; a headless run keeps every term on.

## Resolved decisions

- **Fog dynamic scatter is gated** (owner, 2026-08-10). Bit 5 reaches fog's
  spot/point beam scatter via the group-0 mask the fog raymarch already binds
  (Task 5) — it helps humans read what's happening at runtime, and it closes the
  blackout gap the review found.
- **Checkboxes only, no presets** (owner, 2026-08-10). ≤8 terms; presets overkill.
- **Emissive stays out** (owner, 2026-08-10). Bit 7 reserved; see Out of scope.
- **Emissive has landed.** `emissive-surfaces-bloom` code is merged (the plan
  folder is still in `in-progress/`, a bookkeeping lag), so this plan is not
  blocked on it. Both edit `forward.wgsl` + `kinematic_brush.wgsl`, but emissive
  is already in the current source this plan is grounded against — no ordering
  dependency remains.
