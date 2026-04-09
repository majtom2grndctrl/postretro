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
- **PRL format / level compiler / BSP / PVS** → `build_pipeline.md` §PRL
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

Single authoring pipeline: TrenchBroom `.map` → `prl-build` → `.prl`. Engine loads `.prl` as the primary format.

**PRL path (primary):** prl-build compiles geometry, BSP tree, portal graph, and PVS. Engine consumes BSP tree, per-leaf PVS/portals, and geometry sections. Designed to subsume all baked data in engine-native coordinates. See `build_pipeline.md` §PRL.

**BSP path (legacy support):** Engine can still load `.bsp` files compiled with ericw-tools. No active development on the BSP authoring pipeline. Useful for loading existing assets during the transition. See `build_pipeline.md` §BSP.

### PRL baked data

| Data | Source | How |
|------|--------|-----|
| Geometry | prl-build (CSG clip → BSP → pack) | Geometry section — positions, indices, per-face metadata |
| BSP tree | prl-build | BspNodes + BspLeaves sections |
| Visibility | prl-build (portal traversal or PVS) | Portals section (default) or LeafPvs section (`--pvs` mode) |
| Surface material types | Texture naming convention | Prefix lookup table → footsteps, impacts, decals |
| Lighting | prl-build (Phase 4) | PRL-native sections — details TBD |
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
