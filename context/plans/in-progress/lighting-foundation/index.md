# Lighting Foundation

> **Status:** ready — all sub-plans complete, durable decisions captured in `context/lib/`.
> **Milestone:** 5 (Lighting Foundation) — see `context/plans/roadmap.md`.
> **Related:** `context/lib/rendering_pipeline.md` §4 · `context/lib/build_pipeline.md` §Custom FGD · `context/lib/entity_model.md` · `context/reference/light-entities-across-engines.md`
> **Prerequisite:** Milestone 4 (BVH Foundation) — must ship and pass its check-in gate before any work in this plan begins. The SH baker traverses the BVH built in Milestone 4. See `context/plans/done/bvh-foundation/`.
> **Lifecycle:** Per `context/lib/development_guide.md` §1.5, this plan *is* the lighting spec while it's a draft. Decisions land in the spec as they're made; durable knowledge migrates to `context/lib/` when the plan ships.

---

## Goal

Replace flat ambient with the full target lighting pipeline:

- **Indirect:** SH L2 irradiance volume baked at compile time, sampled per fragment via 3D texture trilinear interpolation.
- **Direct:** dynamic lights (point, spot, directional) authored as FGD entities and consumed both by the bake and by the runtime direct path. Target: up to **500 authored lights per level**. The runtime uses a flat per-fragment loop with per-light influence-volume early-out (sub-plan 4); clustered forward+ binning is a future optimization if profiling shows the flat loop bottlenecks at high visible-light counts.
- **Surface detail:** tangent-space normal maps perturbing the per-fragment normal before shading.
- **Dynamic shadows:** cascaded shadow maps for directional lights, cube shadow maps for point lights, single 2D shadow maps for spot lights. Low-resolution, nearest-neighbor sampling — chunky pixel shadow edges match the target aesthetic.

The translator is decoupled from the parser: future map-format support (e.g., UDMF) adds a sibling module against the same canonical types, so the baker and everything downstream never learn the source format.

**Shadow coverage:** the SH irradiance volume captures indirect light bounces at bake time; dynamic shadow maps cover direct-light occlusion at runtime. Together these replace what lightmaps would contribute in a traditional Quake-lineage pipeline.

---

## How this plan rides on Milestone 4

The SH baker ray-casts probe samples through the **same BVH** that Milestone 4 builds for runtime cull. One acceleration structure, two consumers:

- Milestone 4 runtime: WGSL compute shader walks the BVH for frustum cull each frame.
- Milestone 5 baker: CPU `bvh` crate walks the same flattened tree to ray-cast probe radiance samples and shadow rays.

This means **no second design pass for spatial structures.** The baker calls into the `bvh` crate directly using the same primitive set the compiler built in `context/plans/done/bvh-foundation/1-compile-bvh.md`. No separate baker BVH, no embree, no hardware RT.

---

## Pipeline

```
TrenchBroom authoring (FGD)
  → .map file
    → prl-build parser (extract property bag)
      → format::quake_map::translate_light (validate, convert)
        → MapLight in MapData.lights
          ├─→ SH irradiance volume baker
          │     (ray-casts through the Milestone 4 BVH via bvh crate,
          │      SH L2 projection, validity mask)
          │     → SH section in .prl
          │       → runtime trilinear sample (fragment shader,
          │          indirect term)
          └─→ runtime direct light buffer
              (map lights; transient gameplay lights in Milestone 6+)
                → flat per-fragment light loop (direct term)
                  → shadow map sampling per shadow-casting light
```

---

## Sub-plans

This plan has eight sub-files. Sub-plans 1–2 are compiler/data work; sub-plans 3–8 are engine-side, ordered so each step has a clear visual validation surface before the next layer is added. Sub-plan 1 has no engine impact and could overlap with the tail end of Milestone 4 in principle, but the rule is: nothing from this plan starts until Milestone 4's check-in gate signs off.

**Dependency graph summary:**
- Sub-plan 1 must complete before anything else.
- Sub-plans 2 (compiler-side SH baker) and 3 (engine-side direct lighting) can proceed **in parallel** once sub-plan 1 is done — they are independent work streams.
- Sub-plan 4 (light influence volumes) depends on sub-plan 3. Sub-plan 5 (shadows) depends on sub-plan 3 and **benefits from sub-plan 4** (the CPU frustum-visibility test sub-plan 4 provides enables shadow-slot allocation; sub-plan 5 can ship without it by allocating a fixed slot per light, but the intended design expects sub-plan 4 to land first). Sub-plan 6 (normal maps) depends on sub-plan 3 only. Sub-plans 4, 5, and 6 are otherwise independent of each other.
- Sub-plan 7 (SH volume runtime) depends on sub-plans 2 and 3, but is independent of sub-plans 5 and 6.
- Sub-plan 8 (animated SH) depends on sub-plan 7.

### Compiler / data pipeline

1. **[1-fgd-canonical.md](./1-fgd-canonical.md)** — FGD light entities, parser wiring, translator, map light format. Pure compiler/data work, no engine changes. Output: `MapData.lights: Vec<MapLight>` populated for every test map. **Gate:** sub-plans 2 and 3 may not start until this is done.

2. **[2-sh-baker.md](./2-sh-baker.md)** — SH irradiance volume baker stage in `prl-build`, plus the SH PRL section. Ray-casts through the Milestone 4 BVH. Output: every test map emits an SH section. **Depends on:** sub-plan 1. **Parallel with:** sub-plan 3.

