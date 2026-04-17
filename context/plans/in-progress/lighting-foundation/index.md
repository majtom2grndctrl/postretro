# Lighting Foundation

> **Status:** in progress — sub-plans 1–5 complete; sub-plan 6 (SH volume runtime) is next. Durable decisions migrate to `context/lib/` when the full plan ships.
> **Milestone:** 5 (Lighting Foundation) — see `context/plans/roadmap.md`.
> **Related:** `context/lib/rendering_pipeline.md` §4 · `context/lib/build_pipeline.md` §Custom FGD · `context/lib/entity_model.md` · `context/reference/light-entities-across-engines.md`
> **Prerequisite:** Milestone 4 (BVH Foundation) — must ship and pass its check-in gate before any work in this plan begins. The SH baker traverses the BVH built in Milestone 4. See `context/plans/done/bvh-foundation/`.
> **Lifecycle:** Per `context/lib/development_guide.md` §1.5, this plan *is* the lighting spec while it's a draft. Decisions land in the spec as they're made; durable knowledge migrates to `context/lib/` when the plan ships.

---

## Goal

Replace flat ambient with the full target lighting pipeline:

- **Indirect:** SH L2 irradiance volume baked at compile time, sampled per fragment via 3D texture trilinear interpolation. Indirect bleed lights both static surfaces and dynamic entities via probe sampling.
- **Direct:** lights (point, spot, directional) authored as FGD entities. Each light is either **runtime-dynamic** (participates in the runtime direct loop AND contributes to the probe bake) or **bake-only** (contributes to the probe bake only, has no runtime presence). The distinction is a single `_bake_only` FGD property, not separate entity classnames. Target: up to **500 authored lights per level**. The runtime uses a flat per-fragment loop with per-light influence-volume early-out (sub-plan 4); clustered forward+ binning is a future optimization if profiling shows the flat loop bottlenecks at high visible-light counts.
- **Surface detail:** per-texel specular response from specular maps, evaluated in the direct light loop (sub-plan 9). Shading model TBD (Phong vs. PBR).
- **Shadows:** two complementary paths. **CSM (sub-plan 5)** provides hard-edge cascaded shadows for directional lights (the sun), matching the chunky aesthetic. **SDF sphere-tracing (sub-plan 8)** provides soft-edge shadows for point and spot lights by sphere-tracing a baked signed distance field of the world. Uniform quality across all omnidirectional lights, one fragment-shader pass for all lights, no cube shadow maps.

The translator is decoupled from the parser: future map-format support (e.g., UDMF) adds a sibling module against the same canonical types, so the baker and everything downstream never learn the source format.

**Shadow coverage:** the SH irradiance volume captures indirect light bounces at bake time; CSM + SDF cover direct-light occlusion at runtime. Together these replace what lightmaps would contribute in a traditional Quake-lineage pipeline — without the per-surface UV unwrap or lightmap texture overhead.

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
        → MapLight in MapData.lights                       (all lights)
          │
          ├─→ SH irradiance volume baker                   (all lights)
          │     (ray-casts through the Milestone 4 BVH,
          │      SH L2 projection, validity mask)
          │     → SH section in .prl
          │       → runtime trilinear sample (fragment shader,
          │          indirect term for static surfaces AND
          │          dynamic entities at their position)
          │
          ├─→ SDF atlas baker                              (static geometry, not light-specific)
          │     (closest-point queries through the BVH,
          │      brick-indexed sparse distance field)
          │     → SdfAtlas section in .prl
          │       → runtime sphere trace (fragment shader,
          │          point + spot shadow term)
          │
          └─→ runtime direct light buffer                  (bake_only=false lights only)
              (AlphaLights section; transient gameplay lights join in Milestone 6+)
                → flat per-fragment light loop (direct term)
                  → CSM sample for directional (sub-plan 5)
                  → SDF sphere trace for point/spot (sub-plan 8)
