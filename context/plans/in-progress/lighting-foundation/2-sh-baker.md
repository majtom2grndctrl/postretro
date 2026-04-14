# Sub-plan 2 — SH Irradiance Volume Baker

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals and the BVH dependency.
> **Scope:** SH irradiance volume baker stage in `prl-build`, plus the SH PRL section. Ray-casts through the Milestone 4 BVH via the `bvh` crate. No engine-side rendering work in this sub-plan — that's sub-plan 6.
> **Crates touched:** `postretro-level-compiler`, `postretro-level-format`.
> **Depends on:** sub-plan 1 (`MapData.lights` populated) **and** Milestone 4's BVH (the `bvh` crate primitive set built in `bvh-foundation/1-compile-bvh.md`).
> **Blocks:** sub-plan 6 and sub-plan 7 (runtime needs the SH PRL section to load).

---

## Description

Add a baker stage to `prl-build` that places SH L2 probes on a regular 3D grid over the level's empty space, evaluates incoming radiance at each probe by ray-casting through the **Milestone 4 BVH**, projects the radiance into SH coefficients, flags probes inside solid geometry, and writes the result to a new SH PRL section.

Postretro bakes indirect illumination only. Direct illumination is *not* baked — it is evaluated at runtime by the flat per-fragment light loop (sub-plan 3). This split lets dynamic lights co-exist with baked indirect without lightmap complexity, and keeps probe data read-only at runtime.

**Acceleration structure: the Milestone 4 BVH.** No separate baker BVH. The baker imports `postretro-level-compiler`'s BVH primitive set (the same one that gets flattened to the `Bvh` PRL section in `bvh-foundation/1-compile-bvh.md`) and calls `bvh::Bvh::traverse` directly on the CPU. Same tree, different traversal implementation.

---

## Spatial layout: regular 3D grid

Probes sit on an axis-aligned grid that spans the level's AABB with a configurable cell size (default: 1 meter). The grid is the full coverage — no sparse octree, no per-leaf alignment. Rationale:

- Trivial to index: `(x, y, z)` grid coordinate maps directly to a 3D texture texel.
- Hardware trilinear filtering in the fragment shader, zero work in shader code.
- Target maps are small and indoor — octree adaptivity saves little.
- Probes inside solid geometry are flagged invalid; invalid probes are never sampled (see validity mask below).

Compiler flag `--probe-spacing <meters>` overrides the default. Tighter spacing near floors is handled by a second vertical-tier override in future work, not in the initial cut.

---

## Per-probe storage: SH L2

Nine SH basis coefficients per color channel × three channels = **27 f32 per probe**. SH L2 captures directional incoming radiance with enough fidelity for smooth indirect shading, and the reconstruction math is a single dot product per channel in the fragment shader.

Rejected alternatives:
- **Plain RGB** — loses directional information; flat indirect looks wrong on curved or angled surfaces.
- **Ambient cube** — 18 f32 per probe for comparable quality; SH L2 wins on smoothness.
- **SH L1** — 12 f32 per probe; cheaper but noticeably blurrier on test scenes with colored directional indirect.

---

## Validity mask

Each probe has a `u8` validity flag: `0` = invalid (inside solid), `1` = valid (usable). Validity is determined at bake time by sampling the BSP tree at the probe position — solid leaves produce invalid probes. Runtime sampling uses the mask to fall back to nearby valid probes when the trilinear footprint crosses a wall.

**Leak mitigation.** A mean-distance-to-nearest-surface field per probe direction (as used in DDGI) is a follow-up if simple validity masking proves insufficient on the test maps. The initial cut ships validity-only.

---

## Bake algorithm

For each valid probe:

1. Fire **N stratified sample rays** from the probe (default `N = 256`) distributed over the sphere.
2. For each ray, traverse the **Milestone 4 BVH** (via the `bvh` crate on the CPU) to find the closest triangle hit. Miss → sky/ambient. Hit → evaluate direct light at the hit point (shadow raycasts from each map light traversing the same BVH, sum Lambert contributions), then attenuate by surface albedo approximation to approximate one bounce.
3. Project the incoming radiance samples into SH L2 coefficients.
4. Store coefficients in the probe grid; write validity flag.

Ray count and parallelism strategy are execution details — the spec fixes the algorithm shape, not the sizing. Parallelism: `rayon` over probes. Acceleration: no separate baker BVH — the Milestone 4 BVH primitive set is the acceleration structure. Same tree, same crate, same traversal code that the runtime cull lowers to GPU; the baker just calls it from the CPU side instead of the WGSL compute shader.

