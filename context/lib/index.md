# Postretro – Architecture Index

> **Use as a router:** pick 2–3 linked docs for the task, don't load everything.
> **Source of truth for:** product definition, architectural principles, and where contracts live.
> **Not for:** implementation details (load the specific doc instead).
> **Pre-stable note:** refactors may introduce breaking changes; update all call sites and related tests in the same change.

## Agent Router (Task → Minimal Docs)

- **Engineering conventions / code style** → `development_guide.md`
- **Crate layering / where new code goes / dependency direction** → `development_guide.md` §Workspace
- **Crate dependency graph / blast radius / what depends on X** → `crate-graph.md` (generated); live queries via `cargo run -p xtask -- crate-graph --rdeps <crate>`
- **Context file writing / updates** → `context_style_guide.md`
- **Testing** → `testing_guide.md`
- **Asserting on log output / log capture in tests** → `testing_guide.md` §3 · entry point `crates/test-log-capture`
- **Rendering pipeline / lighting** → `rendering_pipeline.md`
- **Frame capture / offscreen readback / headless (surfaceless) rendering** → `rendering_pipeline.md` §7.8
- **Projectile visuals / emissive billboards / flipbook sprite bodies / mover-attached dynamic lights / impact-flash light / animated light radius** → `rendering_pipeline.md` §4, §7.4 · `resource_management.md` §6
- **PRL format / level compiler / runtime portal vis** → `build_pipeline.md` §PRL Compilation
- **Cell→cell coupling relation / baked cell-visibility substrate / network relevance, audio occlusion, AI-perception broad phase, or VFX cull shared foundation** → `build_pipeline.md` §PRL section IDs
- **Brush roles / which brushes participate in the BSP** → `build_pipeline.md` §Compiler pipeline
- **Audio / spatial sound / reverb zones** → `audio.md`
- **Entity model / game objects / sprites** → `entity_model.md`
- **Enemy AI / behavior state graph / transition guards / brain component** → `entity_model.md` §7c · `scripting.md` §11
- **Hierarchical enemy behavior / statecharts / nested activities / layers / committed attack phases** → `entity_model.md` §7c · `scripting.md` §11
- **Build pipeline / FGD / TrenchBroom** → `build_pipeline.md`
- **Input format adapters / adding a new map source format / what Quake or TrenchBroom vocabulary may cross into shared compiler stages** → `build_pipeline.md` §Source-format neutrality
- **Input handling / gamepad** → `input.md`
- **Player options / settings persistence / mouse sensitivity / invert-Y / view_feel_scale** → `player_options.md`
- **UI layer / HUD / widgets / theming / UI state binding** → `ui.md`
- **Resource management / textures / materials** → `resource_management.md`
- **3D model / glTF import (scale, pivot, material format)** → `resource_management.md` §7
- **Scripting / primitives / SDK types / scripting crate boundaries / VM compile firewall** → `scripting.md`
- **Reaction dispatch model / event sources / dispatch scopes / reaction parameters / occupancy exposure** → `scripting.md` §12
- **Netcode / multiplayer / co-op / replication / transport / wire format** → `networking.md`
- **Joining a session / admission vs content parity / slot lifecycle / host level change / what gates vs what replicates** → `networking.md` §Admission and content parity · §Slot lifecycle · §What gates, and what replicates instead
- **First-person weapon placement / viewmodel offset / where a weapon sits in view / placement vs view-feel / FP vs TP weapon vantage** → `networking.md` §Weapon placement is content
- **Projectile fire origin / muzzle point / where a shot spawns / camera-eye vs barrel** → `networking.md` §Weapon placement is content (Fire origin composes on placement)
- **Game / mod author docs (human-facing, not agent context)** → `docs/`
- **Collision (world/entity)** → `entity_model.md` §7
- **Navigation / navmesh / pathfinding representation** → `build_pipeline.md` §Navigation bake
- **Player movement / movement states / FPS feel** → `movement.md`
- **Frame timing / game loop** → `rendering_pipeline.md` §1 · `entity_model.md` §5
- **Boot / startup / splash / level-load sequence / mod loading** → `boot_sequence.md`
- **Experimental spikes / build-to-learn specs** → `experimental_spikes.md`
- **3rd party library docs** → use `context7` tool (wgpu, winit, kira, glam).

---

## 1. Product Definition

