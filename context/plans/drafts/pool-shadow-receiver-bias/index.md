# Pool-Shadow Receiver Bias

## Goal

Eliminate shadow-map self-comparison acne ("texel-grid striping") on the pool-shadow receiver classes — world surfaces via the shadowmask union term, kinematic movers, and skinned characters — with one shared receiver-side normal-offset mechanism, tuned per receiver class. Self-shadow character on skinned models becomes an authorable per-model scalar, because quantized self-shadow is on-brand for flat pixel-art characters and objectionable on rounded ones. Root-cause notes: `research.md`.

## Scope

### In scope

- Receiver-side normal-offset bias in the shared pool sampling helpers (`shadow_sample.wgsl`), spot and point paths, parameterized by receiver normal and a bias scale.
- Per-receiver-class bias calibration: world (aggressive), mover (aggressive), skinned (conservative — default preserves current character appearance).
- Dead-zone on the forward shadowmask union subtraction so sub-threshold compare noise never darkens world surfaces (invariant enforcement: runtime static→static shadowing stays zero).
- Dev-tools diagnostic mode visualizing raw pool shadow visibility over receivers.
- `shadowBiasScale` authoring scalar on the mesh component descriptor, plumbed to the skinned fragment path per instance.

### Out of scope

- Caster-side changes: hardware `DepthBiasState` values, depth-pass cull modes, and the caster-classes-depth-identical constraint all stay as-is.
- Fog: `fog_volume.wgsl` keeps its private `sample_spot_shadow_pt` copy untouched — raymarch samples have no receiver normal, and in-scattering wedges are not acne-prone.
- Shadow-map resolution, PCF kernel shapes, slot budgets, promotion ranking.
- Exposing bias to world/mover authoring (FGD or scripting) — per-class constants only; skinned models are the only authorable class.
- Frame-capture golden-image regression — deferred until `E20--frame-capture` ships; tracked as a follow-on, not a task here.
- Splitting `forward.wgsl` (>800-line flag acknowledged): edits land inside existing functions; no new subsystem justifies a split task.

## Acceptance criteria

- [ ] On `closet-reveal.map` under the static spotlight, the closed door and the wall it touches show no texel-grid striping at close camera range; the same scene before the fix reproduces it (manual visual, both states screenshotted in the PR).
- [ ] The closed door's shadow and the door/wall contact seam show no light leak or shadow detachment after the mover bias is applied (manual visual on `closet-reveal.map`).
- [ ] An enemy standing under a promoted static light still casts a visible, attached shadow onto floor/wall — no detachment or disappearance of contact shadows (manual visual on `closet-reveal.map` or `campaign-test.prl`).
- [ ] A rounded skinned character with `shadowBiasScale` raised above default shows no crosshatch under a promoted static light; at default scale, a flat pixel-art character's self-shadow is visually unchanged from today (manual visual).
- [ ] With no entity occluder present, the union-term visualization reads ~zero everywhere on `closet-reveal.map` — baked penumbrae included (dev-tools visual).
- [ ] A dev-tools mode displays raw pool shadow visibility for promoted lights over world receivers, selectable alongside the existing shadowmask-union visualization.
- [ ] `shadowBiasScale` outside its valid range is rejected at descriptor validation with an actionable error; omitting it yields the default (compiler/validation test). Regenerated SDK typedefs are committed and the typedef drift test passes.
- [ ] `shadowBiasScale: 0.0` yields sampling identical to pre-change behavior — pinned by a shader-source or packing test, not just visually.
- [ ] Skinned per-instance data still uploads at its current stride; the layout change is pinned by a CPU-side packing test, and the existing shader-layout test pattern stays green untouched (the WGSL instance struct is unchanged).
- [ ] Pinned shader tests (naga validation, forward shader/budget guards, light-space matrix array pins, kinematic content pins) pass, updated where touched strings changed.
- [ ] Fog shader source is byte-identical to before the change.

## Tasks

### Task 1: Receiver-biased pool sampling

