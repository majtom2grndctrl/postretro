# Specular Shadowmask Occlusion

## Goal

Make a `static_light_map` light's view-dependent specular highlight on world geometry respect the
same baked visibility its diffuse term already carries. Today the specular loop multiplies by 1.0 for
every non-`sdf` static light, so a fully shadowed surface can float a full-strength highlight.

## Scope

### In scope

- World (static BSP) geometry only, in the forward opaque pass.
- Per-light baked visibility for static lights that carry a `ShadowmaskAtlas` (PRL id 42) channel.
- A dev A/B toggle that forces the new per-light visibility back to 1.0.
- Headless frame-capture coverage of a shadowed-highlight scene.

### Out of scope

- **Static lights with no shadowmask channel.** The atlas covers only `EntityShadowLights`-selected
  lights: non-dynamic, non-animated, not `bake_only`, point or spot, `static_light_map` shadow type,
  above the intensity and range thresholds, not a decorative spot fixture — minus any light the
  4-channel graph coloring drops. Every other static light keeps today's unshadowed specular.
  Widening that set changes promotion ranking and belongs to its own plan.
- **Entity-occluder receipt on specular.** The shadowmask union term (baked mask ∪ pool shadow map)
  darkens the diffuse lightmap only. Specular consumes the baked mask, never the pool map.
- **Movers, skinned meshes, billboards.** Their specular already comes solely from promoted static
  records shadowed by the pool map and faded by the promotion weight. Untouched.
- **`sdf`-typed lights.** Already shadowed per-light by the SDF visibility slice.
- **Animated static lights.** Their direct term lives in the animated atlas, not the shadowmask.
- **Any new baked PRL section.** No wire-format change.

## Acceptance criteria

- [ ] On a map with a shadowmask, a world surface that reads as shadowed from a given static light in
      the lightmap shows no view-dependent highlight from that light at any camera angle; the same
      surface still shows a full highlight where the lightmap reads lit, and the transition across
      the baked penumbra is a continuous ramp, not a hard step.
- [ ] With the new dev "force specular visibility to 1" toggle on, a captured frame is byte-identical
      to the same frame captured before this change — proving the change only ever multiplies an
      existing term and never adds light.
- [ ] A light authored with `_shadow_type sdf` renders identically to before, including inside its
      SDF-shadowed regions; no static light is darkened by two visibility signals at once.
- [ ] A static light that the compiler does not select for the shadowmask (dim, short-range,
      directional, animated, decorative spot fixture, or dropped by channel assignment) renders
      identically to before.
- [ ] A map with no `ShadowmaskAtlas` section renders identically to before.
- [ ] While a mover walks through a promoted static light's influence and the promotion crossfade runs
      to completion in both directions, no world-surface highlight changes brightness, pops, or
      flickers.
- [ ] The forward pipeline creates the same number of bind groups, samplers, storage buffers, and
      textures as before; no per-stage binding budget moves. Existing headless pipeline-budget and
      uniform-size tests pass unchanged.
- [ ] A committed headless capture scene reproduces the shadowed-highlight case and its golden image
      fails if the occlusion multiply is removed.

## Tasks

### Task 1: Carry the shadowmask channel on every static light record

Give each packed `SpecLight` its shadowmask channel so the forward specular loop can find it without a
new binding. `crates/lighting/src/spec_buffer.rs` packs a fixed 64-byte record whose final slot
(`cone_cos`) uses only `.x` (cos inner) and `.y` (cos outer); `.z` and `.w` are zero padding today.
Write the channel into `.z` as a float: `0.0..3.0` selects an RGBA channel of the shadowmask atlas,
and a new `SPEC_LIGHT_SHADOWMASK_NONE = 4.0` sentinel means "no baked visibility, treat as fully
lit". The record stays 64 bytes — do not reorder fields. `pack_spec_lights` gains a second parameter:
a slice of channel bytes indexed in the same compacted `!is_dynamic` order the function already
emits, with `postretro_level_format::shadowmask_atlas::SHADOWMASK_CHANNEL_DROPPED` (`0xFF`) meaning
none. Build that slice renderer-side in `crates/renderer/src/render/shadowmask.rs` from three inputs
already on `WorldGeometryInput` (`renderer_types.rs`): `entity_shadow_lights`, `shadowmask_atlas`
(whose `channels[i]` aligns with `entity_shadow_lights[i]`), and the atlas's presence. Reuse the
existing `build_selection_spec_light_indices` helper for the selection-index → spec-index mapping —
it already encodes the `!is_dynamic` compaction, so never index with a global light index. When the
atlas is absent, when a selection entry maps to `FORWARD_SHADOWMASK_INVALID_INDEX`, or when the
channel is `0xFF`, emit the none sentinel. Both `pack_spec_lights` call sites must pass the new slice:
`crates/renderer/src/render/renderer_init_resources.rs` (~line 324) and
`crates/renderer/src/render/renderer_resources.rs` (~line 274). Keep `postretro-lighting` wgpu-free —
it receives a plain byte slice, it does not compute the mapping.

