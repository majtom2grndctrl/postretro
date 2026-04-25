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
  small per-light delta SH volume representing that light's contribution to the probe grid
  at peak brightness (brightness = 1.0, base color). New PRL section. Delta volumes use the
  same world extent and probe layout as the base SH volume but may be baked at coarser
  spatial resolution (1/4 per axis minimum); the runtime compose pass interpolates.

- **Delta SH volumes — PRL section.** New PRL section (`DeltaShVolumes`) that stores
  header metadata (delta grid dimensions, world extent, animated light count, per-light
  `AnimationDescriptorIndex`) and per-light probe data in f16 format.

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

Albedo estimation: use the texture's stored average color (from the level material table)
or a fixed constant (0.7 per channel) as a fallback. The exact value affects brightness but
not correctness of the separation.

This task changes the baked output. Existing `.prl` files must be recompiled after this
lands. The change is isolated to the bake side; the runtime consumes the same section
format.

### Task B — PRL section: DeltaShVolumes

**Crate:** `postretro-level-format` · **New file:** `src/delta_sh_volumes.rs`

Define a new PRL section type (assign next available `SectionId` integer) for delta SH
volumes. Section layout:

- Header: version, probe grid origin + cell size + dimensions for the delta grid, world
  extent (must match or be a sub-grid of the base SH extent), animated light count,
  `AnimationDescriptorIndex` table (one u32 per animated light → index into the SH section's
  descriptor array).
- Per-light probe data: the same 9-band f16 RGB layout as the base SH section but at the
  delta grid resolution. Each probe stores 27 × f16 values (9 bands × RGB).
- Probe order matches the base SH section (Z-major then Y then X).

The delta grid resolution is configurable at bake time (default: 1/4 per axis vs. the base
grid, minimum 1 probe per axis). The runtime performs trilinear interpolation to upsample
delta contributions to base-grid resolution during the compose pass.

### Task C — Delta SH volume baking

**Crate:** `postretro-level-compiler` · **New file:** `src/delta_sh_bake.rs`

For each animated light: run the same ray-tracing loop used by the base SH bake, but with
only that one light active in the evaluator and brightness fixed at 1.0 / base_color = 1.0
(peak contribution). Store the result as a delta SH probe grid at the configured coarse
resolution. The bake is embarrassingly parallel across lights (each is independent).

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
3. Writes the total SH bands into the existing `sh_band0..8` 3D textures (or into a
   separate `sh_total_band0..8` set if wgpu requires non-overlapping read/write bindings —
   both sets would be created at load time; consumers rebind to the total set).

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

Rename the existing single `use_direct` boolean into two independent booleans:
`use_lightmap` and `use_specular`. Update the lighting isolation switch:

| Mode | `use_lightmap` | `use_specular` | `use_indirect` |
|------|---------------|----------------|----------------|
| Normal (0) | true | true | true |
| DirectOnly (1) | true | true | false |
| IndirectOnly (2) | false | true | false |
| AmbientOnly (3) | false | false | false |

Specular was previously silenced in IndirectOnly mode because it shared the `use_direct`
flag with the lightmap sample. IndirectOnly was the best-looking mode and should have
working specmaps. AmbientOnly is a minimal debug view; specular off there is intentional.

Also update `src/render/mod.rs` where the isolation uniform value is packed.

This task is independent of all others.

---

## Sequencing

**Phase 1 (concurrent):** Task A (indirect-only bake), Task B (PRL section format),
Task E (split use_direct) — all independent.

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

**Memory.** At 1/4 resolution (8×4×2 = 64 probes), 16 animated lights × 27 × 2 bytes × 64
probes ≈ 55 KB on GPU. At same resolution as base (4096 probes): 16 × 27 × 2 × 4096 ≈ 3.5 MB.
Default 1/4 resolution is recommended; the constant lives in `prl-build` as a configurable flag.

**Albedo fallback for Task A.** If no material albedo is accessible at bake time for a hit
surface, use a neutral 0.7 constant per channel. This is a conservative estimate for
typical painted surfaces and matches common radiosity defaults.

---

## Open Questions

1. **Base SH re-bake delta.** After the indirect-only change (Task A), will existing maps
   look significantly different at base ambience levels? The SH contribution may drop (it
   was previously over-bright due to double-counting). Should `ambient_floor` be adjusted
   upward to compensate, or is the resulting look acceptable as-is after the fix?

2. **Delta grid resolution.** Is 1/4 per axis (8×4×2 probe default) sufficient spatial
   precision for animated panel lights? A flickering ceiling panel over a doorway should
   visibly affect a character passing through. If the 1/4 grid smears the contribution
   over too large an area, a 1/2 resolution delta (~8× the memory) may be warranted.
   A visual prototype of the compose result at both resolutions should inform this before
   Task C starts.

3. **Animated lights in AmbientOnly mode.** Mode 3 today shows a flat ambient floor only.
   After this plan, should mode 3 also include the composed animated SH ambient? Current
   proposal: no — AmbientOnly is a debug baseline and should stay minimal.

4. **Surface albedo for indirect bake (Task A).** The baker needs a per-surface albedo to
   weight the reflected contribution. Does `prl-build` have access to average texture color
   at bake time, or is the 0.7 constant fallback the practical path for now?
