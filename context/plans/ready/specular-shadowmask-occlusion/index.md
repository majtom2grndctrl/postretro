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
- **Directional (sun) lights.** Excluded by the shadowmask's own selection, and correctly so: a sun
  reaches nearly every exterior-facing texel, so it would be adjacent to almost every other light in
  the 4-channel overlap graph and consume a quarter of the budget while raising drop pressure on the
  point and spot lights this plan targets. Their specular stays unshadowed.
- **Animated static lights.** Their direct term lives in the animated atlas, not the shadowmask.
- **Any new baked PRL section.** No wire-format change.

## Acceptance criteria

- [ ] On a map with a shadowmask, a world surface that reads as shadowed from a given static light in
      the lightmap shows no view-dependent highlight from that light at any camera angle; the same
      surface still shows a full highlight where the lightmap reads lit, and the transition across
      the baked penumbra is a continuous ramp, not a hard step. This is a visual criterion, verified
      manually — there is no committed render golden (see AC 8).
- [ ] With the new dev "force specular visibility to 1" toggle on, a captured frame is byte-identical
      to the same frame captured before this change — proving the change only ever multiplies an
      existing term and never adds light. Checked as an explicit manual A/B gate: capture the
      pre-change baseline (on `main`) versus the branch with the toggle on and assert byte-identity;
      no committed baseline image or golden; AC 8 is the opposite-direction manual check (toggle off, the
      shadowed region visibly darkens). Capture the pre-change baseline and the toggle-ON branch frame
      on the same GPU adapter — cross-adapter rounding of the unchanged `blinn_phong` / `atten` / `cone`
      math can break bit-identity even though the code is arithmetically equivalent.
- [ ] A light authored with `_shadow_type sdf` renders identically to before, including inside its
      SDF-shadowed regions; no static light is darkened by two visibility signals at once.
- [ ] A static light that the compiler does not select for the shadowmask (dim, short-range,
      directional, animated, decorative spot fixture, or dropped by channel assignment) renders
      identically to before.
- [ ] A map with no `ShadowmaskAtlas` section renders identically to before. So does a map whose
      section is present but rejected by `filter_usable_shadowmask_section` (device-limit failure):
      the rejection binds the `[255,255,255,255]` placeholder, which samples 1.0 on every channel and
      so renders unshadowed, identically to before.
- [ ] While a mover walks through a promoted static light's influence and the promotion crossfade runs
      to completion in both directions, no world-surface highlight changes brightness, pops, or
      flickers. The discriminating surface is one the promoting light LIGHTS (a real highlight present
      at a grazing angle), not a shadowed one where `baked_vis ≈ 0` masks a `× weight` coupling; world
      specular is invariant across the promotion weight BY CONSTRUCTION — the specular visibility is
      sourced from level-constant `SpecLight.cone_cos.z`, and Task 2 forbids the specular loop from
      reading the per-frame `light_influence` metadata; because the capture harness is single-frame
      with empty `light_influences`, this is verified by that source prohibition plus the construction
      argument, not by a rendered weight-sweep.
- [ ] The forward pipeline creates the same number of bind groups, samplers, storage buffers, and
      textures as before; no per-stage binding budget moves. The pipeline-budget test and the
      `UNIFORM_SIZE == 128` / `SPEC_LIGHT_SIZE == 64` size assertions still hold. Adding the
      `FrameUniforms` field changes the source of the byte-124 tail-is-zero tests — they are updated
      for the new field (defaulting false), not left unchanged. Of these clauses, the texture-count
      one is backed by a runnable test (the pipeline-budget test fails on any new fragment texture
      binding); the bind-group / sampler / storage-buffer clauses are review-level, and the design
      adds none.
- [ ] The shadowed-highlight case is verified as a manual A/B capture (no committed golden image or
      golden-diff harness), paired with AC 2 as the opposite direction: in the committed
      shadowmask-bearing grazing-angle scene, with the force-lit toggle OFF the shadowed region's
      highlight is visibly darker than with the toggle ON, proving the occlusion multiply actually
      reduces the highlight where the lightmap reads shadowed. Fails if the multiply is a no-op. Any
      rendered form of this check is GPU-adapter-gated and does not run in default CI.

## Tasks

