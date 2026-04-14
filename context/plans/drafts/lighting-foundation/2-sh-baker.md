# Sub-plan 2 — SH Irradiance Volume Baker

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals and the BVH dependency.
> **Scope:** SH irradiance volume baker stage in `prl-build`, plus the SH PRL section. Ray-casts through the Milestone 4 BVH via the `bvh` crate. No engine-side rendering work in this sub-plan — that's sub-plan 3.
> **Crates touched:** `postretro-level-compiler`, `postretro-level-format`.
> **Depends on:** sub-plan 1 (`MapData.lights` populated) **and** Milestone 4's BVH (the `bvh` crate primitive set built in `bvh-foundation/1-compile-bvh.md`).
> **Blocks:** sub-plan 3 (runtime needs the SH PRL section to load).

---

## Description

Add a baker stage to `prl-build` that places SH L2 probes on a regular 3D grid over the level's empty space, evaluates incoming radiance at each probe by ray-casting through the **Milestone 4 BVH**, projects the radiance into SH coefficients, flags probes inside solid geometry, and writes the result to a new SH PRL section.

Postretro bakes indirect illumination only. Direct illumination is *not* baked — it is evaluated at runtime by the clustered forward+ path (sub-plan 3). This split lets dynamic lights co-exist with baked indirect without lightmap complexity, and keeps probe data read-only at runtime.

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
2. For each ray, traverse the **Milestone 4 BVH** (via the `bvh` crate on the CPU) to find the closest triangle hit. Miss → sky/ambient. Hit → evaluate direct light at the hit point (shadow raycasts from each canonical light traversing the same BVH, sum Lambert contributions), then attenuate by surface albedo approximation to approximate one bounce.
3. Project the incoming radiance samples into SH L2 coefficients.
4. Store coefficients in the probe grid; write validity flag.

Ray count and parallelism strategy are execution details — the spec fixes the algorithm shape, not the sizing. Parallelism: `rayon` over probes. Acceleration: no separate baker BVH — the Milestone 4 BVH primitive set is the acceleration structure. Same tree, same crate, same traversal code that the runtime cull lowers to GPU; the baker just calls it from the CPU side instead of the WGSL compute shader.

### Animation baking

Lights with animation curves bake into a sample vector per probe: `period` seconds discretized into `sample_count` entries (default 11 samples/cycle). At runtime, the shader reads the current sample and blends with the next. Memory overhead: `probes × animated_lights × samples × 4 bytes`. A 60 × 60 × 20 grid (72k probes) with 5 animated lights and 11 samples is ~16 MB — acceptable upper bound for a large level. Small levels pay proportionally less.

The initial cut may defer animation baking — a static-only first revision that ignores `LightAnimation` is acceptable if it simplifies the first end-to-end path. Execution decides.

### Shadow strategy

Bake-time raycast occlusion. Each canonical light contribution at a probe is modulated by a shadow ray from the light position (or direction, for `Directional`) to the probe. Visible → full contribution; occluded → zero. This is the full cost during the bake, but the bake happens once per compile.

Runtime dynamic lights rely on shadow maps (sub-plan 3), not probe data.

---

## PRL section layout

New PRL section for the SH irradiance volume. Section ID to be allocated in `postretro-level-format/src/lib.rs` alongside existing section IDs (separate from Milestone 4's `Bvh` section id).

All little-endian. Header, then packed probe records.

```
Header (32 bytes):
  f32 × 3    grid_origin      (world-space min corner, meters)
  f32 × 3    cell_size        (meters per cell along x/y/z)
  u32 × 3    grid_dimensions  (probe count along x/y/z)
  u32        probe_stride     (bytes per probe record; 112 for static-only, more with animation)

Probe records (probe_stride bytes each, iterated z-major then y, then x):
  f32 × 27   sh_coefficients  (9 bands × 3 channels)
  u8         validity         (0 = invalid, 1 = valid)
  u8 × 3     padding          (align to 4 bytes)
```

Total static-only probe record: `27 × 4 + 4 = 112 bytes`.

### Compatibility

Missing section is not an error. The world shader degrades to flat white ambient when the section is absent, matching pre-Milestone-5 behavior.

---

## Acceptance criteria

- [ ] New PRL section ID allocated in `postretro-level-format/src/lib.rs` for the SH irradiance volume (separate from Milestone 4's `Bvh` section id)
- [ ] Probe record and section types added to `postretro-level-format` with read/write + round-trip tests matching the existing section pattern
- [ ] Baker stage in `prl-build` runs after Milestone 4's BVH construction and before pack
- [ ] Ray traversal goes through the Milestone 4 BVH via the `bvh` crate — no separate baker BVH
- [ ] Probe placement: regular grid over map AABB at configurable spacing; solidity query against BSP populates the validity mask
- [ ] Stratified sphere sampling produces SH L2 coefficients per probe
- [ ] Shadow raycasts traverse the same BVH; canonical lights modulated by visibility
- [ ] Determinism: identical input `.map` produces identical SH coefficients (stratified sampling uses a fixed seed)
- [ ] `--probe-spacing <meters>` CLI flag implemented with default of 1.0
- [ ] Bake parallelism via `rayon` — one task per probe or per probe slab
- [ ] Every test map in `assets/maps/` compiles and emits an SH section
- [ ] Missing SH section degrades cleanly (verified by removing the section from a test PRL and loading it; no error)
- [ ] `cargo test -p postretro-level-compiler -p postretro-level-format` passes
- [ ] `cargo clippy -p postretro-level-compiler -p postretro-level-format -- -D warnings` clean

---

## Implementation tasks

1. Allocate a new PRL section ID for the SH irradiance volume in `postretro-level-format/src/lib.rs` (separate from the Milestone 4 `Bvh` section id).

2. Add probe record and section types to `postretro-level-format` with read/write + round-trip tests.

3. Implement probe placement in the baker: regular grid over map AABB at configurable spacing; solidity query against BSP populates the validity mask.

4. Implement radiance sampling: stratified sphere rays, traverse the **Milestone 4 BVH via the `bvh` crate** for closest-triangle hits, per-light shadow raycasts through the same BVH, Lambert evaluation at hit points. No separate baker BVH.

5. Implement SH L2 projection from radiance samples.

6. Parallelize with `rayon` over probes; expose `--probe-spacing` CLI flag.

7. Wire the SH volume section into the `prl-build` pack stage.

---

## Notes for implementation

- **Sharing the BVH primitive set across stages.** The Milestone 4 BVH primitives live in the compiler. The baker is a sibling module that imports them directly — no PRL round-trip at compile time. This matters: serializing and re-parsing the BVH between stages would be wasted work and risks introducing subtle byte-layout drift.
- **Stratified sphere sampling.** Use the standard cosine-weighted hemisphere sampling for hit-point evaluations (Lambert is the surface BRDF), and uniform sphere sampling for the probe ray distribution itself. Fixed RNG seed for determinism.
- **Albedo approximation for one bounce.** Without a full material system, treat each surface as Lambertian with a constant albedo (e.g., 0.5 grey) for the bounce. This is intentional — Postretro's aesthetic is not photorealistic GI; the SH volume just needs to carry directional indirect color and intensity. A future revision may sample albedo from the texture, but it's not in the initial cut.
- **Sky / miss handling.** Rays that miss all geometry contribute a constant ambient color (configurable, default near-black). No HDR sky cubemap in the initial cut — that's a follow-up if outdoor maps need it.
- **Probe stride forward-compat.** The header's `probe_stride` field allows future revisions to add per-probe data (DDGI distance fields, animation samples, etc.) without breaking the loader. Initial cut ships at 112 bytes.