In `crates/renderer/src/shaders/shadow_sample.wgsl`, extend `sample_spot_shadow` and `sample_point_shadow` to accept the receiver's geometric world normal and a bias scale, and offset the sampled world position along that normal by the shadow-texel world footprint × scale before light-space projection (spot: estimate footprint from distance to light, the projection's FOV, and `textureDimensions`; point: from distance and `CUBE_FACE_RESOLUTION`). The spot projection's FOV is not a shader input: derive the footprint scale from the light-space projection matrix already bound per slot (its y-axis column scale), or use a conservative constant-angle approximation — pick one and comment why. Keep the existing `POINT_SHADOW_DEPTH_BIAS` receiver depth bias. Define per-class scale constants in the same file: world, mover, skinned. Update every consumer to pass its class constant and geometric normal: `forward.wgsl` — the dynamic-loop spot and point call sites, `shadowmask_sample_spot_shadow_wide`, and `shadowmask_shadow_visibility` (world class, `mesh_n`); `kinematic_brush.wgsl` — the spot and point call sites in `accumulate_dynamic_direct` (mover class; `accumulate_dynamic_direct` currently receives only the bump/shading normal — thread the geometric `mesh_n` in as a new argument; the offset must use it, not the bump normal); `skinned_mesh.wgsl` — its spot and point call sites (skinned class, interpolated world normal, scale multiplied by a per-instance factor that this task hardwires to 1.0 — Task 3 supplies the real value). Do NOT touch `fog_volume.wgsl` — its `sample_spot_shadow_pt` is an intentionally separate copy. No kinematic test fingerprints these shadow calls — the naga-validation shader tests are what catch signature mismatches; update `light_space_matrices_array_len_matches_pool` and any forward/kinematic shader content tests only if their pinned strings are touched. Calibrate world/mover constants so the closet-reveal door/wall striping disappears while an enemy's floor shadow stays attached; skinned default must not visibly change current characters. Calibration workflow: compile with `cargo run -p postretro-level-compiler -- content/dev/maps/closet-reveal.map -o content/dev/maps/closet-reveal.prl`, then launch `cargo run -p xtask -- run content/dev/maps/closet-reveal.prl`.

### Task 2: Union dead-zone + raw-visibility diagnostic

In `forward.wgsl` `shadowmask_union_subtraction`, apply a dead-zone to `baked_vis − shadow_map_vis`: differences below a threshold contribute zero, and above it the difference is renormalized so the response stays continuous (no hard step at the threshold). Threshold is a shader constant sized to swallow residual compare noise surviving Task 1 without eating real entity shadows (entity occlusion drives the difference toward 1). Add a dev-tools diagnostic mode that renders raw pool `shadow_map_vis` for promoted lights over world receivers, following the existing `SdfShadowMode::ShadowmaskUnion` / mode-5 pattern (`SdfShadowMode` enum, uniform round-trip, diagnostics label). Pin with the existing test patterns: a mode round-trip test alongside `sdf_shadow_mode_round_trips_through_uniform`, and keep the `promoted_count = 0` zero-subtraction pin green.

### Task 3: Per-model `shadowBiasScale` authoring

