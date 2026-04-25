# Lighting — Animated SH Volumes

> **Status:** draft
> **Related:** `context/lib/rendering_pipeline.md` §4, §7.1–7.5 · `context/plans/in-progress/fx-volumetric-smoke/`

---

## Goal

Make animated lights affect entities (billboard sprites), surfaces, and volumetric fog
uniformly through a composed SH irradiance volume that is updated each frame. Fix the
double-counting of static lights between the SH base volume and the lightmap. Fix specmap
evaluation being silenced by the lighting isolation mode.

Without this plan: a flickering panel light can animate surface irradiance (via animated
weight maps) but cannot tint a nearby character or cast animated ambient scatter into fog.
With it: the same animation curve drives all three consumers from a single shared signal.

---

## Scope

### In scope

- **SH bake — indirect only.** Rebake the static SH volume to capture only
  bounced (multi-bounce) illumination from static lights. Static lights currently bake into
  both the lightmap (direct) and the SH volume (direct + indirect), causing surfaces to
  be doubly lit in Normal mode. After this change: lightmap = direct, SH = indirect only.
  Animated and dynamic lights are unaffected by this change.

- **Delta SH volumes — bake.** For each animated light, `prl-build` bakes a
  per-light delta SH volume representing that light's contribution at peak brightness
  (brightness = 1.0, base color). New PRL section. Each delta volume covers only that
  light's influence sphere AABB, at a fixed probe spacing (default 1.0m, bake-time
  configurable). A small panel light (5m radius) yields ~125 probes; a wide ambient fill
  gets proportionally more. The runtime compose pass interpolates during sample.

- **Delta SH volumes — PRL section.** New PRL section (`DeltaShVolumes`) that stores
  header metadata (per-light AABB origin, cell size, grid dimensions, animated light count,
  per-light `AnimationDescriptorIndex`) and per-light probe data in f16 format.

- **Compose pass.** A pre-frame GPU compute pass runs before the depth prepass. It
  reads the static base SH textures and per-light delta data, evaluates animated curve
  weights for the current frame time, and writes composed total SH bands to the 3D
  textures that all consumers (forward, billboard, fog) already sample via group 3. No
  changes to consumer shaders.

- **Specmap isolation fix.** In `forward.wgsl`, split the `use_direct` flag into two
  independent booleans: one gates the lightmap sample, the other gates the specular
  buffer loop. Specmaps now respond in all lighting isolation modes.

### Out of scope

- Animated lights casting hard shadow boundaries on surfaces. The existing animated weight
  map path already handles this for qualified surface texels (up to 4 animated lights per
  chunk); that path is unchanged.
- Volumetric spot shafts from animated baked lights. Shaft casting requires a dynamic
  light in the spot shadow slot budget; animated baked lights are not in that budget.
  Animated lights contribute to fog ambient scatter via composed SH (L0 term), not shafts.
- Increasing `MAX_ANIMATED_LIGHTS_PER_CHUNK` or changing animated weight map capacity.
- Dynamic light pipeline changes.
- Color cycling that requires full L1–L2 SH per probe for the animated delta contribution.
  The delta volumes carry the full 9-band SH, so directional color response is preserved;
  this note is to clarify that only the delta magnitude and hue change per frame — the
  spatial distribution is baked.
- Configurable ambient floor — out of scope; a separate future task.

---

## Acceptance Criteria

- [ ] In Normal lighting mode, a scene lit only by static lights is not visibly brighter
  than the same scene in IndirectOnly mode (SH) or DirectOnly mode (lightmap) summed
  mentally — confirming the double-counting is resolved.
- [ ] A billboard sprite positioned inside the influence sphere of an animated light
  visibly changes brightness/color as the animation plays.
- [ ] The fog ambient scatter color shifts with a running animated light. (Observable with
  `env_fog_volume` brush in the test map and a nearby animated panel light.)
- [ ] Specular highlights are visible in IndirectOnly lighting isolation mode (Alt+Shift+4
  mode 2). They are silent today.
- [ ] `cargo test -p postretro -p postretro-level-compiler -p postretro-level-format` passes.
- [ ] No new `unsafe`.
- [ ] `cargo clippy -- -D warnings` clean.
- [ ] A map compiled without any animated lights loads and renders identically to before
  (empty delta section degrades gracefully to compose pass writing base data unchanged).

---

## Tasks

### Task A — Indirect-only SH base bake

**Crate:** `postretro-level-compiler` · **File:** `src/sh_bake.rs`

Change the probe raytracer to accumulate only reflected (albedo-weighted) radiance at each
ray hit rather than incident radiance. The current loop evaluates the direct irradiance at
a hit surface, which is the same quantity the lightmap records. The corrected loop:
for each ray that reaches a surface, multiply the evaluated irradiance by the surface's
estimated albedo before projecting into the SH bands. This converts the SH bake from
"what does the probe see as incident radiance" to "what bounced off surfaces into the
probe," which is the indirect component.

Albedo estimation: use a fixed constant (0.45 per channel) — update `BOUNCE_ALBEDO` in
`sh_bake.rs` to this value. Per-face texture color is not accessible at bake time (see Q4
decision below). The exact value affects brightness but not correctness of the separation.