**Retro-inspired FPS engine** — a hybrid of new and old. Doom/Quake boomer shooter with a cyberpunk aesthetic. Monster closets and scripted reveals are first-class set-pieces rather than engine-fighting workarounds, making for theatrical gameplay experiences. Inspired by retro look and feel but game design is a meaningful iteration beyond games of the period.

**Aesthetic:** Low-poly 3D environments + blocky pixelated textures; with modern embellishments like baked volumetric indirect lighting (SH irradiance volumes), normal-mapped surfaces, dynamic direct lighting, and billboard sprite volumetrics that react to light.

**Architectural northstar:** Lean, wgpu-driven pipeline — not a resource heavy modern engine with retro filters. Near-instant boot, tiny binary, and _some_ retro filters, but used sparingly.

---

## 2. Architectural Principles

| Principle | Invariant |
|-----------|-----------|
| **Renderer owns GPU** | All wgpu calls live in the renderer module. Other subsystems never touch wgpu types. |
| **Baked over computed** | Spatial data and indirect lighting are baked offline; portal traversal normally computes visibility per frame from baked portal geometry. Defined fallback cases use per-cell AABB frustum culling. Direct light may be baked (static lightmaps; baked layers for movers) or evaluated at runtime — whether a light is authored static (baked) or dynamic (runtime) is an **authoring choice, not an engine rule**. The one engine invariant: a physical light's contribution must never be **double-counted on a given receiver** — overlapping static and dynamic light must not over-brighten the same fragment. Lighting techniques compose additively in the forward pass. |
| **Subsystem boundaries** | Renderer, audio, input, game logic are distinct modules with explicit contracts. |
| **Frame ordering** | Input → Game logic → Audio → Render → Present. Later stages depend on earlier ones. |
| **No `unsafe`** | The crate stack provides safe APIs. If `unsafe` appears necessary, stop and consult the project owner. |
| **Primitive surface is a contract** | Engine parameters exposed as scripting primitives carry API contracts. Changing semantics, valid ranges, or clamping behavior requires updating the scripting surface — SDK types, validation rules, and reaction constructors — in the same pass. |

---

## 3. Baked Data Strategy

Single authoring pipeline today: TrenchBroom `.map` → `prl-build` → `.prl`. Engine loads `.prl` as the sole runtime map format. One input format is a content decision, not an architectural one — the compiler's `format/` adapter translates source vocabulary to canonical engine terms so a second front end can target PRL without touching a shared stage. See `build_pipeline.md` §Source-format neutrality.

prl-build uses a BSP tree as a compiler intermediate to produce cells, portal geometry, and per-cell draw chunks. The runtime consumes cells, a cell locator, portals, and BVH arrays; it does not load or walk BSP nodes for rendering or visibility. Portal traversal normally computes visibility; solid-cell, exterior-camera, and no-portals cases fall back to per-cell AABB frustum culling. Designed to subsume all baked data in engine-native coordinates. See `build_pipeline.md`.

### PRL baked data

| Data | Source |
|------|--------|
| Geometry | prl-build (brush-volume BSP → brush-side projection → pack) |
| BSP tree | prl-build (compile-time scaffolding only; not emitted as runtime spatial sections) |
| Visibility | prl-build (portal generation — runtime traverses portal graph each frame) |
| Cell locator | prl-build (compiler BSP-derived point-to-cell decision tree) |
| Light entities | FGD entities parsed and translated to canonical format at compile time |
| Indirect lighting | SH L2 irradiance volume baked from canonical lights |
| Fog volumes | FGD fog entities + `FogCellMasks` over runtime cells |
| Acoustic zones | FGD brush entities intended to resolve through runtime cells for reverb |
| Reflection probes | FGD point entities → baked cubemaps |

Full detail (section inventory, SectionId registry): `build_pipeline.md`.

---

## 4. Non-Goals

- General-purpose game engine
- General-purpose / extensible ECS framework — archetype storage, query planner, system scheduler, modder-defined component types. Internal storage *is* data-oriented (dense per-kind component columns); the component *vocabulary* is engine-closed. See `entity_model.md` §1.
- Deferred rendering
- Runtime level compilation
- General-purpose multiplayer — deterministic lockstep / rollback, competitive PvP, matchmaking, anti-cheat, peer-to-peer topologies, full server-rewind lag compensation. Authoritative client-server **co-op** is in scope.
