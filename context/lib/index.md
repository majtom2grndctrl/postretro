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
- **Player options / settings persistence / mouse sensitivity / invert-Y / view_feel_scale** → `player_options.md`
- **UI layer / HUD / widgets / theming / UI state binding** → `ui.md`
- **Resource management / textures / materials** → `resource_management.md`
- **Scripting / primitives / SDK types** → `scripting.md`
- **Netcode / multiplayer / co-op / replication / transport / wire format** → `networking.md`
- **Game / mod author docs (human-facing, not agent context)** → `docs/`
- **Collision (world/entity)** → `entity_model.md` §7
- **Navigation / navmesh / pathfinding representation** → `build_pipeline.md` §Navigation bake
- **Player movement / movement states / FPS feel** → `movement.md`
- **Frame timing / game loop** → `rendering_pipeline.md` §1 · `entity_model.md` §5
- **Boot / startup / splash / level-load sequence / mod loading** → `boot_sequence.md`
- **Roadmap / implementation phases** → `plans/roadmap.md`
- **Experimental spikes / build-to-learn specs** → `experimental_spikes.md`
- **Draft plans / future features** → `plans/drafts/`
- **Ready plans (reviewed, awaiting implementation)** → `plans/ready/` — promoted out of drafts after review; current design intent.
- **Shipped plans** → `plans/done/` — historical record, frozen at ship time. May describe stale state. Read only when explicitly referenced.
- **Research archive** → `research/` — past research, not current design. Do not read unless explicitly instructed. See also: `research/weapon-model.md` for weapon-model / weapon-instance design intent; `research/combat-events.md` for the on-hit / on-kill combat-event substrate (XP, scoring, kill credit, resource economy) design intent.
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
| **Baked over computed** | Spatial data and indirect lighting are baked offline; visibility computes per frame from baked portal geometry (id Tech 4 lineage; portal traversal is the sole visibility path). Direct light may be baked (static lightmaps; baked layers for movers) or evaluated at runtime — whether a light is authored static (baked) or dynamic (runtime) is an **authoring choice, not an engine rule**. The one engine invariant: a physical light's contribution must never be **double-counted on a given receiver** — overlapping static and dynamic light must not over-brighten the same fragment. Lighting techniques compose additively in the forward pass. |
| **Subsystem boundaries** | Renderer, audio, input, game logic are distinct modules with explicit contracts. |
| **Frame ordering** | Input → Game logic → Audio → Render → Present. Later stages depend on earlier ones. |
| **No `unsafe`** | The crate stack provides safe APIs. If `unsafe` appears necessary, stop and consult the project owner. |
| **Primitive surface is a contract** | Engine parameters exposed as scripting primitives carry API contracts. Changing semantics, valid ranges, or clamping behavior requires updating the scripting surface — SDK types, validation rules, and reaction constructors — in the same pass. |

---

## 3. Baked Data Strategy

Single authoring pipeline: TrenchBroom `.map` → `prl-build` → `.prl`. Engine loads `.prl` as the sole runtime map format.

prl-build uses a BSP tree as a compiler intermediate to produce cells, portal geometry, and per-cell draw chunks. The runtime consumes cells and portals; it does not walk BSP nodes for rendering or visibility. (`BspNodes`/`BspLeaves` sections are still emitted for camera-leaf lookup — replacing that with a cell-location section is a future step.) Portal traversal is the sole visibility path; the runtime falls back to per-leaf AABB frustum culling for solid-leaf, exterior-camera, and no-portals cases. Designed to subsume all baked data in engine-native coordinates. See `build_pipeline.md`.

### PRL baked data

| Data | Source |
|------|--------|
| Geometry | prl-build (brush-volume BSP → brush-side projection → pack) |
| BSP tree | prl-build (compile-time scaffolding; BspNodes/BspLeaves used for camera-leaf lookup) |
| Visibility | prl-build (portal generation — runtime traverses portal graph each frame) |
| Light entities | FGD entities parsed and translated to canonical format at compile time |
| Indirect lighting | SH L2 irradiance volume baked from canonical lights |
| Fog volumes | FGD brush entities resolved to BSP leaves at load time |
| Acoustic zones | FGD brush entities resolved to BSP leaves at load time |
| Reflection probes | FGD point entities → baked cubemaps |

Full detail (section inventory, SectionId registry): `build_pipeline.md`.

---

## 4. Non-Goals

- General-purpose game engine
- General-purpose / extensible ECS framework — archetype storage, query planner, system scheduler, modder-defined component types. Internal storage *is* data-oriented (dense per-kind component columns); the component *vocabulary* is engine-closed. See `entity_model.md` §1.
- Deferred rendering
- Runtime level compilation
- General-purpose multiplayer — deterministic lockstep / rollback, competitive PvP, matchmaking, anti-cheat, peer-to-peer topologies, full server-rewind lag compensation. Authoritative client-server **co-op** is in scope: see Milestone 15 (`plans/roadmap.md`); design in `context/research/netcode/`.