### Task 1: Carry the shadowmask channel on every static light record

Give each packed `SpecLight` its shadowmask channel so the forward specular loop can find it without a
new binding. `crates/lighting/src/spec_buffer.rs` packs a fixed 64-byte record whose final slot
(`cone_cos`) uses only `.x` (cos inner) and `.y` (cos outer); `.z` and `.w` are zero padding today.
Write the channel into `.z` as a float: `0.0..3.0` selects an RGBA channel of the shadowmask atlas,
and a new `SPEC_LIGHT_SHADOWMASK_NONE = 4.0` sentinel means "no baked visibility, treat as fully
lit" (this equals the existing WGSL `SHADOWMASK_CHANNEL_DROPPED` value, 4.0 — the two are the same
threshold, not different ones). The record stays 64 bytes — do not reorder fields.
`pack_spec_lights` gains a second parameter: a slice of channel bytes indexed in the same compacted
`!is_dynamic` order the function already emits, with
`postretro_level_format::shadowmask_atlas::SHADOWMASK_CHANNEL_DROPPED` (`0xFF`) meaning none. Build
that slice renderer-side in `crates/renderer/src/render/shadowmask.rs` from three inputs already on
`LevelGeometry` (`renderer_types.rs`): `entity_shadow_lights`, `shadowmask_atlas` (whose `channels[i]`
aligns with `entity_shadow_lights[i]`), and the atlas's presence. The presence input MUST be
`geometry.shadowmask_atlas.is_some()` — the PRL-section presence available at the `pack_spec_lights`
call site — and this task explicitly FORBIDS keying on `full.shadowmask_present`: in the
`install_level_geometry` reload path, `full.shadowmask_present` is set `false` early and only resolved
after `LightmapResources::new` runs, i.e. after `pack_spec_lights` has already packed the channels, so
reading it at the pack site yields a stale `false` and emits the none sentinel for every light,
silently no-opping the feature on every map. This is safe against the resource-drop case: a section
present but rejected by `filter_usable_shadowmask_section` binds the `[255,255,255,255]` placeholder
atlas, which samples to 1.0 (fully lit) on any channel, so a valid channel over a placeholder still
degrades to today's render (AC5). Reuse the existing `build_selection_spec_light_indices` helper for
the selection-index → spec-index mapping — it already encodes the `!is_dynamic` compaction, so never
index with a global light index. When the atlas is absent, when a selection entry maps to
`FORWARD_SHADOWMASK_INVALID_INDEX`, or when the channel is `0xFF`, emit the none sentinel. The atlas's
`channels[i]` is in selection order (aligned to `entity_shadow_lights[i]`), but `pack_spec_lights`
consumes the slice in spec-index order over all `!is_dynamic` lights, so the builder contract is a
scatter, not a copy: allocate the output slice with length equal to the number of `!is_dynamic`
lights; initialize every entry to `0xFF` (the dropped/none marker); for each selection index `i` whose
spec index `s = build_selection_spec_light_indices()[i]` is not `FORWARD_SHADOWMASK_INVALID_INDEX`,
set `out[s] = atlas.channels[i]`; absent atlas leaves all entries `0xFF`. Selection-order in,
spec-index-order out. Both `pack_spec_lights` call sites must pass the new slice:
`crates/renderer/src/render/renderer_init_resources.rs` (~line 336) and
`crates/renderer/src/render/renderer_resources.rs` (~line 274). Keep `postretro-lighting` wgpu-free —
it receives a plain byte slice, it does not compute the mapping. Foreclose the P3 trap by construction: the channel-slice builder in `shadowmask.rs` reads
`geometry.shadowmask_atlas.is_some()` itself and is never handed `full.shadowmask_present` or any
precomputed presence bool, so the stale-`false` reload-window value is not reachable from it. Guard it
with two pure tests, no GPU (they run in default CI): (a) a builder unit test — given a geometry
carrying `Some(shadowmask_atlas)` with a selected light and a dynamic light preceding it in
`world.lights`, assert the selected light's spec-index slice entry decodes to its authored channel
(0..3), not the `0xFF` none marker (this is the same pure builder path Task 4's alignment assertion
uses); and (b) a `pack_spec_lights` unit test that feeds a channel slice and asserts the selected
record's `cone_cos.z` (bytes 56..60) decodes to that channel, not the `SPEC_LIGHT_SHADOWMASK_NONE`
sentinel. A full `install_level_geometry` integration test would be GPU-adapter-gated (it creates wgpu
buffers) and is not relied on here — P3 is foreclosed structurally and guarded by the pure builder
test.

