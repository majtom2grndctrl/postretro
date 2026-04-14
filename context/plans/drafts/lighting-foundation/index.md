# Lighting Foundation

> **Status:** draft — architectural direction locked. Sub-plans fill out as we drill into each one.
> **Milestone:** 5 (Lighting Foundation) — see `context/plans/roadmap.md`.
> **Related:** `context/lib/rendering_pipeline.md` §4 · `context/lib/build_pipeline.md` §Custom FGD · `context/lib/entity_model.md` · `context/reference/light-entities-across-engines.md`
> **Prerequisite:** Milestone 4 (BVH Foundation) — must ship and pass its check-in gate before any work in this plan begins. The SH baker traverses the BVH built in Milestone 4. See `context/plans/ready/bvh-foundation/`.
> **Lifecycle:** Per `context/lib/development_guide.md` §1.5, this plan *is* the lighting spec while it's a draft. Decisions land in the spec as they're made; durable knowledge migrates to `context/lib/` when the plan ships.

---

## Goal

Replace flat ambient with the full target lighting pipeline:

- **Indirect:** SH L2 irradiance volume baked at compile time, sampled per fragment via 3D texture trilinear interpolation.
- **Direct:** clustered forward+ dynamic lights (point, spot, directional) authored as FGD entities and consumed both by the bake and by the runtime direct path.
- **Surface detail:** tangent-space normal maps perturbing the per-fragment normal before shading.
- **Dynamic shadows:** cascaded shadow maps for directional lights, cube shadow maps for point and spot lights. Low-resolution, nearest-neighbor sampling — chunky pixel shadow edges match the target aesthetic.

The translator is decoupled from the parser: future map-format support (e.g., UDMF) adds a sibling module against the same canonical types, so the baker and everything downstream never learn the source format.

**Shadow coverage:** the SH irradiance volume captures indirect light bounces at bake time; dynamic shadow maps cover direct-light occlusion at runtime. Together these replace what lightmaps would contribute in a traditional Quake-lineage pipeline.

---

## How this plan rides on Milestone 4

The SH baker ray-casts probe samples through the **same BVH** that Milestone 4 builds for runtime cull. One acceleration structure, two consumers:

- Milestone 4 runtime: WGSL compute shader walks the BVH for frustum cull each frame.
- Milestone 5 baker: CPU `bvh` crate walks the same flattened tree to ray-cast probe radiance samples and shadow rays.

This means **no second design pass for spatial structures.** The baker calls into the `bvh` crate directly using the same primitive set the compiler built in `context/plans/ready/bvh-foundation/1-compile-bvh.md`. No separate baker BVH, no embree, no hardware RT.

---

## Pipeline

```
TrenchBroom authoring (FGD)
  → .map file
    → prl-build parser (extract property bag)
      → format::quake_map::translate_light (validate, convert)
        → CanonicalLight in MapData.lights
          ├─→ SH irradiance volume baker
          │     (ray-casts through the Milestone 4 BVH via bvh crate,
          │      SH L2 projection, validity mask)
          │     → SH section in .prl
          │       → runtime trilinear sample (fragment shader,
          │          indirect term)
          └─→ runtime direct light buffer
              (canonical lights + transient gameplay lights)
                → clustered light list compute prepass
                  → cluster walk in fragment shader (direct term)
                    → shadow map sampling per shadow-casting light
```

---

## Sub-plans

This plan has three sub-files, executed roughly in order. Sub-plan 1 has no engine impact and could overlap with the tail end of Milestone 4 in principle, but the rule is: nothing from this plan starts until Milestone 4's check-in gate signs off.

1. **[1-fgd-canonical.md](./1-fgd-canonical.md)** — FGD light entities, parser wiring, translator, canonical light format. Pure compiler/data work, no engine changes. Output: `MapData.lights: Vec<CanonicalLight>` populated for every test map.

2. **[2-sh-baker.md](./2-sh-baker.md)** — SH irradiance volume baker stage in `prl-build`, plus the SH PRL section. Ray-casts through the Milestone 4 BVH. Output: every test map emits an SH section.

3. **[3-runtime-lighting.md](./3-runtime-lighting.md)** — All engine-side lighting work: SH volume loader and 3D texture upload, world shader extension for indirect sampling, normal map loading and TBN reconstruction, clustered forward+ light list compute prepass, fragment shader direct term, shadow map passes.

---

## Scope

### In scope

- FGD entities `light`, `light_spot`, `light_sun` in `assets/postretro.fgd`
- `postretro-level-compiler/src/format/quake_map.rs` translator module. Pattern is `format/<name>.rs` per source format; each format's internal structure is its own decision.
- Parser wiring: `prl-build` extracts light entity properties into a property bag and dispatches to the translator.
- Canonical light format: `LightType`, `FalloffModel`, `LightAnimation`, `CanonicalLight`. Format-agnostic.
- Validation rules (errors block compilation; warnings log and proceed).
- Quake `style` integer → `LightAnimation` preset conversion. Canonical format is preset-free; the translator owns the Quake style table.
- SH irradiance volume baker: probe placement, radiance evaluation with shadow raycasting through the Milestone 4 BVH, SH L2 projection, validity masking, PRL section writer.
- Runtime SH volume sampling: parse PRL section to 3D texture, trilinear sampling in world shader.
- Normal map loading and tangent-space shading in the world shader.
- Clustered light list compute prepass: cluster grid definition, per-cluster light index lists, fragment-shader walk.
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

## Resolved questions

| Question | Decision | Rationale |
|----------|----------|-----------|
| Baker approach | Probes only for indirect; dynamic direct at runtime | Lightmaps add a bake stage, an atlas, a UV channel, and a two-texture sampling path in the shader. SH volume + dynamic direct is lighter. |
| Spatial layout | Regular 3D grid | Trivial indexing, hardware trilinear, low complexity. Octree adaptivity wins little on small indoor maps. |
| Per-probe storage | SH L2 (27 f32/probe) | Smooth reconstruction, small shader cost, industry-standard. Ambient cube rejected as needing nearly as much storage for less smoothness. |
| Probe evaluation | Trilinear interpolation on a 3D texture | Hardware-accelerated; zero shader complexity. |
| Shadow strategy (bake) | Raycast at bake time, per light per probe — traverses Milestone 4 BVH | Bake is expensive but runs once. Runtime shadow estimation on the volume is not worth the complexity. |
| Shadow strategy (runtime) | Shadow maps per dynamic shadow-caster | CSM for directional, cube for point/spot. Matches aesthetic (chunky edges at modest resolution). Not hardware ray tracing. |
| Animation baking | Bake per-probe sample vectors; defer to execution plan | Animation support may be cut from the initial revision if it complicates the first pass. |
| Acceleration structure for the baker | The Milestone 4 BVH | One structure, two consumers. No separate baker BVH. |

---

## When this plan ships

Durable architectural decisions migrate to `context/lib/rendering_pipeline.md` (`context/lib/lighting.md` if the section outgrows §4):

- Canonical format struct shape and design rationale.
- `format/<name>.rs` architecture for multi-format source support.
- SH volume spatial layout, per-probe storage, validity masking.
- SH volume PRL section shape.
- Clustered forward+ cluster grid parameters and shadow map defaults.
- Baker ↔ BVH sharing pattern: one acceleration structure, two consumers (bake-time CPU, runtime GPU).

The plan document itself is ephemeral per `development_guide.md` §1.5.