This task changes the baked output. Existing `.prl` files must be recompiled after this
lands. The change is isolated to the bake side; the runtime consumes the same section
format.

### Task B — PRL section: DeltaShVolumes

**Crate:** `postretro-level-format` · **New file:** `src/delta_sh_volumes.rs`

Define a new PRL section type (assign next available `SectionId` integer) for delta SH
volumes. Section layout:

- Header: version, animated light count, `AnimationDescriptorIndex` table (one u32 per
  animated light → index into the SH section's descriptor array).
- Per-light grid header: AABB origin, cell size, and grid dimensions for that light's delta
  volume (each light has its own AABB derived from its influence sphere).
- Per-light probe data: the same 9-band f16 RGB layout as the base SH section. Each probe
  stores 27 × f16 values (9 bands × RGB).
- Probe order is Z-major then Y then X within each per-light grid.

The probe spacing is fixed at bake time (default 1.0m, configurable via `--delta-spacing`).
Each light's grid is sized to cover its influence sphere AABB at that spacing. A small panel
light at 5m radius produces an ~11×11×11 (≤1331 probes) worst-case, typically a tighter
sphere clip; a wide ambient fill at 20m radius produces ~41×41×41. The runtime performs
trilinear interpolation within each per-light AABB during the compose pass.

### Task C — Delta SH volume baking

**Crate:** `postretro-level-compiler` · **New file:** `src/delta_sh_bake.rs`

For each animated light: compute the light's influence sphere AABB, place probes at the
configured fixed spacing (default 1.0m) within that AABB, then run the same ray-tracing
loop used by the base SH bake with only that one light active and brightness fixed at 1.0
(peak contribution). Store the result as a per-light delta SH probe grid. The bake is
embarrassingly parallel across lights (each is independent).

The base SH baker and delta baker share the same BVH, primitive list, and ray-generation
helpers. Extract shared helpers if not already separated.

The composed result at runtime must satisfy: `compose(base, delta[i] × 0) == base` and
`compose(base, delta[i] × 1) == what the base would be if light i were always at peak`.

Task C depends on Task B (section format) and can be developed in parallel with Task A.

### Task D — Compose pass (runtime)

**Crate:** `postretro` · **New files:** `src/render/sh_compose.rs`,
`src/shaders/sh_compose.wgsl`

A GPU compute pass that runs once per frame before the depth prepass (in the visibility
and culling prepasses block at §7.1). The pass:

1. Binds the static base SH 3D textures (read-only), the delta volume data (storage
   buffer or array texture), the animation descriptor buffer and curve samples (from
   group 3 binding 11–12), and the current game time.
2. For each probe in the output grid, reads the base SH bands; for each animated light,
   reads the corresponding delta probe (with trilinear fetch from the coarser delta grid),
   evaluates the animation curve at the current time using the shared Catmull-Rom helper,
   multiplies delta bands by the curve result, and accumulates into the total.
3. Writes the total SH bands into a separate `sh_total_band0..8` set of 3D textures created
   at load time alongside the base textures. Consumer bind groups (group 3) point at the
   total set rather than the base set; `ShVolumeResources` must be updated to create both
   sets and expose the total-set bind group to the forward, billboard, and fog pipelines.

Because all SH consumers (forward, billboard, fog) already sample `sh_band0..8` via
group 3, no consumer shader changes are required. The compose pass is the only point of
change in the render pipeline.

When no animated lights are present (delta section absent or empty), the compose pass
copies base to total unconditionally — one compute dispatch that amounts to a 3D texture
blit. This is cheap (~300KB blit) and keeps the code path uniform.

Task D depends on Task B (section format for loading delta data) and Task C (data to
compose). Can be developed with a zeroed delta buffer before Task C lands, confirming
consumer correctness independently.

### Task E — Split `use_direct` flag

**Crate:** `postretro` · **File:** `src/shaders/forward.wgsl`

Rename the existing single `use_direct` boolean into three independent booleans:
`use_lightmap`, `use_specular`, and `use_dynamic`. The current `use_direct` gates the
lightmap sample, the specular accumulation block, and the dynamic-light loop count — all
three must be separated. Update the lighting isolation switch:

| Mode | `use_lightmap` | `use_specular` | `use_indirect` | `use_dynamic` |
|------|---------------|----------------|----------------|---------------|
| Normal (0) | true | true | true | true |
| DirectOnly (1) | true | true | false | true |
| IndirectOnly (2) | false | true | false | false |
| AmbientOnly (3) | false | false | false | false |

Specular was previously silenced in IndirectOnly mode because it shared the `use_direct`
flag with the lightmap sample. IndirectOnly was the best-looking mode and should have
working specmaps. AmbientOnly is a minimal debug view; specular off there is intentional.

Also update `src/render/mod.rs` where the isolation uniform value is packed.

Note: Task F extends this switch to 9 modes; both tasks edit `forward.wgsl` and
`render/mod.rs`. Land Task E first; Task F applies on top.

This task is independent of all others.

### Task F — Extended Lighting Isolation Modes

**Crate:** `postretro` · **Files:** `src/render/mod.rs`, `src/shaders/forward.wgsl`

Redesign the `LightingIsolation` enum from the current 4-value sequential enum to a wider
set of named diagnostic modes. After this plan ships, the forward shader has six distinct
contributing terms: ambient floor, static lightmap, static SH (indirect), animated SH
delta, dynamic direct, and specular. A mode per term lets the user isolate each
contribution independently.

Update `LightingIsolation` and the forward shader isolation logic to the following modes:

| Mode | Name | Ambient | Lightmap | Static SH | Animated ΔSH | Dynamic | Specular |
|------|------|---------|----------|-----------|--------------|---------|---------|
| 0 | Normal | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 1 | DirectOnly | ✓ | ✓ | — | — | ✓ | ✓ |
| 2 | IndirectOnly | ✓ | — | ✓ | ✓ | — | ✓ |
| 3 | AmbientOnly | ✓ | — | — | — | — | — |
| 4 | LightmapOnly | ✓ | ✓ | — | — | — | — |
| 5 | StaticSHOnly | ✓ | — | ✓ | — | — | — |
| 6 | AnimatedDeltaOnly | ✓ | — | — | ✓ | — | — |
| 7 | DynamicOnly | ✓ | — | — | — | ✓ | — |
| 8 | SpecularOnly | ✓ | — | — | — | — | ✓ |

The Alt+Shift+4 chord cycles through modes 0–8 in order. The existing
`cycle_lighting_isolation()` method in `src/render/mod.rs` wraps at 9 instead of 4. The
uniform value stays a u32; the forward shader uses a switch/match to enable each term.

Note: modes 5 (StaticSHOnly) and 6 (AnimatedDeltaOnly) are only meaningful after Task D
lands. Before Task D they both show the base SH result (there is no separate animated delta
term yet).

This task is independent of all others.

---

## Sequencing

**Phase 1 (concurrent):** Task A (indirect-only bake), Task B (PRL section format),
Task E then Task F (both edit forward.wgsl and render/mod.rs — land E first, F applies
on top; can develop concurrently, merge sequentially).

**Phase 2 (concurrent):** Task C (delta baking — needs Task B PRL format), Task D
(compose pass stub — needs Task B for loading; can stub with zero deltas to validate
consumer correctness independently).

**Phase 3 (sequential):** Task D full (wire Task C delta data into Task D's compose pass
once delta baking lands). Close acceptance criteria.

---

## Rough Sketch

**Compose pass dispatch geometry.** One thread per output probe. For a 32×16×8 base grid,
that is 4096 threads — trivially one 8×8×8 dispatch. Curve evaluation per probe: O(N_lights),
typically ≤ 16.

**Delta texture packing.** Delta probe data stored as a flat storage buffer (`array<vec4<f32>>`
or `array<f16>` via packed u32s) indexed by `light_index * delta_probe_count + probe_index`.
This avoids 3D texture array (unsupported in wgpu base) and keeps one binding per section.

**Curve reuse.** The animated-lightmap compose pass (`animated_lightmap_compose.wgsl`) already
evaluates Catmull-Rom curves per animated light using the shared `curve_eval.wgsl` helper.
The SH compose pass can share the same helper. Both passes read from the same group-3
descriptor and sample buffers (bindings 11–12) — no new curve infrastructure needed.

**Memory.** With per-light AABB baking at 1.0m spacing: a small panel light at 5m radius
produces a 10×10×10 = 1000-probe grid (worst case); 16 such lights × 27 × 2 bytes × 1000
≈ 864 KB. A wide ambient fill at 20m radius: 40×40×40 = 64 000 probes × 27 × 2 bytes ≈ 3.5 MB
per light. In practice, most animated lights are small-to-medium panel lights; a scene with
16 small panel lights sits around 800 KB–1 MB delta total. The probe spacing lives in
`prl-build` as `--delta-spacing` (default 1.0m).

**Albedo fallback for Task A.** Use a fixed 0.45 constant per channel (`BOUNCE_ALBEDO`).
Tuned to avoid over-brightening the indirect contribution relative to the direct lightmap.

---

## Open Questions

*(All previously open questions have been resolved.)*

**Q1 — Ambient floor:** Closed. Configurable ambient floor is out of scope; moved to Out of
Scope above. The resulting look after the indirect-only rebake will be evaluated visually;
no pre-emptive floor adjustment is planned.

**Q2 — Delta grid resolution:** Closed. Per-light AABB baking at fixed 1.0m spacing
(bake-time configurable via `--delta-spacing`). See Task B, Task C, and the Memory sketch
above.

**Q3 — Animated lights in AmbientOnly mode:** Closed. AmbientOnly (mode 3) stays minimal —
no composed animated SH. See Task F isolation table.

**Q4 — Surface albedo for indirect bake (Task A):** Closed. `prl-build` does NOT have
access to per-face texture color at bake time. Texture images are never loaded during the
bake. Adding texture sampling to `prl-build` is out of scope. **Decision: use
`BOUNCE_ALBEDO = 0.45` — update the existing constant in `sh_bake.rs`.**
