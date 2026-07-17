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
- [ ] An enemy standing under a promoted static light still casts a visible, attached shadow onto floor/wall — no detachment or disappearance of contact shadows (manual visual on `closet-reveal.map` or `campaign-test.prl`).
- [ ] A rounded skinned character with `shadowBiasScale` raised above default shows no crosshatch under a promoted static light; at default scale, a flat pixel-art character's self-shadow is visually unchanged from today (manual visual).
- [ ] With no entity occluder present, the union-term visualization reads ~zero everywhere on `closet-reveal.map` — baked penumbrae included (dev-tools visual).
- [ ] A dev-tools mode displays raw pool shadow visibility for promoted lights over world receivers, selectable alongside the existing shadowmask-union visualization.
- [ ] `shadowBiasScale` outside its valid range is rejected at descriptor validation with an actionable error; omitting it yields the default (compiler/validation test).
- [ ] Skinned per-instance data still uploads at its current stride; the layout change is pinned by a CPU-side packing test and the existing shader-layout test pattern.
- [ ] Pinned shader tests (kinematic call fingerprint, forward shader/budget guards, light-space matrix array pins) pass, updated where signatures changed.
- [ ] Fog shader source is byte-identical to before the change.

## Tasks

### Task 1: Receiver-biased pool sampling

In `crates/renderer/src/shaders/shadow_sample.wgsl`, extend `sample_spot_shadow` and `sample_point_shadow` to accept the receiver's geometric world normal and a bias scale, and offset the sampled world position along that normal by the shadow-texel world footprint × scale before light-space projection (spot: estimate footprint from distance to light, the projection's FOV, and `textureDimensions`; point: from distance and `CUBE_FACE_RESOLUTION`). Keep the existing `POINT_SHADOW_DEPTH_BIAS` receiver depth bias. Define per-class scale constants in the same file: world, mover, skinned. Update every consumer to pass its class constant and geometric normal: `forward.wgsl` — the dynamic-loop spot and point call sites, `shadowmask_sample_spot_shadow_wide`, and `shadowmask_shadow_visibility` (world class, `mesh_n`); `kinematic_brush.wgsl` — the spot and point call sites in `accumulate_dynamic_direct` (mover class; thread the mesh normal into that function); `skinned_mesh.wgsl` — its spot and point call sites (skinned class, interpolated world normal, scale multiplied by a per-instance factor that this task hardwires to 1.0 — Task 3 supplies the real value). Do NOT touch `fog_volume.wgsl` — its `sample_spot_shadow_pt` is an intentionally separate copy. Update the pinned shader tests that fingerprint these calls (kinematic fingerprint tests in `kinematic_brush.rs`, forward shader tests, `light_space_matrices_array_len_matches_pool`). Calibrate world/mover constants so the closet-reveal door/wall striping disappears while an enemy's floor shadow stays attached; skinned default must not visibly change current characters.

### Task 2: Union dead-zone + raw-visibility diagnostic

In `forward.wgsl` `shadowmask_union_subtraction`, apply a dead-zone to `baked_vis − shadow_map_vis`: differences below a threshold contribute zero, and above it the difference is renormalized so the response stays continuous (no hard step at the threshold). Threshold is a shader constant sized to swallow residual compare noise surviving Task 1 without eating real entity shadows (entity occlusion drives the difference toward 1). Add a dev-tools diagnostic mode that renders raw pool `shadow_map_vis` for promoted lights over world receivers, following the existing `SdfShadowMode::ShadowmaskUnion` / mode-5 pattern (`SdfShadowMode` enum, uniform round-trip, diagnostics label). Pin with the existing test patterns: a mode round-trip test alongside `sdf_shadow_mode_round_trips_through_uniform`, and keep the `promoted_count = 0` zero-subtraction pin green.

### Task 3: Per-model `shadowBiasScale` authoring

Add optional `shadowBiasScale` (number, default 1.0, valid range 0.0–4.0 inclusive, finite; reject outside with an actionable validation error) to the mesh descriptor surface: `MeshDescriptor` in `sdk/types/postretro.d.ts` and the Luau sibling `sdk/types/postretro.d.luau`, descriptor parsing/validation in `crates/scripting-core/src/data_descriptors/` (serde camelCase), and a `shadow_bias_scale: f32` field on `MeshComponent` (`crates/entities/src/components/mesh.rs`, serde default = 1.0). Plumb it through the mesh collector (`crates/postretro/src/scripting/systems/mesh_render.rs`) into `MeshInstanceInput` (`crates/render-cpu/src/mesh_instances.rs`), then into the skinned per-instance SSBO in the mesh pass upload: pack the f32 into one of the padding lanes of the instance struct's trailing `base_and_pad: vec4<u32>` (bitcast; 80-byte stride unchanged). In `skinned_mesh.wgsl`, read it in `vs_main`, carry it to the fragment via an `@interpolate(flat)` output, and multiply it into the skinned-class bias scale at the spot and point sample call sites (replacing Task 1's hardwired 1.0). Scale 0.0 must yield zero offset — today's exact sampling. Add: a validation test for range rejection and default, a CPU packing test pinning the instance-lane bytes, and update the skinned shader layout comment/test.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the helper signature and class constants; blocks both others.
**Phase 2 (concurrent):** Task 2 (forward.wgsl only), Task 3 (descriptor→instance plumbing + skinned_mesh.wgsl) — disjoint files after Task 1 lands.

## Rough sketch

- Offset direction is the geometric normal, not the bump/shading normal — bump perturbation would wobble shadow boundaries.
- Spot texel footprint ≈ `2 · dist · tan(fov_y/2) / resolution`; fov is not in the shader — either derive from the projection matrix column scale (`1/m[1][1]` in light space) or accept a conservative constant-angle approximation. Implementer's choice; pick one and comment why.
- Per-class constants live beside `SPOT_SHADOW_PCF_RADIUS` in `shadow_sample.wgsl` with the same "ONE shared parameter, per-consumer scale" comment discipline.
- Dead-zone shape: `smoothstep(t, 2t, diff) · diff` or `max(diff − t, 0) / (1 − t)` — continuous, zero below threshold, identity near 1.
- Diagnostic mode: next free `SdfShadowMode` discriminant; render `shadow_map_vis` greyscale via the mode-5 early-return pattern in `fs_main`.
- `MeshComponent` is `Serialize`/`Deserialize` with skip-defaults idioms already (`origin_offset`); mirror that so existing persisted/replicated payloads deserialize with the default. Replication: field rides the existing component snapshot path; no wire-format change beyond the serde field.
- Task 1 calibration workflow: `cargo run -p xtask -- run content/dev/maps/closet-reveal.map` (compile via `prl-build` first if stale), toggle the Task 2 diagnostic once available; until then compare against the striping screenshot in the originating session.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Shadow bias scale | `MeshComponent::shadow_bias_scale` | `"shadowBiasScale"` | `shadowBiasScale` | `shadowBiasScale` | n/a |

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