### Engine runtime (ordered by visual validation dependencies)

3. **[3-direct-lighting.md](./3-direct-lighting.md)** — Direct lighting via a flat per-fragment light loop + ambient floor. Uploads map lights to a GPU storage buffer, evaluates Lambert diffuse with per-type falloff and spot cone attenuation. This is the foundation — sub-plans 4, 5, and 6 are all validated relative to what this shows. Uses a flat loop, not clustered forward+; clustering is a future optimization when light counts demand it. **Depends on:** sub-plan 1. **Parallel with:** sub-plan 2.

4. **[4-light-influence-volumes.md](./4-light-influence-volumes.md)** — Light influence volumes (compile-time per-light sphere bounds in PRL, runtime spatial culling). **Depends on:** sub-plan 3. **Enables:** sub-plan 5's shadow-slot allocation (the CPU frustum-visibility test produced here gates which lights need an active shadow map this frame).

5. **[5-shadow-maps.md](./5-shadow-maps.md)** — Shadow map passes modulating the direct term. CSM for directional, cube shadow maps for point, single 2D for spot. Nearest-neighbor sampling for chunky retro shadow edges. **Depends on:** sub-plan 3. **Benefits from:** sub-plan 4 (CPU frustum-visibility test for slot allocation). **Independent of:** sub-plan 6.

6. **[6-normal-maps.md](./6-normal-maps.md)** — Tangent-space normal maps. Activates the TBN data already in the vertex format (Milestone 3.5). Perturbs the per-fragment shading normal before both direct and indirect evaluation. **Depends on:** sub-plan 3. **Independent of:** sub-plans 4 and 5.

7. **[7-sh-volume.md](./7-sh-volume.md)** — SH irradiance volume sampling (indirect lighting). Loads the SH PRL section from sub-plan 2, uploads to 3D textures, trilinear samples in the fragment shader, reconstructs SH L2 irradiance. Replaces flat ambient as the indirect term. **Depends on:** sub-plans 2 and 3. **Independent of:** sub-plans 5 and 6.

8. **[8-animated-sh.md](./8-animated-sh.md)** — Animated SH layers. Loads per-light monochrome SH layers and animation descriptors, evaluates brightness/color curves per frame, modulates and adds to base SH. Final sub-plan. **Depends on:** sub-plan 7.

---

## Scope

### In scope

- FGD entities `light`, `light_spot`, `light_sun` in `assets/postretro.fgd`
- `postretro-level-compiler/src/format/quake_map.rs` translator module. Pattern is `format/<name>.rs` per source format; each format's internal structure is its own decision.
- Parser wiring: `prl-build` extracts light entity properties into a property bag and dispatches to the translator.
- Map light format: `LightType`, `FalloffModel`, `LightAnimation`, `MapLight`. Format-agnostic.
- Validation rules (errors block compilation; warnings log and proceed).
- Quake `style` integer → `LightAnimation` preset conversion. Canonical format is preset-free; the translator owns the Quake style table.
- SH irradiance volume baker: probe placement, radiance evaluation with shadow raycasting through the Milestone 4 BVH, SH L2 projection, validity masking, PRL section writer.
- Runtime SH volume sampling: parse PRL section to 3D texture, trilinear sampling in world shader.
- Normal map loading and tangent-space shading in the world shader.
- Flat per-fragment direct light loop with influence-volume early-out: per-light-type evaluation (point/spot/directional), falloff models, spot cone attenuation. Target: 500 authored lights per level; influence volumes ensure the per-fragment cost scales with nearby lights, not total lights. Clustered forward+ binning deferred until profiling shows the flat loop bottlenecks.
- Shadow map pipeline: CSM for directional, cube shadow maps for point/spot, sampling in the world shader.
- Test map coverage extending `assets/maps/test.map`.
- Documentation update to `context/lib/build_pipeline.md` §Custom FGD table.

### Out of scope

- UDMF map-format support. Separate initiative. The `format/<name>.rs` architecture established here accommodates it without refactor.
- `env_projector` and texture-projecting lights.
- IES profiles / photometric data.
- Area lights (rectangle, disk). Point / spot / directional cover the target feature set.
- Second-bounce indirect. The SH volume captures direct-to-static bounces; multi-bounce is a follow-up if visuals demand it.
- Runtime dynamic probe updates (DDGI-style). The SH volume is baked, read-only at runtime.
- Runtime evaluation of light animation curves. The baker bakes animation into probe sample curves at compile time; runtime evaluation of dynamic light animations is a Milestone 6+ follow-up.
- Hardware ray tracing. Pre-RTX target locked in Milestone 4.
- Exhaustive academic literature review.
- Benchmarking probe baking performance. Decisions are made on design grounds; benchmarking belongs in execution if sizing questions surface.

---

## When this plan ships

Durable architectural decisions migrate to `context/lib/rendering_pipeline.md` (`context/lib/lighting.md` if the section outgrows §4):

- `MapLight` struct shape and design rationale.
- `format/<name>.rs` architecture for multi-format source support.
- SH volume spatial layout, per-probe storage, validity masking.
- SH volume PRL section shape.
- Flat light loop design and migration path to clustered forward+ when needed.
- Shadow map defaults (resolution, cascade count, depth bias).
- Baker ↔ BVH sharing pattern: one acceleration structure, two consumers (bake-time CPU, runtime GPU).

The plan document itself is ephemeral per `development_guide.md` §1.5.