### Task 2: Multiply static specular by its baked visibility in the forward pass

In `crates/renderer/src/shaders/forward.wgsl`, extend the `SpecLight` WGSL struct comment to document
`cone_cos.z`, then resolve a per-light visibility inside the `use_specular` chunk-light loop. `fs_main`
must sample `shadowmask_atlas` (already declared at `@group(4) @binding(6)`, already an array texture)
through `lightmap_filtering_sampler` ONCE, above the `use_specular` loop, at `in.lightmap_uv` /
`i32(in.lightmap_layer)` — mirroring how `sdf_factor` is already sampled once outside the loop (via
`upsample_shadow_factor`). The chunk loop's trip count (`chunk_count`) is non-uniform, so an
implicit-LOD `textureSample` inside it is a WGSL uniformity hazard. Add a helper
`shadowmask_visibility_for_spec_light(sl, mask)` that takes the already-sampled `vec4` and returns 1.0
when `round(sl.cone_cos.z) >= SHADOWMASK_CHANNEL_DROPPED` — reusing the existing WGSL constant rather
than introducing a new threshold — and otherwise selects the channel via
`shadowmask_channel_value(mask, u32(round(sl.cone_cos.z)))`. The loop currently computes `visibility`
via `select(...)` over `sdf_visibility_for_light` with `sdf_force_lit || !is_sdf` forcing 1.0; replace
that with an explicit two-branch choice keyed on `sdf_select_is_sdf(sl)` — `sdf` lights keep the SDF
slice exactly as today, non-`sdf` lights take the shadowmask value — so exactly one visibility signal
ever applies to one light. `sdf_force_lit` must keep forcing 1.0 on the SDF branch only. The result
multiplies `blinn_phong(...)` alongside `atten * cone`; nothing is added to `total_light` that was not
there before. The shadowmask visibility applies only when `use_lightmap` is true; gate the apply on
`use_lightmap` so isolation modes 1 (`NoLightmap`), 3 (`IndirectOnly`), and 9 (`SpecularOnly`) — where
`use_specular` is true but `use_lightmap` is false — keep today's unshadowed specular, and their debug
captures stay byte-identical to pre-change. Do not touch `static_direct`, `shadowmask_union_subtraction`,
or the dynamic light loop. Do not read the promoted-record metadata appended to `light_influence` —
that metadata is per-frame and promotion-weighted, and using it here would make the highlight
crossfade with promotion. The shadowmask channel comes from `SpecLight` only, which is level-load
constant.

### Task 3: Dev force-lit toggle and uniform plumbing

