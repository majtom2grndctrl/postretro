# Postretro – Architecture Index

> **Use as a router:** pick 2–3 linked docs for the task, don't load everything.
> **Source of truth for:** product definition, architectural principles, and where contracts live.
> **Not for:** implementation details (load the specific doc instead).
> **Pre-stable note:** refactors may introduce breaking changes; update all call sites and related tests in the same change.

## Agent Router (Task → Minimal Docs)

- **Engineering conventions / code style** → `development_guide.md`
- **Context file writing / updates** → `context_style_guide.md`
- **Testing** → `testing_guide.md`
- **Rendering pipeline / BSP / lighting / BSPX** → `rendering_pipeline.md`
- **PRL format / level compiler / clusters / PVS** → `plans/prl-spec-draft.md` · `build_pipeline.md` §PRL
- **Audio / spatial sound / reverb zones** → `audio.md`
- **Entity model / game objects / sprites** → `entity_model.md`
- **Build pipeline / ericw-tools / FGD / TrenchBroom** → `build_pipeline.md`
- **Input handling / gamepad** → `input.md`
- **Resource management / textures / materials** → `resource_management.md`
- **Collision / player movement** → `entity_model.md` §7 · `reference/collision-without-bsp.md` · `plans/drafts/grounded-movement/index.md`
- **Frame timing / game loop** → `rendering_pipeline.md` §1 · `entity_model.md` §5
- **Roadmap / implementation phases** → `plans/roadmap.md`
- **Draft plans / future features** → `plans/drafts/`

---

## 1. Product Definition

Retro-style FPS engine. Doom/Quake boomer shooter with a cyberpunk aesthetic. Low-poly 3D environments with baked lightmaps, billboard sprite characters, and modern embellishments (dynamic colored lights, bloom, particles). Visual fidelity through genuinely retro technology — not a modern engine with retro filters. Near-instant boot, tiny binary.

---

## 2. Architectural Principles

| Principle | Invariant |
|-----------|-----------|
| **Renderer owns GPU** | All wgpu calls live in the renderer module. Other subsystems never touch wgpu types. |
| **Baked over computed** | Lighting, visibility, and spatial data are baked offline. BSP levels use ericw-tools; PRL levels use prl-build. Dynamic lights supplement, not replace. |
| **Subsystem boundaries** | Renderer, audio, input, game logic are distinct modules with explicit contracts. |
| **Frame ordering** | Input → Game logic → Audio → Render → Present. Later stages depend on earlier ones. |
| **No `unsafe`** | The crate stack provides safe APIs. If `unsafe` appears necessary, stop and consult the project owner. |

---

## 3. Baked Data Strategy

Two authoring pipelines, both consuming TrenchBroom `.map` files:

- **BSP path** (current): ericw-tools compiles to `.bsp`. Engine consumes BSP/BSPX lumps.
- **PRL path** (in development): prl-build compiles to `.prl`. Engine consumes cluster-based binary sections.

The PRL format replaces per-leaf BSP visibility with cluster-based PVS, stores geometry in engine-native coordinates, and is designed to subsume the baked data currently provided by BSPX lumps. The compiler uses a voxel grid for solid/empty classification and ray-cast visibility — no BSP tree. See `plans/prl-spec-draft.md` for the full format spec and `build_pipeline.md` §PRL for the compiler pipeline.

### BSP baked data (current)

| Data | Source | How |
|------|--------|-----|
| Colored lightmaps | ericw-tools (`light -bspx`) | `RGBLIGHTING` BSPX lump |
| Directional lightmaps | ericw-tools (`light -bspx`) | `LIGHTINGDIR` BSPX lump → per-pixel specular |
| Ambient occlusion | ericw-tools (worldspawn `_dirt 1`) | Baked into lightmap data, no separate lump |
| Volumetric light probes | ericw-tools (`light -lightgrid`) | `LIGHTGRID_OCTREE` BSPX lump → sprite/particle lighting (experimental for Q1) |
| Surface material types | Texture naming convention | Prefix lookup table → footsteps, impacts, decals |
| Fog volumes | FGD entity (`env_fog_volume`) | Brush entity resolved to BSP leaves at load time |
| Reflection probes | FGD entity (`env_cubemap`) | Point entity → baked cubemap |
| Acoustic zones | FGD entity (`env_reverb_zone`) | Brush entity resolved to BSP leaves at load time |

Full detail: `build_pipeline.md`.

---

## 4. Non-Goals

- General-purpose game engine
- ECS architecture
- Deferred rendering
- Extending or forking ericw-tools
- Runtime level compilation
- Multiplayer / networking
