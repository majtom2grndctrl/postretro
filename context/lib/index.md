# Postretro – Architecture Index

> **Use as a router:** pick 2–3 linked docs for the task, don't load everything.
> **Source of truth for:** product definition, architectural principles, and where contracts live.
> **Not for:** implementation details (load the specific doc instead).
> **Pre-stable note:** refactors may introduce breaking changes; update all call sites and related tests in the same change.

## Agent Router (Task → Minimal Docs)

- **Engineering conventions / code style** → `development_guide.md`
- **Context file writing / updates** → `context_style_guide.md`
- **Testing** → `testing_guide.md`
- **Rendering pipeline / lighting** → `rendering_pipeline.md`
- **PRL format / level compiler / runtime portal vis** → `build_pipeline.md`
- **Brush roles / which brushes participate in the BSP** → `build_pipeline.md` §Brush role spectrum
- **Audio / spatial sound / reverb zones** → `audio.md`
- **Entity model / game objects / sprites** → `entity_model.md`
- **Build pipeline / FGD / TrenchBroom** → `build_pipeline.md`
- **Input handling / gamepad** → `input.md`
- **Resource management / textures / materials** → `resource_management.md`
- **Scripting / primitives / SDK types / hot reload** → `scripting.md`
- **Collision / player movement** → `entity_model.md` §7 · `reference/collision-without-bsp.md` · `plans/drafts/grounded-movement/index.md`
- **Frame timing / game loop** → `rendering_pipeline.md` §1 · `entity_model.md` §5
- **Roadmap / implementation phases** → `plans/roadmap.md`
- **Draft plans / future features** → `plans/drafts/`
- **Shipped plans** → `plans/done/` — historical record, frozen at ship time. May describe stale state. Read only when explicitly referenced.
- **Research archive** → `research/` — past research, not current design. Do not read unless explicitly instructed.
- **3rd party library docs** → use `context7` tool (wgpu, winit, kira, glam).

---

## 1. Product Definition

Retro-style FPS engine. Doom/Quake boomer shooter with a cyberpunk aesthetic. Low-poly 3D environments with fully dynamic direct lighting, baked volumetric indirect lighting (SH irradiance volumes), normal-mapped surfaces, billboard sprite characters, and modern embellishments (bloom, particles). Visual fidelity through a lean, wgpu-driven pipeline — not a modern engine with retro filters. Near-instant boot, tiny binary.

---

## 2. Architectural Principles

| Principle | Invariant |
|-----------|-----------|
| **Renderer owns GPU** | All wgpu calls live in the renderer module. Other subsystems never touch wgpu types. |
| **Baked over computed** | Spatial data and indirect lighting baked offline. Two deliberate exceptions: visibility computes per frame from baked portal geometry (id Tech 4 lineage; `--pvs` precomputed fallback exists), and direct illumination is fully dynamic (flat per-fragment light loop with per-light influence-volume early-out and shadow maps; up to 500 lights per level). Baked SH irradiance volume carries indirect light; dynamic lights drive direct shading. |
| **Subsystem boundaries** | Renderer, audio, input, game logic are distinct modules with explicit contracts. |
| **Frame ordering** | Input → Game logic → Audio → Render → Present. Later stages depend on earlier ones. |
| **No `unsafe`** | The crate stack provides safe APIs. If `unsafe` appears necessary, stop and consult the project owner. |

---

## 3. Baked Data Strategy

Single authoring pipeline: TrenchBroom `.map` → `prl-build` → `.prl`. Engine loads `.prl` as the sole runtime map format.

prl-build uses a BSP tree as a compiler intermediate to produce cells, portal geometry, and per-cell draw chunks. The runtime consumes cells and portals; it does not walk BSP nodes for rendering or visibility. (`BspNodes`/`BspLeaves` sections are still emitted for camera-leaf lookup — replacing that with a cell-location section is a future step.) `--pvs` mode produces a precomputed PVS bitset as a fallback. Designed to subsume all baked data in engine-native coordinates. See `build_pipeline.md`.

### PRL baked data

| Data | Source | How |
|------|--------|-----|
| Geometry | prl-build (brush-volume BSP → brush-side projection → pack) | Geometry section — positions, UVs, packed normals, packed tangents, per-face metadata |
| BSP tree | prl-build | BspNodes + BspLeaves sections (compile-time scaffolding; see `build_pipeline.md`) |
| Visibility | prl-build (portal traversal or PVS) | Portals section (default) or LeafPvs section (`--pvs` mode) |
| Surface material types | Texture naming convention | Prefix lookup table → footsteps, impacts, decals |
| Light entities | FGD entities (`light`, `light_spot`, `light_sun`) | Parsed and translated to canonical format at compile time |
| Indirect lighting | prl-build (Milestone 5) | SH L2 irradiance volume baked from canonical lights; stored in PRL section |
| Fog volumes | FGD entity (`env_fog_volume`) | Brush entity resolved to BSP leaves at load time |
| Reflection probes | FGD entity (`env_cubemap`) | Point entity → baked cubemap |
| Acoustic zones | FGD entity (`env_reverb_zone`) | Brush entity resolved to BSP leaves at load time |
| Animated light weight maps | prl-build (animated-light-weight-maps plan) | SectionId 25 — per-texel light-weight data for the animated-lightmap compose pass; see `build_pipeline.md` for the full PRL section inventory |

`chart_raster.rs` (in `postretro-level-compiler`) is a shared baker helper: both the static lightmap baker and the animated-weight baker use it to derive world positions and normals from chart placements without duplication.

Full detail: `build_pipeline.md`.

---

## 4. Non-Goals

- General-purpose game engine
- ECS architecture
- Deferred rendering
- Runtime level compilation
- Multiplayer / networking