### Animation baking

Lights with `LightAnimation` bake into separate monochrome SH layers — one layer per animated light — so the runtime can modulate each light's indirect contribution independently without re-baking.

**Decomposition.** The baker splits map lights into two sets:

- **Static lights** (no animation): their combined contribution bakes into the base SH coefficients (27 f32 per probe, 3-channel). This is the same data the static-only path produces.
- **Animated lights** (have `LightAnimation`): each animated light bakes its own contribution at unit intensity into a **monochrome SH layer** (9 f32 per probe — luminance only, no color). The light's base color and animation curve are stored once per light in the animation descriptor table, not per probe.

**Why monochrome.** Animation modulates intensity and/or color but never changes the light's position or direction. The directional distribution of a light's contribution to a probe is constant across the cycle — only the magnitude and color change. Storing 9 monochrome SH coefficients per probe instead of 27 RGB ones cuts per-light storage by 3×.

**Runtime reconstruction.** For each animated light, the fragment shader evaluates the animation curve at the current time (`t = fract(time / period + phase)`, linearly interpolate between adjacent brightness samples), multiplies monochrome SH by `base_color × brightness(t)` (or `color(t) × brightness(t)` if the light has color animation), and adds to the base SH before irradiance reconstruction. See sub-plan 6.

**Memory.** Per-light layers: `probes × 9 × 4 bytes` each. A 60 × 60 × 20 grid (72k probes) with 5 animated lights: base = 7.7 MB + 5 layers × 2.6 MB = 20.7 MB total. Proportionally less for smaller maps. Animation descriptors (curves, periods, colors) are negligible — stored once per light, not per probe.

**Bake procedure for animated lights.** Identical to the static path except: (a) each animated light bakes in isolation at unit intensity with white color, (b) SH projection collapses to monochrome (average the 3-channel result, or project luminance directly), (c) output writes to the per-light layer in the PRL section instead of the base probe record.

**All animated lights bake.** There is no "defer animation" path. If a map light has `animation: Some(...)`, it bakes into a separate layer. If no lights have animation, no layers are emitted and the section layout matches the static-only format.

### Shadow strategy

Bake-time raycast occlusion. Each map light contribution at a probe is modulated by a shadow ray from the light position (or direction, for `Directional`) to the probe. Visible → full contribution; occluded → zero. This is the full cost during the bake, but the bake happens once per compile.

Runtime dynamic lights rely on shadow maps (sub-plan 3), not probe data.

---

## PRL section layout

New `ShVolume` PRL section (section ID 20) in `postretro-level-format/src/lib.rs`, allocated after Milestone 4's `Bvh` (ID 19).

All little-endian. Header, then base probe records, then animation descriptor table, then per-light SH layers.

```
Header (44 bytes):
  f32 × 3    grid_origin          (world-space min corner, meters)
  f32 × 3    cell_size            (meters per cell along x/y/z)
  u32 × 3    grid_dimensions      (probe count along x/y/z)
  u32        probe_stride         (bytes per base probe record; always 112)
  u32        animated_light_count (0 = no animated lights; determines layer count)

Base probe records (probe_stride bytes each, iterated z-major then y, then x):
  f32 × 27   sh_coefficients      (9 bands × 3 channels, RGB)
  u8         validity             (0 = invalid, 1 = valid)
  u8 × 3     padding              (align to 4 bytes)

--- only present when animated_light_count > 0 ---

Animation descriptor table (one entry per animated light):
  f32        period               (cycle duration in seconds)
  f32        phase                (0-1 offset within cycle)
  f32 × 3    base_color           (linear RGB, 0-1)
  u32        brightness_count     (number of brightness samples; 0 = no brightness animation)
  u32        color_count          (number of color samples; 0 = no color animation)
  f32 × brightness_count          brightness samples (uniformly spaced over period)
  [f32; 3] × color_count          color samples (linear RGB, uniformly spaced over period)

Per-light SH layers (one layer per animated light, same probe iteration order):
  f32 × 9    sh_coefficients_mono (9 SH bands, monochrome / luminance)
  per probe, total_probes entries per layer
```

Base probe record: `27 × 4 + 4 = 112 bytes`. Per-light layer record: `9 × 4 = 36 bytes` per probe. Animation descriptors are variable-length due to sample arrays.

