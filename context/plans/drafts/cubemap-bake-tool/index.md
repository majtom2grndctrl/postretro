# Cubemap Bake Tool

> **Status:** pre-draft — depends on features not yet implemented. Depth and sub-tasks will be refined as dependencies land.
> **Blocked by:** Phase 3 (textured world), Phase 5 (advanced lighting) — see `context/plans/roadmap.md`
> **Related:** `context/lib/build_pipeline.md`, `context/lib/resource_management.md`, `context/lib/rendering_pipeline.md`

---

## Goal

Build an offline tool that renders cubemaps from `env_cubemap` entity positions in a compiled BSP. Output is a set of cubemap textures consumed at runtime for environment-mapped reflections on wet floors, chrome, glass, and other reflective surfaces.

---

## Pipeline integration

1. Mapper places `env_cubemap` point entities in TrenchBroom.
2. Map compiles through standard ericw-tools pipeline (qbsp → vis → light).
3. **Cubemap bake tool** loads the compiled BSP, finds all `env_cubemap` entities, renders six faces from each position, outputs cubemap images.
4. Engine loads cubemaps at runtime alongside the BSP. Surfaces assigned to nearest cubemap probe by spatial proximity.

---

## Scope

### In scope

- Load compiled BSP and extract `env_cubemap` entity positions from the entity lump.
- Render six axis-aligned views (±X, ±Y, ±Z) from each position using the engine's BSP renderer.
- Include baked lighting (lightmaps) in cubemap renders for accurate reflections.
- Output cubemaps as image files (PNG or KTX2) in a directory alongside the BSP.
- Naming convention: cubemap files keyed by entity index or position hash for deterministic matching at load time.

### Out of scope

- Real-time cubemap updates
- Parallax-corrected cubemaps (box projection)
- Cubemap filtering / mip generation (use runtime GPU filtering or a separate image processing step)
- HDR cubemaps
- Reflection probe blending between multiple cubemaps

---

## Key decisions to make during refinement

- Output format: PNG (simple, large) vs. KTX2 (compressed, GPU-native)
- Cubemap resolution per face (64x64? 128x128? configurable?)
- Whether the bake tool shares renderer code with the engine or is a standalone reimplementation
- Whether dynamic lights are included in bakes or only baked lighting
- How cubemaps are matched to surfaces at runtime (nearest probe, manual assignment, BSP leaf overlap)

---

## Acceptance criteria

- Tool loads a BSP with `env_cubemap` entities and produces one cubemap per entity.
- Cubemaps show correct BSP world geometry with baked lighting.
- Engine loads produced cubemaps and applies them to reflective surfaces.
- Missing cubemap files degrade gracefully (no reflection, not a crash).