### Task 2: Multiply static specular by its baked visibility in the forward pass

In `crates/renderer/src/shaders/forward.wgsl`, extend the `SpecLight` WGSL struct comment to document
`cone_cos.z`, then resolve a per-light visibility inside the `use_specular` chunk-light loop. Add a
helper `shadowmask_visibility_for_spec_light(sl, lightmap_uv, lightmap_layer)` that returns 1.0 when
`round(sl.cone_cos.z) >= 4.0`, and otherwise samples `shadowmask_atlas` (already declared at
`@group(4) @binding(6)`, already an array texture) through `lightmap_filtering_sampler` at
`in.lightmap_uv` / `i32(in.lightmap_layer)` and selects the channel via the existing
`shadowmask_channel_value`. The loop currently computes `visibility` via `select(...)` over
`sdf_visibility_for_light` with `sdf_force_lit || !is_sdf` forcing 1.0; replace that with an explicit
two-branch choice keyed on `sdf_select_is_sdf(sl)` — `sdf` lights keep the SDF slice exactly as
today, non-`sdf` lights take the shadowmask value — so exactly one visibility signal ever applies to
one light. `sdf_force_lit` must keep forcing 1.0 on the SDF branch only. The result multiplies
`blinn_phong(...)` alongside `atten * cone`; nothing is added to `total_light` that was not there
before. Do not touch `static_direct`, `shadowmask_union_subtraction`, or the dynamic light loop. Do
not read the promoted-record metadata appended to `light_influence` — that metadata is per-frame and
promotion-weighted, and using it here would make the highlight crossfade with promotion. The
shadowmask channel comes from `SpecLight` only, which is level-load constant.

### Task 3: Dev force-lit toggle and uniform plumbing

Add a `spec_shadowmask_force_one` dev toggle that forces the Task 2 visibility to 1.0, mirroring the
existing `sdf_force_visibility_one` knob, so the A/B acceptance check is repeatable. The group-0
`Uniforms` struct has a trailing `_dyn_pad1: u32` slot; repurpose it rather than growing the struct —
`UNIFORM_SIZE` in `crates/render-cpu/src/frame_uniforms.rs` is exactly 128 and wgpu rejects the
pipeline if the CPU size and WGSL-derived stride drift. The struct is a four-way contract: update the
Rust writer (`FrameUniforms` field plus its `build_uniform_data` byte write at the same offset the
old pad occupied) and all three shaders that declare the tail —
`crates/renderer/src/shaders/forward.wgsl`, `crates/renderer/src/shaders/billboard.wgsl`, and
`crates/renderer/src/shaders/wireframe.wgsl`, whose own comment states it holds its stride "in
lockstep with forward.wgsl (128 bytes)". Only `forward.wgsl` reads the field; the other two must
still rename in lockstep, because a stale name there is silent layout drift rather than a
compile error. Thread the flag from the dev-tools Diagnostics
panel the same way `sdf_force_visibility_one` is threaded, and add a headless test asserting the flag
encodes at the expected byte offset, matching the existing
`uniform_data_encodes_sdf_force_visibility_one_at_correct_offset` test.

### Task 4: Headless capture coverage

Make the frame-capture path able to reproduce the bug and its fix, then commit a golden. Today
`crates/postretro/src/capture/driver.rs` overrides `entity_shadow_lights: &[]` in the `LevelGeometry`
it hands to `install_level_geometry`, so no capture ever has a shadowmask channel table. Thread the
loaded world's selection through instead. Watch the index space: `entity_shadow_lights` holds indices
into the full `world.lights` list, while `capture_static_lights` hands the renderer a pre-filtered
`!is_dynamic` copy — passing the filtered list alongside global selection indices misaligns them the
moment any dynamic light precedes a selected one. Pass the unfiltered `world.lights` (packing already
applies its own `!is_dynamic` filter, so the emitted `spec_lights` bytes are unchanged) or remap the
indices explicitly; assert the alignment in a test rather than relying on map content. Then author a
capture scene under the existing dev capture-scene layout whose camera looks at a wall region the
lightmap shadows from a selected point or spot light at a grazing view angle, commit its golden, and
confirm the golden differs when the Task 3 toggle forces visibility to 1.

## Sequencing

**Phase 1 (sequential):** Task 1 — every later task reads the channel it publishes on `SpecLight`.

**Phase 2 (sequential):** Task 2 — the only behavioral change; it edits `fs_main` in `forward.wgsl`
and must land before anything toggles or captures it.

