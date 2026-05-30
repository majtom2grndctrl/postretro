# SDF Spotlight Cone Shadows

## Goal
Let `light_spot` entities with `_shadow_type sdf` cast cone-shaped light while keeping
their existing per-light SDF soft shadow. Today a static spotlight on the SDF path
shades as an omni point — the cone direction and angles are dropped at pack time, so
the light spills in every direction. This adds cone shaping (direction + inner/outer
angle smoothstep) to the static diffuse and specular loops, folds cone awareness into
per-fragment light selection, and clamps the shadow march to the cone — at the cost of a
wider per-light GPU record. It also fixes a pre-existing bug: `static_light_map`
spotlights leak cone-less *specular* (their diffuse bakes correctly; specular is computed
live as omni).

## Scope

### In scope
- Carry cone geometry (direction, precomputed inner/outer cosines, spot flag) and a
  reserved light source-radius slot in the per-light static GPU record.
- Apply cone smoothstep attenuation in the forward **SDF diffuse** loop and the forward
  **static specular** loop — fixing the `static_light_map` cone-less-specular bug for free.
- Fold cone falloff into the per-fragment top-K SDF light **selection** weight as a
  continuous term (not a binary in/out gate), so a spot pointing away surrenders its slot
  smoothly. Update the host-side reference comparator to match.
- SDF visibility pass: skip the trace for fragments outside a spot's cone or range, and
  clamp the march length to `min(range, dist-to-light)`.
- A live debug-panel **cone early-reject toggle** (gate-tuning instrument for the AMD
  5500M floor) with a headless env-var override, wired through the renderer like the
  existing SDF sliders.

### Out of scope
- **Animated aim / reorientation** of SDF spots. SDF spots keep static aim; the occluder
  field is static and intensity/color animation (the "animated" axis) is orthogonal and
  already covered elsewhere. (Aim animation remains a dynamic-tier capability only.)
- **Per-light source radius driving penumbra width.** The record reserves the slot, but v1
  penumbra stays on the existing global `penumbra_k`. Wiring per-light radius is a later
  pass with no record-layout change.
- **Billboard static-spot cone shaping.** Billboards iterate the same static light list and
  would also benefit, but cone-less spill on additive sprites is far less visible than on
  world geometry. Deferred; the record change makes it a small follow-up.
- New FGD KVPs. Cone direction/angles are already authored and parsed; this plan only
  stops discarding them.
- New PRL section. The static light record is a runtime GPU buffer packed from `MapLight`
  at load, not serialized.

## Acceptance criteria

Automated:
- [ ] A fragment outside a spot's outer cone does not select that spot into its top-K SDF
      set — verified by the host-side reference comparator over a spot + omni case.
- [ ] Half-res SDF selection and forward selection choose the same lights, same order, with
      cone weighting active — the existing K-selection parity test passes against the
      amended comparator.
- [ ] Both consumer shaders (forward, SDF visibility) pass naga validation with the widened
      static-light record.
- [ ] The static-light record packer emits the spot flag and cone cosines for a spotlight
      and zero/identity for a point light — unit-tested in the packer.

Manual-visual:
- [ ] An `sdf` `light_spot` lights only inside its cone (full within inner angle, smooth
      falloff to dark by the outer angle); geometry behind/outside the cone gets no direct
      term from it. Its SDF shadow still occludes correctly within the cone.
- [ ] A `static_light_map` `light_spot` no longer shows a specular highlight outside its
      cone.
- [ ] `occlusion-test` (its 6 `sdf` spots) renders cone-shaped spotlights with no visual
      regression to the surrounding point lights.
- [ ] Toggling cone early-reject in the Diagnostics panel changes the `sdf_shadow` pass time
      under `POSTRETRO_GPU_TIMING=1` on a spot-heavy view, with no visible shading change
      when on.

## Tasks

### Task 1: Widen the static-light record + shared cone helper
Extend the per-fragment static-light GPU record (`SpecLight`) to carry the cone direction,
precomputed `cos(inner)` / `cos(outer)`, a spot flag, and a reserved source-radius slot.
Update the packer (`lighting/spec_buffer.rs::pack_spec_lights`, `SPEC_LIGHT_SIZE`) to read
the cone fields already on `MapLight` (`cone_direction`, `cone_angle_inner/outer`,
`light_type`) and precompute the cosines; point lights pack the spot flag clear. Update the
two WGSL struct declarations (`forward.wgsl`, `sdf_shadow.wgsl`) to match. Add ONE shared
cosine-taking cone-attenuation helper to `sdf_light_select.wgsl` (already concatenated into
both consumers) so shading, selection, and reject share a single definition and no
in-shader `cos()` runs in the per-fragment loops. The record is a storage buffer, so no
bind-group-layout stride change is needed — only the struct decls and the size constant.