Add a `spec_shadowmask_force_one` dev toggle that forces the Task 2 visibility to 1.0, mirroring the
existing `sdf_force_visibility_one` knob, so the A/B acceptance check is repeatable. On the WGSL side,
the group-0 `Uniforms` struct has a trailing `_dyn_pad1: u32` slot; repurpose it rather than growing
the struct — `UNIFORM_SIZE` in `crates/render-cpu/src/frame_uniforms.rs` is exactly 128 and wgpu
rejects the pipeline if the CPU size and WGSL-derived stride drift. `FrameUniforms` in
`crates/render-cpu/src/frame_uniforms.rs` has no pad field to repurpose — it ends at
`total_light_count`, and bytes 124..128 are written implicit-zero (`// 124..128 stays zero`) — so on
the Rust side, ADD a `spec_shadowmask_force_one` field to `FrameUniforms`, encode it at bytes 124..128
in `build_uniform_data` (adding a real write at 124..128, replacing the `// 124..128 stays zero`
implicit-zero pad), and update every
`FrameUniforms { .. }` constructor, since the struct has no `Default`. The struct is a four-way
contract: update the Rust writer as above, and all three shaders that declare the tail —
`crates/renderer/src/shaders/forward.wgsl`, `crates/renderer/src/shaders/billboard.wgsl`, and
`crates/renderer/src/shaders/wireframe.wgsl`, whose own comment states it holds its stride "in
lockstep with forward.wgsl (128 bytes)". Name the slot by offset (124..128), not by field name: it is
`_dyn_pad1` in `forward.wgsl` and `billboard.wgsl`, but `_dyn_pad3` in `wireframe.wgsl` (whose own
`_dyn_pad1` sits at a different offset, 116). Only `forward.wgsl` reads the field, so only it MUST
change for correctness; the `billboard.wgsl`/`wireframe.wgsl` renames are cosmetic hygiene, not a
correctness requirement — WGSL uniform layout is fixed by offset and type, not field name, so renaming
a pad field changes zero bytes. Thread the flag from the dev-tools Diagnostics
panel the same way `sdf_force_visibility_one` is threaded, and add a headless test asserting the flag
encodes at byte 124 with `force = true`, mirroring the existing
`uniform_data_encodes_sdf_force_visibility_one_at_correct_offset` test. Update every existing tail-zero
assertion that spans byte 124 (there are several — `data[104..128]`, `data[120..128]`,
`data[124..128]`) to add the new field to their `FrameUniforms` literals; each still passes with the
field defaulting `false`. Note the `assert_eq!(data.len(), UNIFORM_SIZE)` check is tautological (`data`
is `[u8; UNIFORM_SIZE]`) and is not the 128-byte drift guard — the real guard is the
WGSL-stride-vs-wgpu-pipeline rejection cited above. `build_uniform_data` writes
the field every frame from the `FrameUniforms` field, mirroring `sdf_force_visibility_one` — the
uniform buffer is reused across frames, so a conditional write would leave a stale force-one set, and
clearing the toggle would not take effect until something else happened to overwrite the byte. Update
the existing "3-way contract" comments in `forward.wgsl` and `billboard.wgsl` (which omit wireframe) to
a 4-way contract.

### Task 4: Headless capture coverage

Make the frame-capture path able to reproduce the bug and its fix for the manual A/B gates (AC 2 and
AC 8); no golden image or golden-diff harness is committed. Today
`crates/postretro/src/capture/driver.rs` overrides `entity_shadow_lights: &[]` in the `LevelGeometry`
it hands to `install_level_geometry`, so no capture ever carries a shadowmask channel table. Thread the
loaded world's selection through instead. Watch the index space: `entity_shadow_lights` holds indices
into the full `world.lights` list, while `capture_static_lights` hands the renderer a pre-filtered
`!is_dynamic` copy — passing the filtered list alongside global selection indices misaligns them the
moment any dynamic light precedes a selected one. Pass the unfiltered `world.lights` (packing applies
its own `!is_dynamic` filter, so the emitted `spec_lights` bytes are unchanged) or remap the indices
explicitly. Assert the alignment in a pure test — call the Task 1 channel-slice builder and
`build_selection_spec_light_indices` directly with `world.lights` / `entity_shadow_lights` / the atlas,
no GPU — covering both the `spec_lights` bytes and the channel table: on a map with a dynamic light
preceding a selected static light, assert the known selected light resolves to its authored channel
after threading (a `spec_lights`-bytes-only check would miss a shifted channel table). Author and
commit a capture scene — a shadowmask-bearing PRL that shadows a wall from a selected point or spot
light, plus a camera looking at that wall at a grazing angle — following the inline-scene-JSON pattern
in `tests/capture_frame.rs` (there is no committed capture-scene layout to reuse). That scene makes the
two manual A/B comparisons repeatable: with the Task 3 toggle ON the frame is byte-identical to
pre-change (AC 2), and with the toggle OFF the shadowed region is visibly darker (AC 8). Any rendered
capture check is GPU-adapter-gated like the existing `tests/capture_frame.rs` and does not run in
default CI.

## Sequencing

**Phase 1 (sequential):** Task 1 — every later task reads the channel it publishes on `SpecLight`.

**Phase 2 (sequential):** Task 2 — the only behavioral change; it edits `fs_main` in `forward.wgsl`
and must land before anything toggles or captures it.

**Phase 3 (sequential):** Task 3 — uniform plumbing for the force-lit toggle.