**Phase 3 (concurrent):** Task 3, Task 4 — independent files (uniform plumbing vs. capture driver and
scene assets) with no shared edit sites.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No double-counting: a static light's contribution is only ever scaled, never re-added | Task 2 | Task 2 (multiply into the existing `blinn_phong` product; never touch `static_direct` or the union subtraction), Task 1 (none sentinel resolves to 1.0) | AC 2 (force-lit toggle reproduces the pre-change frame byte-for-byte) |
| One shadow technique per light per receiver | Task 2 | Task 2 (`sdf` → SDF slice, `static_light_map` → shadowmask, mutually exclusive branches) | AC 3 |
| Promotion crossfade neutrality: world specular is invariant across a promoted light's weight | Task 1 (channel is level-load constant, sourced from `SpecLight` not per-frame promoted metadata) | Task 1, Task 2 (must not read the promoted metadata in `light_influence`), Task 3 | AC 6 |
| Fixed GPU record strides: `SPEC_LIGHT_SIZE` 64, `UNIFORM_SIZE` 128 | Task 1, Task 3 | Task 1 (reuse `cone_cos.z` pad), Task 3 (reuse `_dyn_pad1`) | AC 7 |
| Absent-data paths degrade to today's render | Task 1 | Task 1 (no atlas / invalid index / dropped channel → none sentinel) | AC 4, AC 5 |

## Rough sketch

**Why the shadowmask and not a derived factor.** Deriving specular occlusion from baked irradiance
divided by an analytic unshadowed irradiance is not viable here: `pack_spec_lights` emits no falloff
model (the 64-byte record carries position/range, premultiplied color, cone direction/type, and cone
cosines only), while `lightmap_bake.rs` bakes `Linear`, `InverseDistance`, and `InverseSquared`
falloff. The shader denominator would disagree with the bake numerator on every non-`Linear` light and
falsely darken fully lit surfaces. It is also aggregate-only — it cannot separate two lights sharing a
texel — and would double-darken against `shadowmask_union_subtraction`. A new baked section is
unnecessary: `shadowmask_bake.rs` already stores exactly the right signal, `LayerTexel::raw_visibility`
(soft area-light visibility in `0..1`, `-1.0` where the light has no direct term), quantized to
`Rgba8Unorm` and 4-colored across non-overlapping lights by `build_shadowmask_from_layers`. The atlas
shares the lightmap's UV space, dimensions, and array layers, and the forward pass already binds and
samples it. So this reuses baked data, adds no compile-time cost, and adds no binding.

**Correctness of the channel read.** `overlap_graph` marks two lights adjacent when both have a mask
entry at the same texel, and `assign_channels_with_drops` colors that graph — so two lights sharing a
channel never both write the same texel, and a channel read at a texel is unambiguous. Texels a light
does not reach keep the `255` initializer and read as fully visible, which matches the loop's own
range/cone/NdotL rejection. Bilinear bleed across a chart edge can mix two channel-sharing lights'
values; this is the same behavior `shadowmask_union_subtraction` already accepts and is not corrected
here.

**Files touched.** `crates/lighting/src/spec_buffer.rs` (channel field, `SPEC_LIGHT_SHADOWMASK_NONE`),
`crates/renderer/src/render/shadowmask.rs` (per-spec-light channel table builder),
`crates/renderer/src/render/renderer_init_resources.rs` and `renderer_resources.rs` (call sites),
`crates/renderer/src/shaders/forward.wgsl` (`shadowmask_visibility_for_spec_light`, specular loop
branch, `Uniforms` field), `crates/renderer/src/shaders/billboard.wgsl` (`Uniforms` tail mirror),
`crates/render-cpu/src/frame_uniforms.rs` (toggle), `crates/postretro/src/capture/driver.rs`
(selection threading).

**Cost.** One extra `textureSample` per (fragment × non-`sdf` static light in the chunk list), on a
texture already resident and already sampled once per fragment by the union path. The sample can be
hoisted: all lights read the same UV and layer, so fetch the `vec4` once before the loop and select a
channel per light inside it — one added sample per fragment total, independent of light count. Prefer
the hoisted form.

## Open questions

- **Coverage gap.** Selected-and-then-dropped lights, and lights below the promotion thresholds, keep
  unshadowed specular. The bright long-range lights whose highlights read worst are exactly the ones
  selection prefers, so this is judged acceptable — but a human should confirm that a dim wall sconce
  with a visible floating highlight is a tolerable residue rather than a blocker. If not, the follow-on
  is a specular-only selection pass decoupled from `EntityShadowLights`, which needs its own channel
  budget.
- **Directional (sun) lights.** `is_promotable_base_light` accepts only point and spot, so a sun's
  specular stays unshadowed. Whether that reads as a defect depends on whether outdoor maps ship with
  strong sun specular; not decided here.
- **Hoisted vs. per-light sample.** The hoisted single-fetch form is recommended above, but it assumes
  the specular loop keeps reading one UV per fragment. If a future change makes specular sample a
  different UV per light, the hoist must be undone.
