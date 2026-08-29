# Billboard Volumetric Direct Lighting

## Goal

Make particle billboards respond to direct lights as smoke, not camera-facing
flat surfaces. A red direct light must keep smoke red as the camera moves
around it, while static-light occlusion and animated brightness still apply.

## Scope

### In scope

- A camera-independent, normal-free direct-scatter field for all particle
  billboards.
- Static `static_light_map` direct transport and animated baked direct deltas.
- Isotropic dynamic point, spot, and directional direct response for billboards.
- Existing probe-depth visibility, light reach, animation scales, and optional
  PRL degradation behavior.
- A fixture and manual GPU check covering both sides of a red light/emitter.

### Out of scope

- Changing world, mover, or mesh lighting.
- Changing indirect-SH ambient behavior, fog scattering, or material lighting.
- Billboard self-shadowing, dynamic shadow-map receipt, or static SDF direct
  policy.
- New emitter KVPs, scripting primitives, or sprite-instance fields.

## Design decisions

1. **Normal-free scatter is the billboard direct-lighting model.** For every
   particle billboard, the renderer samples a normal-free RGB direct-scatter
   field at the sprite centre. It replaces the billboard's baked direct-SH
   surface response for `static_light_map` lights and the old surface-like
   static-specular term; it does not choose another fake surface normal. This
   scatter path is the model for every billboard — there is no per-material split
   — with the legacy-fallback branch (Design decision 6) the only other path.
2. **Bake before cosine convolution.** The base field sums each reaching
   static light's `incident_radiance_at_point` multiplied by the same
   `soft_visibility` used by direct SH. The animated field stores the matching
   per-light peak delta. Both retain light color, falloff, cone, reach culling,
   and baked occlusion without a receiver normal.
3. **Animated scatter composes independently.** Before the billboard draw,
   the renderer writes `base + Σ(active animation scale × delta)` to a 3D
   `Rgba16Float` texture. It uses the section-45 descriptor namespace and the
   same brightness/color/active values, but does not reuse direct-SH tiles.
4. **Dynamic direct has no Lambert cosine.** The billboard loop keeps its
   existing influence, falloff, and spot-cone gates, then adds color times
   attenuation without `dot(N, L)`. Its current unshadowed policy remains.
5. **No double count.** When scatter data is usable, a billboard does not
   sample `DirectShVolumeSection`, and its `static_light_map` entries do not add
   the old surface-like static-specular term — scatter replaces both, with no
   receiver normal in play. The static-specular loop is not deleted from the
   shader: it remains reachable only by the legacy-fallback branch (Design
   decision 6), which reproduces current behavior when scatter data is absent. It
   is not a reservation for any other spec. Static SDF entries keep their current
   path. Meshes and movers keep their direct-SH/promotion behavior unchanged.
6. **Safe legacy fallback.** Both new sections are optional. Missing,
   malformed, or mutually incompatible scatter data selects the exact current
   billboard direct path. If section 45 is present, section 47 must validate
   against it; a missing or invalid section 47 disables the scatter feature as
   a whole instead of silently omitting animated direct light.

## Acceptance criteria

- [ ] `[manual GPU]` In a room lit only by a static red `light_spot`, smoke at
      a fixed position remains red while the camera crosses from one side of
      the emitter to the space between emitter and light. Its brightness may
      change only with particle position, authored animation, or light range —
      not camera direction.
- [ ] `[manual GPU]` The same red-light test remains dark behind an occluding
      wall and respects the spot cone. It does not gain unshadowed red through
      walls.
- [ ] `[unit]` A baked direct-scatter probe equals the sum of normal-free
      visible light radiance; an occluded source contributes zero. The test
      covers point, spot, and directional lights.
- [ ] `[unit]` Animated scatter uses the same descriptor index, active state,
      brightness, and color scale as section 45. At zero scale it is base-only;
      at unit scale it adds exactly the baked peak delta.
- [ ] `[unit]` Section 46/47 round-trip and loader validation reject bad
      version, dimensions, CSR shape, descriptor mapping, or payload length.
      Invalid optional data falls back to legacy billboard direct lighting
      without failing the level load.
- [ ] `[unit]` On the scatter path, the billboard's static-light-map direct has
      no camera-facing `NdotL` or static-specular contribution, and its dynamic
      path retains range/cone rejection with no Lambert cosine. The
      static-specular loop is reached only on the legacy-fallback branch, never
      on the scatter path.