**Phase 4 (sequential):** Task 4 — depends on Task 2 (the occlusion to observe) and Task 3 (the toggle
its manual A/B compares against). Task 4's driver-threading and pure alignment test do not need Task 3,
but its manual A/B toggle comparison does, so Task 4 runs after Task 3 rather than concurrently with
it.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No double-counting: a static light's contribution is only ever scaled, never re-added | Task 2 | Task 2 (multiply into the existing `blinn_phong` product; never touch `static_direct` or the union subtraction), Task 1 (none sentinel resolves to 1.0) | AC 2 (force-lit toggle reproduces the pre-change frame byte-for-byte) |
| One shadow technique per light per receiver | Task 2 | Task 2 (`sdf` → SDF slice, `static_light_map` → shadowmask, mutually exclusive branches) | AC 3 |
| Promotion crossfade neutrality: world specular is invariant across a promoted light's weight | Task 1 (channel is level-load constant, sourced from `SpecLight` not per-frame promoted metadata) | Task 1, Task 2 (must not read the promoted metadata in `light_influence`), Task 3 | AC 6 |
| Fixed GPU record strides: `SPEC_LIGHT_SIZE` 64, `UNIFORM_SIZE` 128 | Task 1, Task 3 | Task 1 (reuse `cone_cos.z` pad), Task 3 (write the added `spec_shadowmask_force_one` into the 124..128 tail pad; stride stays 128) | AC 7, `spec_light_size_is_64` (`spec_buffer.rs`) |
| Absent-data paths degrade to today's render | Task 1 | Task 1 (no atlas / invalid index / dropped channel → none sentinel) | AC 4, AC 5 |

## Pin table

Orderings the implementation must satisfy (human reference; not delivered to task agents). P9 is exercised by Task 4's pure alignment test; P7's byte-identity slice is the AC 2 toggle-ON check,
while its mid-crossfade / no-stale-persistence content holds by construction (Task 3's unconditional
per-frame write, Task 2's `light_influence` prohibition); P3 is foreclosed by construction (the builder
reads `geometry.shadowmask_atlas.is_some()`) and guarded by Task 1's pure builder test; the remaining
rows hold by construction.

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Reload with a valid shadowmask, resource kept | `install_level_geometry`: `shadowmask_present` set `false` early -> `pack_spec_lights` packs channels -> `shadowmask_present` resolved later from `lightmap_resources`. Frame renders after install. | `SpecLight.cone_cos.z` for a selected light holds its RGBA channel (0..3), packed from `geometry.shadowmask_atlas.is_some()`, not `full.shadowmask_present`. World specular is shadowed on the first rendered frame. |
| P2 | Section present, resource dropped to placeholder | PRL carries a `ShadowmaskAtlas` but `filter_usable_shadowmask_section` rejects it -> `shadowmask_present=false`, placeholder `[255,255,255,255]` bound. Channel packed valid from section presence; frame samples placeholder. | Byte-identical to today (AC 5): every channel of the placeholder samples 1.0, specular unshadowed. |
| P3 | Implementer keys Task 1 on `shadowmask_present` (the trap) | As P1 but the builder reads `full.shadowmask_present` (stale `false`) at the pack site. | Every channel = none sentinel -> feature no-ops on all maps; AC 1 and AC 8 fail. Foreclosed by construction — the builder reads `geometry.shadowmask_atlas.is_some()`, so the stale flag is unreachable — and guarded by Task 1's pure builder test. |
| P4 | Specular must not leak through baked shadow (light not promoted) | A selected static light is present but not promoted (no promoted record; weight 0.0 == record absent). Camera on a world surface the lightmap reads as shadowed from that light. | No highlight from that light on the shadowed surface (shadowmask channel ~0 -> visibility 0). Guards specular leaking through baked shadow; distinct from the crossfade-neutrality case, which P10 covers on a lit surface. |
| P5 | Zero-duration crossfade | Promotion authored at 0 duration: weight jumps 0->1 in one tick; `promoted_static_records` populated same tick. | World specular unchanged across the jump (reads level-constant `SpecLight`). AC 6 holds at the zero-duration limit, not only a smooth ramp. |
| P6 | Static light both static-lit and promoted same frame | Selected static light is in `spec_lights` (world path) and appended to `light_influence` metadata (mover path) on one frame. | World receiver: specular from `SpecLight` baked mask; mover receiver: from the promoted shadow-map record, weight-faded. No double-count on any single receiver; world result promotion-independent. |
| P7 | Force-lit toggle flipped mid-crossfade | `spec_shadowmask_force_one` set at frame N (written unconditionally into uniform bytes 124..128) while a promotion crossfade is mid-transition. | Frame N: world specular forced to 1.0 (byte-identical to pre-change, AC 2); mover crossfade unaffected. After clearing: re-shadowed next frame; no stale force-one persists. |
| P8 | Isolation modes where lightmap off but specular on | Render with `lighting_isolation` in {1 NoLightmap, 3 IndirectOnly, 9 SpecularOnly}: `use_specular` true, `use_lightmap` false. | Shadowmask visibility is gated on `use_lightmap`, so these modes keep unshadowed specular; debug captures byte-identical to pre-change. |
| P9 | Capture with dynamic light preceding a selected static | Task 4 threads selection: `world.lights = [dynamic, static_selected, ...]`, `entity_shadow_lights = [1]` (global). Driver passes unfiltered `world.lights` (or remaps). | `build_selection_spec_light_indices` maps selection[0] -> spec index 0 (dynamic compacted out); the channel table aligns to the same compacted index. Test asserts the selected light resolves to its authored channel. |
| P10 | Promotion crossfade neutrality, LIT world surface | Selected static light promotes as a mover enters; per-frame metadata writes weight 0->1 to `light_influence`. Camera fixed on a world surface the light lights (baked_vis high, grazing highlight present). | Highlight pixels invariant across weight {0.0, 0.5, 1.0}. Fails iff the specular loop multiplies `blinn_phong` by anything sourced from `light_influence` (e.g. the record weight) -- the crossfading-highlight defect the neutrality invariant forbids. Verified by construction + Task 2's prohibition on reading `light_influence` (the single-frame capture harness cannot render a weight sweep). |

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
`crates/renderer/src/shaders/wireframe.wgsl` (`Uniforms` tail rename),
`crates/render-cpu/src/frame_uniforms.rs` (toggle), `crates/postretro/src/capture/driver.rs`
(selection threading).