### Compatibility

Missing section is not an error. The world shader degrades to flat white ambient when the section is absent, matching pre-Milestone-5 behavior. A section with `animated_light_count = 0` is valid — the runtime loads base probes only and skips animated layer processing.

---

## Acceptance criteria

- [ ] `SectionId::ShVolume = 20` allocated in `postretro-level-format/src/lib.rs`
- [ ] Probe record and section types added to `postretro-level-format` with read/write + round-trip tests matching the existing section pattern
- [ ] Baker stage in `prl-build` runs after Milestone 4's BVH construction and before pack
- [ ] Ray traversal goes through the Milestone 4 BVH via the `bvh` crate — no separate baker BVH
- [ ] Probe placement: regular grid over map AABB at configurable spacing; solidity query against BSP populates the validity mask
- [ ] Stratified sphere sampling produces SH L2 coefficients per probe
- [ ] Shadow raycasts traverse the same BVH; map lights modulated by visibility
- [ ] Animated lights bake into separate monochrome SH layers (9 f32 per probe per animated light)
- [ ] Static and animated light contributions are correctly decomposed: base SH excludes animated lights; per-light layers capture each animated light at unit intensity
- [ ] Animation descriptor table written to PRL with correct period, phase, base_color, and sample arrays from `LightAnimation`
- [ ] `animated_light_count` header field is 0 when no lights have animation; section degrades to static-only layout
- [ ] Determinism: identical input `.map` produces identical SH coefficients (stratified sampling uses a fixed seed)
- [ ] `--probe-spacing <meters>` CLI flag implemented with default of 1.0
- [ ] Bake parallelism via `rayon` — one task per probe or per probe slab
- [ ] Every test map in `assets/maps/` compiles and emits an SH section
- [ ] Missing SH section degrades cleanly (verified by removing the section from a test PRL and loading it; no error)
- [ ] `cargo test -p postretro-level-compiler -p postretro-level-format` passes
- [ ] `cargo clippy -p postretro-level-compiler -p postretro-level-format -- -D warnings` clean

---

## Implementation tasks

1. Allocate `SectionId::ShVolume = 20` in `postretro-level-format/src/lib.rs` (next after Milestone 4's `SectionId::Bvh = 19`).

2. Add probe record and section types to `postretro-level-format` with read/write + round-trip tests.

3. Implement probe placement in the baker: regular grid over map AABB at configurable spacing; solidity query against BSP populates the validity mask.

4. Implement radiance sampling: stratified sphere rays, traverse the **Milestone 4 BVH via the `bvh` crate** for closest-triangle hits, per-light shadow raycasts through the same BVH, Lambert evaluation at hit points. No separate baker BVH.

5. Implement SH L2 projection from radiance samples.

6. Parallelize with `rayon` over probes; expose `--probe-spacing` CLI flag.

7. Wire the SH volume section into the `prl-build` pack stage.

8. Implement animated light decomposition: separate map lights into static and animated sets, bake animated lights into monochrome SH layers at unit intensity, write animation descriptor table and per-light layers to the PRL section.

---

## Notes for implementation

- **Sharing the BVH primitive set across stages.** The Milestone 4 BVH primitives live in the compiler. The baker is a sibling module that imports them directly — no PRL round-trip at compile time. This matters: serializing and re-parsing the BVH between stages would be wasted work and risks introducing subtle byte-layout drift.
- **Stratified sphere sampling.** Use the standard cosine-weighted hemisphere sampling for hit-point evaluations (Lambert is the surface BRDF), and uniform sphere sampling for the probe ray distribution itself. Fixed RNG seed for determinism.
- **Albedo approximation for one bounce.** Without a full material system, treat each surface as Lambertian with a constant albedo (e.g., 0.5 grey) for the bounce. This is intentional — Postretro's aesthetic is not photorealistic GI; the SH volume just needs to carry directional indirect color and intensity. A future revision may sample albedo from the texture, but it's not in the initial cut.
- **Sky / miss handling.** Rays that miss all geometry contribute a constant ambient color (configurable, default near-black). No HDR sky cubemap in the initial cut — that's a follow-up if outdoor maps need it.
- **Probe stride forward-compat.** The header's `probe_stride` field allows future revisions to add per-probe base data (DDGI distance fields, etc.) without breaking the loader. Animated light data lives in separate per-light layers after the base probe array, not in the base probe record — `probe_stride` stays 112 regardless of animation.