- [ ] `[unit]` Existing direct-SH compose behavior and the billboard
      vertex-stage storage-buffer budget remain unchanged after extracting the
      renderer seam needed for the scatter resource.
- [ ] `[manual GPU]` An animated red baked light pulses billboard scatter with
      the wall/other dynamic receivers, has no camera-side pop, and adds a
      measurable but bounded compose cost to `POSTRETRO_GPU_TIMING`.

## Tasks

### Task 1: Extract the renderer direct-resource seam

Split the direct-atlas/compose ownership out of the 2,142-line
`crates/renderer/src/render/sh_volume.rs` before extending it. Keep the shared
SH bind-group construction and existing direct-SH resources behavior-preserving:
current bindings, Case-1/Case-2 compose dispatch, mesh binding 16, and the
billboard vertex storage-budget guard must keep their contracts. The extracted
seam becomes the owner for the new billboard scatter texture and compose pass.

### Task 2: Add optional direct-scatter PRL sections and loader validation

Register `BillboardDirectScatterVolume` (ID 46) and
`AnimatedBillboardDirectScatterDeltaVolumes` (ID 47) in
`postretro-level-format`. Section 46 carries a versioned base grid mirroring
the section-34 origin, cell size, dimensions, and x-fastest probe order; each
probe is one `Rgba16Float` normal-free RGB value. Section 47 is versioned and
mirrors section 45's affinity factor, dimensions, descriptor-index table, CSR
offsets/light indices, and x-fastest 4×4×4 sub-block order, but each payload
probe is one `Rgba16Float` RGB delta rather than an octahedral tile.

Thread both through `postretro-level-loader::LevelWorld`. Validate section 46
against the base SH grid. Validate section 47 against section 45's affinity
layout and descriptor map. Treat either parse/validation failure, or a missing
section 47 for a map carrying section 45, as absent scatter data and select the
legacy billboard path; do not reject the map.

### Task 3: Bake base and animated normal-free scatter

Add a focused compiler module rather than extending the 2,137-line
`direct_sh_bake.rs`. Reuse the direct baker's static-light filtering, affinity
reach decomposition, `incident_radiance_at_point`, and `soft_visibility` seed
rules. Emit one base RGB value per valid probe for static-light-map sources and
one sparse peak RGB delta per animated baked light/cell. The base and delta
must share the existing grid and section-45 `AnimatedBakedLights` indexing.
Version/cache the new bake outputs independently; changing their radiance or
payload computation invalidates only their cache stages. Append both optional
sections during PRL assembly.

### Task 4: Compose and consume scatter in the billboard pass

Add renderer-owned upload resources for the base 3D scatter texture, an
optional composed texture, and the section-47 CSR buffers. Append the sampled
texture at group 3 binding 17; do not change existing bindings. A compute pass
runs before billboards when animated scatter is present and composes the base
plus active scaled deltas. Its frame ordering matches animated direct-SH
compose, but it never applies static-promotion subtraction because billboards
are not promotion receivers.

In `billboard.wgsl`, depth-aware trilinear interpolation reads the scatter
texture at the sprite centre when usable. On the scatter path, route
static-light-map direct through that result instead of the static-specular term
and the direct-SH surface response, and retain the existing static-SDF behavior.
The static-specular loop stays in the shader but is reached only by the
legacy-fallback branch (Design decision 6); the scatter path does not evaluate
it, and this spec reserves nothing there for another spec. Make runtime dynamic
direct isotropic by removing only the Lambert cosine after existing
influence/range/cone work. Preserve the legacy shader branch when scatter is
unavailable.

### Task 5: Regression fixture and durable documentation

Extend `content/dev/maps/spawner-test.map` with a red-light smoke station that
has a clear camera path from the light-facing side to the space between the
light and emitter, plus a nearby occluded comparison. Exercise the existing
animated alarm fixture too. Document billboard's direct-scatter policy,
legacy fallback, and timing check in `context/lib/rendering_pipeline.md` and
update adjacent billboard-lighting references that still describe all direct
terms as `N = V`.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — renderer seam extraction and format/loader contract do not overlap.

**Phase 2 (sequential):** Task 3 — emits the sections defined by Task 2.