**Cost.** One extra `textureSample` per (fragment × non-`sdf` static light in the chunk list), on a
texture already resident and already sampled by the union path (same texture, sampler, UV
`in.lightmap_uv`, and layer). The sample can be
hoisted: all lights read the same UV and layer, so fetch the `vec4` once before the loop and select a
channel per light inside it — one added sample per fragment total, independent of light count. Use the
hoisted form. It depends on every light in the loop reading the same UV and layer, which holds today
because world specular samples the fragment's own lightmap coordinates; leave a comment at the fetch
site saying the hoist must be undone if specular ever samples a per-light UV.

## Open questions

None.

**Coverage gap — accepted, not deferred.** Lights outside the `EntityShadowLights` selection keep
unshadowed specular. This is a partial fix to a limitation `forward.wgsl` already documents and
already ships with, so every uncovered light renders exactly as it does today and none renders worse.
Selection prefers bright, long-range, shadow-casting lights — the ones whose highlights read worst —
so the fix lands where the artifact is most visible. A dim wall sconce retaining a faint highlight is
consistent with an engine whose stated aesthetic uses modern embellishments sparingly; specular is an
embellishment here, not a load-bearing cue. Ship the partial fix and record the residue as a narrowed
known limitation. A specular-only selection pass decoupled from `EntityShadowLights` remains possible
later, but it needs its own channel budget and should not gate this.

**Directional lights — a correct exclusion, not an oversight.** The shadowmask carries four channels
assigned by graph coloring over a texel-overlap graph, with lights dropped when the graph needs more
than four colors. A sun reaches nearly every exterior-facing texel, so it would be adjacent to almost
every other light in that graph, consume a channel outright, and raise drop pressure on exactly the
point and spot lights this plan targets. Spending a quarter of the budget to shadow the one light type
whose specular the engine is least likely to lean on — the aesthetic is interior-heavy cyberpunk, and
one dev fixture map currently authors a `light_sun` at all — is the wrong trade. Directional stays out
of scope on that reasoning, not pending a judgment call.