```

---

## Sub-plans

This plan has eight sub-files. Sub-plans 1–2 and 8's baker half are compiler/data work; sub-plans 3–6, 8, and 9's runtime half are engine-side, ordered so each step has a clear visual validation surface before the next layer is added. Sub-plan 1 has no engine impact and could overlap with the tail end of Milestone 4 in principle, but the rule is: nothing from this plan starts until Milestone 4's check-in gate signs off.

**Dependency graph summary:**
- Sub-plan 1 must complete before anything else.
- Sub-plans 2 (compiler-side SH baker) and 3 (engine-side direct lighting) can proceed **in parallel** once sub-plan 1 is done — they are independent work streams.
- Sub-plan 4 (light influence volumes) depends on sub-plan 3. Sub-plan 5 (CSM) depends on sub-plan 3 and **benefits from** sub-plan 4. Sub-plans 4 and 5 are otherwise independent of each other.
- Sub-plan 6 (SH volume runtime) depends on sub-plans 2 and 3, but is independent of sub-plans 5 and 8.
- Sub-plan 7 (animated SH) is deprioritized to Future (see Out of scope); it can ship as a follow-up once the rest of Milestone 5 is complete.
- Sub-plan 8 (SDF + sphere-traced shadows) depends on sub-plan 3 and the Milestone 4 BVH; **benefits from** sub-plan 4. Independent of sub-plans 5 and 6.
- Sub-plan 9 (specular maps) depends on sub-plan 8 and a shading model decision recorded in the sub-plan file before implementation starts.

### Compiler / data pipeline

1. **[1-fgd-canonical.md](./1-fgd-canonical.md)** — FGD light entities, parser wiring, translator, map light format. Pure compiler/data work, no engine changes. Output: `MapData.lights: Vec<MapLight>` populated for every test map; `_bake_only` authored lights skip the runtime AlphaLights section. **Gate:** sub-plans 2, 3, and 9 may not start until this is done.

2. **[2-sh-baker.md](./2-sh-baker.md)** — SH irradiance volume baker stage in `prl-build`, plus the SH PRL section. Ray-casts through the Milestone 4 BVH. Consumes all `MapData.lights` (both runtime-dynamic and bake-only). Output: every test map emits an SH section. **Depends on:** sub-plan 1. **Parallel with:** sub-plan 3.

### Engine runtime (ordered by visual validation dependencies)

3. **[3-direct-lighting.md](./3-direct-lighting.md)** — Direct lighting via a flat per-fragment light loop + ambient floor. Uploads map lights (runtime-dynamic only) to a GPU storage buffer, evaluates Lambert diffuse with per-type falloff and spot cone attenuation. This is the foundation — sub-plans 4, 5, and 9 are all validated relative to what this shows. Uses a flat loop, not clustered forward+; clustering is a future optimization when light counts demand it. **Depends on:** sub-plan 1. **Parallel with:** sub-plan 2.

4. **[4-light-influence-volumes.md](./4-light-influence-volumes.md)** — Light influence volumes (compile-time per-light sphere bounds in PRL, runtime spatial culling). **Depends on:** sub-plan 3. **Enables:** sub-plan 5's CSM slot allocation and sub-plan 9's per-light sphere-trace gating (lights whose influence volume doesn't intersect the view don't trace).

5. **[5-shadow-maps.md](./5-shadow-maps.md)** — CSM for directional/sun shadows. Hard-edge cascaded shadow maps with nearest-neighbor sampling and rotation-invariant texel snapping. Point and spot shadows are handled by sub-plan 9 (SDF sphere-trace), not here. **Depends on:** sub-plan 3. **Benefits from:** sub-plan 4.

6. **[6-sh-volume.md](./6-sh-volume.md)** — SH irradiance volume sampling (indirect lighting). Loads the SH PRL section from sub-plan 2, uploads to 3D textures, trilinear samples in the fragment shader, reconstructs SH L2 irradiance. Replaces flat ambient as the indirect term for static surfaces and provides indirect lighting for dynamic entities via probe sampling at the entity's position. **Depends on:** sub-plans 2 and 3. **Independent of:** sub-plans 5 and 8.

7. **[7-animated-sh.md](./7-animated-sh.md)** — Animated SH layers. Loads per-light monochrome SH layers and animation descriptors, evaluates brightness/color curves per frame, modulates and adds to base SH. **Deprioritized to Future** — ships as a follow-up once the rest of Milestone 5 is complete. **Depends on:** sub-plan 6.

8. **[8-sdf-shadows.md](./8-sdf-shadows.md)** — SDF atlas baker + sphere-traced soft shadows for point and spot lights. Replaces cube shadow maps entirely. Bake-time: brick-indexed sparse distance field over all static geometry, written as a new PRL section. Runtime: single fragment-shader trace per visible shadow-casting light, gated by `shadow_kind == 2`. Chunk-friendly brick addressing so Milestone 8's chunk primitive migration is additive. **Depends on:** sub-plan 3, Milestone 4 BVH. **Benefits from:** sub-plan 4.

9. **[9-specular-maps.md](./9-specular-maps.md)** — Specular maps. Per-texel specular intensity and color evaluated in the direct light loop, adding a highlight term on top of Lambert diffuse. Shading model decision (Phong vs. PBR) required before implementation starts — recorded in the sub-plan file. **Depends on:** sub-plan 8.

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
- Flat per-fragment direct light loop with influence-volume early-out: per-light-type evaluation (point/spot/directional), falloff models, spot cone attenuation. Target: 500 authored lights per level; influence volumes ensure the per-fragment cost scales with nearby lights, not total lights. Clustered forward+ binning deferred until profiling shows the flat loop bottlenecks.
- Shadow pipeline: CSM for directional (sub-plan 5); SDF atlas + sphere-traced soft shadows for point and spot (sub-plan 9). No cube shadow maps, no per-spot 2D shadow maps.
- Test map coverage extending `assets/maps/test.map`.
- Documentation update to `context/lib/build_pipeline.md` §Custom FGD table.

### Out of scope

- UDMF map-format support. Separate initiative. The `format/<name>.rs` architecture established here accommodates it without refactor.
- `env_projector` and texture-projecting lights.
- IES profiles / photometric data.
- Area lights (rectangle, disk). Point / spot / directional cover the target feature set.
- Second-bounce indirect. The SH volume captures direct-to-static bounces; multi-bounce is a follow-up if visuals demand it.
- Runtime dynamic probe updates (DDGI-style). The SH volume is baked, read-only at runtime.
- Animated SH layers (sub-plan 7). Deprioritized to Future — ships as a follow-up once the rest of Milestone 5 is complete.
- Dynamic SDF rebake. The sub-plan 9 SDF is baked once per level; kinematic clusters and destruction-driven SDF invalidation are addressed in Milestones 9 and 10.
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
- CSM defaults (resolution, cascade count, depth bias, bounding-sphere texel-snap fit).
- SDF atlas layout (brick indirection, voxel quantization, sentinel slots) and sphere-trace soft-shadow parameters.
- Baker ↔ BVH sharing pattern: one acceleration structure, three consumers (runtime cull GPU, SH baker CPU, SDF baker CPU).

The plan document itself is ephemeral per `development_guide.md` §1.5.