### Task 2: Cone shaping in forward shading
In `forward.wgsl`, multiply the shared cone-attenuation factor into the per-light term in
both the SDF diffuse loop and the static specular loop. The spot flag gates it — point
lights get factor 1.0. This is the bug fix for `static_light_map` cone-less specular as
well as the new SDF-spot diffuse/specular cone. Cone falloff is applied at full res here,
independent of the half-res visibility slice (visibility stays occlusion-only).

### Task 3: Cone-aware light selection
In `sdf_light_select.wgsl`, multiply the selection influence metric by the continuous cone
attenuation (the shared helper) so a fragment outside a spot's cone weights that spot toward
zero and it loses its K-slot smoothly — no binary gate (which would flicker at cone edges
between neighboring fragments). Amend the host-side reference comparator in
`render/sdf_light_select_test.rs` to mirror the new weight exactly so the parity test stays
valid.

### Task 4: Cone-bounded SDF march + panel toggle
In `sdf_shadow.wgsl`, before tracing a selected light, reject when the fragment is outside
the light's cone (`dot` vs `cos(outer)`) or beyond range — return fully lit, no march.
Clamp the bounded march length to `min(range, dist-to-light)` instead of the fixed 64 m.
Add a cone-early-reject enable to `ShadowPassParams` (sdf_shadow.rs), a renderer setter
mirroring the existing `set_sdf_*` pattern, a Diagnostics-panel toggle
(`render/debug_ui/mod.rs`), and a headless env-var override matching the existing
`POSTRETRO_SDF_*` convention.

### Task 5: Documentation amendments
Amend the parent slice's contract map (`context/plans/in-progress/sdf-per-light-shadows/
architecture.md`): extend the K-selection-parity seam row to note cone-weighted selection;
correct invariant 9's `static_light_map`-specular note (now cone-shaped, still unshadowed);
add animated-aim to the defers list. These are plan-layer (not `context/lib/`) edits; the
durable promotion into `context/lib/sdf_shadows.md` happens with the parent slice after its
perf gate.

## Sequencing

**Phase 1 (sequential):** Task 1 — the record shape and shared helper block all consumers.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — distinct files (forward shading loops /
the shared select helper + its host test / the SDF pass + panel); all consume Task 1.
**Phase 3 (sequential):** Task 5 — documents the final shape.

## Rough sketch
- `MapLight` (prl.rs:154) already has every cone field; the gap is purely that
  `pack_spec_lights` (spec_buffer.rs) drops them. The record grows from two `vec4`s
  (32 B) to three (64 B): keep `position_and_range` and `color_and_sdf_flag`; add a third
  `vec4` carrying cone direction + one cosine, with the second cosine, reserved
  source-radius, and spot flag in the spare lanes. Treat the layout as a **constraint**
  (carries: cone dir, `cos_inner`, `cos_outer`, `is_spot`, reserved `source_radius`); the
  implementer picks lane assignment — but the two WGSL decls and the packer must agree.
- The existing `cone_attenuation` (forward.wgsl:329, billboard.wgsl:241) takes raw angles
  and calls `cos()`; the new shared helper takes precomputed cosines so the
  iterate-all-chunk-lights specular loop spends no `cos()` per light. Leave the
  angle-taking versions for the dynamic loop (which packs angles, not cosines).
- Selection parity is the load-bearing seam: cone weight must enter
  `sdf_select_influence` (the shared helper), so the half-res pass and the forward shade
  the same lights. The host comparator in `sdf_light_select_test.rs` is the durable record
  of that order — update it in lockstep.
- Panel knob mirrors the `set_sdf_max_march_steps` / `DEFAULT_*` pattern in sdf_shadow.rs
  (tuning struct + byte-pack into the uniform near offset 88–96) and the seed-on-first-draw
  `DiagnosticsState` flow in debug_ui/mod.rs.
- `pack_spec_lights` has two call sites (render/mod.rs:1270, :2088 — load + reload paths);
  both already route through the one function, so the packer change reaches both.

## Open questions
- Cone-reject margin: reject hard at `cos(outer)`, or keep a small angular slack so the
  smoothstep penumbra near the cone edge isn't clipped before it reaches zero? Leaning
  slack = a few degrees past outer; decide during Task 4 against the visual AC.
- Promotion coupling: this plan's durable content lands in `context/lib/sdf_shadows.md`
  with the parent SDF slice (gated). If the parent reverts on its perf gate, the cone
  diffuse/selection path reverts with it; the `static_light_map` specular-cone fix is
  gate-independent and stays. Confirm this split is acceptable before promotion.