Add optional `shadowBiasScale` (number, default 1.0, valid range 0.0–4.0 inclusive, finite; reject outside with an actionable validation error) to the mesh descriptor surface. The descriptor struct and validation live in `crates/entities/src/data_descriptors/types/entity.rs`: add the field to `MeshDescriptor` and a range check in `MeshDescriptor::build`, which stores the validated value into the `MeshComponent` it constructs; update both `build` call sites. Descriptor parsing is manual value reading, not serde: add `shadowBiasScale` readers to `mesh_descriptor_from_js` in `crates/scripting-core/src/data_descriptors/js/entity.rs` and the Luau sibling in `lua/entity.rs`. The SDK typedefs (`sdk/types/postretro.d.ts`, `sdk/types/postretro.d.luau`) are generated — "Do not edit by hand": add the field to the `MeshDescriptor` type registration in `crates/postretro/src/scripting/primitives/mod.rs` (the Rust typedef source consumed by `gen-script-types`), regenerate, and commit the output so the typedef drift test stays green. Add `shadow_bias_scale: f32` to `MeshComponent` (`crates/entities/src/components/mesh.rs`, serde default = 1.0, snake_case like its siblings). Plumb the full chain: mesh collector (`crates/postretro/src/scripting/systems/mesh_render.rs`), which reads `MeshComponent::shadow_bias_scale` into `MeshInstanceInput` → `PlannedInstance` (both in `crates/render-cpu/src/mesh_instances.rs`; the planner copies fields one-by-one — add the copy) → `build_instance_entry` in `crates/renderer/src/render/mesh_pass.rs`, which gains the scale as a new argument and packs the f32 bitcast into a padding lane of the instance struct's trailing `base_and_pad: vec4<u32>` (80-byte stride unchanged). The existing byte-layout test asserting padding bytes 68..80 are zero must change to pin the new lane. In `skinned_mesh.wgsl`, read the lane in `vs_main`, carry it to the fragment via an `@interpolate(flat)` output, and multiply it into the skinned-class bias scale at the spot and point sample call sites (replacing Task 1's hardwired 1.0). Semantics: only scale 0.0 reproduces today's exact sampling (zero offset); the skinned class constant is calibrated so the default 1.0 is visually imperceptible on current characters, with smoothing appearing only at raised scales. Add: a validation test for range rejection and default that also asserts a built `MeshComponent` carries the descriptor's `shadow_bias_scale` value (pins the descriptor→component copy, which no downstream packing test covers), the updated CPU packing test pinning the instance-lane bytes, and the skinned shader layout comment.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the helper signature and class constants; blocks both others.
**Phase 2 (concurrent):** Task 2 (forward.wgsl only), Task 3 (descriptor→instance plumbing + skinned_mesh.wgsl) — disjoint files after Task 1 lands.

## Rough sketch

- Offset direction is the geometric normal, not the bump/shading normal — bump perturbation would wobble shadow boundaries.
- Spot texel footprint ≈ `2 · dist · tan(fov_y/2) / resolution` — FOV-derivation choice is stated in Task 1.
- Per-class constants live beside `SPOT_SHADOW_PCF_RADIUS` in `shadow_sample.wgsl` with the same "ONE shared parameter, per-consumer scale" comment discipline.
- Dead-zone shape: `smoothstep(t, 2t, diff) · diff` or `max(diff − t, 0) / (1 − t)` — continuous, zero below threshold, identity near 1.
- Diagnostic mode: next free `SdfShadowMode` discriminant; render `shadow_map_vis` greyscale via the mode-5 early-return pattern in `fs_main`.
- `MeshComponent` is `Serialize`/`Deserialize` with skip-defaults idioms already (`origin_offset`); mirror that so existing persisted/replicated payloads deserialize with the default. Replication: field rides the existing component snapshot path; no wire-format change beyond the serde field.
- Calibration command lives in Task 1's paragraph; the run target is the compiled `.prl`, never the `.map` (xtask `run` does not compile). Toggle the Task 2 diagnostic once available; until then compare against the striping screenshot in the originating session.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Shadow bias scale | `MeshComponent::shadow_bias_scale` | `"shadow_bias_scale"` | `shadowBiasScale` | `shadowBiasScale` | n/a |

## Script syntax examples

```ts
// Rounded hero model: smooth out pool self-shadow quantization.
const knight = defineEntity({
  components: {
    mesh: { model: "models/knight.gltf", shadowBiasScale: 2.5 },
  },
});

// Flat pixel-art enemy: keep the crunchy quantized self-shadow (default).
const imp = defineEntity({
  components: {
    mesh: { model: "models/imp.gltf" }, // shadowBiasScale omitted → 1.0
  },
});
```

## Open questions

- Whether the spot path needs a small receiver-side depth bias in addition to normal-offset for the coplanar door-against-wall contact case — decide during Task 1 calibration; if added, keep it a constant beside the class scales.
- Dead-zone threshold value — calibrate in Task 2 against closet-reveal with and without an entity under the light; the AC pins the observable outcomes, not the number.
