# SSR Decals

> **Status:** pre-draft — low priority placeholder. Scope and tasks need a deeper research pass before promotion.
> **Related:** `context/lib/rendering_pipeline.md` §4, `context/lib/resource_management.md`

---

## Goal

Add projected decals with per-decal specmaps that gate screen-space reflections. Primary use case: puddles that cover a portion of a floor surface without modifying the underlying texture. Lets level authors place wet areas as map entities rather than baking them into texture UVs.

---

## Scope

### In scope

- Decal projection volumes placed as map entities in TrenchBroom.
- Per-decal albedo + specmap pair. Specmap R channel drives SSR reflection intensity (0 = dry, 1 = fully reflective).
- SSR fullscreen post-process pass: screen-space ray march against the depth buffer, modulated per-fragment by the decal specmap contribution.
- Decals render after opaque world geometry, before post-processing.
- Decals degrade gracefully when SSR is disabled (specmap contribution is discarded, albedo still blends).

### Out of scope

- Animated decals (filling/draining puddles).
- Decals on dynamic entities.
- Off-screen reflections — SSR only; no cubemap fallback in this plan.
- Parallax or planar reflections.
- Decal normal maps (the underlying surface normal is used).

---

## Open questions

- **Shader slot budget.** Forward pass already at 16 slots. Decals likely render in a separate pass — confirm they don't need to share the forward bind group layout.
- **Depth buffer availability.** SSR needs a readable depth buffer. Confirm wgpu texture usage flags allow sampling the depth prepass output as a texture in a subsequent fullscreen pass.
- **Decal entity format.** Projection volume shape (OBB vs AABB), blend mode (alpha-over), and map FGD properties TBD during deeper research.
- **Decal count / performance budget.** No cap decided. Needs profiling on target hardware before committing to a culling strategy.
- **Interaction with lightmaps.** Decal albedo sits on top of baked lighting. Whether decals receive lightmap irradiance (via lightmap UV projection) or just inherit ambient floor TBD.

---

## Acceptance criteria

*(Placeholder — needs refinement before promotion.)*

- [ ] Level author places a puddle decal entity in TrenchBroom; compiled map renders a wet patch on the floor at that position.
- [ ] SSR reflections appear only where decal specmap R > 0.
- [ ] Disabling SSR shows the decal albedo without reflection artifacts.
- [ ] No crash or visual corruption when zero decals are present in a map.