**Phase 3 (sequential):** Task 4 — consumes Task 1's seam, Task 2's loader output, and Task 3's payloads.

**Phase 4 (sequential):** Task 5 — validates the integrated renderer path.

## Wire format

Both new sections are little-endian and optional.

| Section | ID | Header | Payload | Empty encoding |
|---|---:|---|---|---|
| `BillboardDirectScatterVolume` | 46 | `u8 version`, `f32×3 grid_origin`, `f32×3 cell_size`, `u32×3 grid_dimensions` | `f16×4` per x-fastest probe; RGB = normal-free static direct scatter, A = validity mirror | Valid grid with all-zero values; omitted when no static-light-map source exists. |
| `AnimatedBillboardDirectScatterDeltaVolumes` | 47 | `u8 version`, `u8 affinity_factor`, `u32×3 affinity_dims`, `u32 animated_light_count`, `u32×animated_light_count descriptor_indices` | CSR offsets, CSR `AnimatedBakedLights` indices, then `f16×4` per x-fastest probe in each 4×4×4 entry | offsets of zero for every cell, no indices or payload. |

## Boundary inventory

| Name | Rust | Wire | WGSL | FGD KVP |
|---|---|---|---|---|
| Base scatter | `BillboardDirectScatterVolumeSection` | section 46 | sampled 3D texture | n/a |
| Animated scatter | `AnimatedBillboardDirectScatterDeltaVolumesSection` | section 47 | compose CSR buffers | n/a |
| Scatter texture | renderer-owned resource | n/a | group 3 binding 17 | n/a |
| Animation key | `AnimatedBakedLights` index | section-47 descriptor/CSR index | compose descriptor index | existing light animation |

## Cross-spec coordination

This spec makes normal-free scatter the billboard direct model for every
billboard on the scatter path; the old vertex-stage static-specular loop is not
evaluated there and survives only on this spec's own legacy-fallback branch (no
reservation for another spec). The downstream `billboard-specular-shimmer` spec
adds a second, opt-in billboard lighting model (per-fragment specular from a
baked normal map) and owns the per-material split between the two — the
classification flag, the fragment-stage specular path, and the re-scoping of
scatter to the non-shimmer default. That partition is entirely shimmer's to
build. Shimmer depends on this spec only for the established default (scatter for
all billboards); it builds its own per-fragment specular path rather than
inheriting one.

**`billboard.wgsl` / `VertexOutput` merge coordination with `prm-array-layers`.**
The foundational `prm-array-layers` spec restructures `billboard.wgsl` — the
sprite binding becomes `texture_2d_array`, the strip UV math is replaced by
per-layer sampling, and `VertexOutput` gains a flat-interpolated `frame_idx`.
This spec's Task 4 also edits `billboard.wgsl` (the scatter compose/read) and may
grow `VertexOutput`. The two are orthogonal in intent — sprite sampling vs. the
lighting term — but touch the same shader and struct, so this spec's edits rebase
onto the array-migrated shader; it does not reintroduce strip UV math or collide
with the `frame_idx` field. No dependency, only a merge point.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Billboards are normal-free on the scatter path: smoke direct color is independent of camera direction. | Task 3 bakes normal-free transport; Task 4 samples it without `N = V`. | Legacy fallback must remain explicit; on the scatter path, no static-light-map specular or direct-SH double path. | AC 1, 5, 7 |
| Baked occlusion and cone reach remain authoritative. | Task 3 reuses direct-bake radiance, visibility, and reach logic. | Task 4 may only interpolate baked values; it cannot replace them with `spec_lights`. | AC 2, 3 |
| Animated scatter and animated direct SH describe the same light scale. | Task 2 pins the shared descriptor map; Task 3 emits matching deltas. | Task 4 compose uses the shared active/color/brightness values and treats mismatch as absent. | AC 4, 5 |
| No receiver double-counts a physical direct term. | Task 4 replaces the billboard's static-light-map direct SH with scatter on the scatter path. | A billboard on the scatter path uses scatter, not the old specular or direct-SH; the vertex-stage specular loop is confined to the legacy-fallback branch. Static promotion remains mesh-only; SDF and dynamic policies stay disjoint. | AC 5, 7 |

## Open questions

None. The first manual GPU pass may tune authored light intensity only; it does
not change the normal-free transport contract or add an author-facing scale.
