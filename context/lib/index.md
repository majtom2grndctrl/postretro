# Postretro – Architecture Index

> **Use as a router:** pick 2–3 linked docs for the task, don't load everything.
> **Source of truth for:** product definition, architectural principles, and where contracts live.
> **Not for:** implementation details (load the specific doc instead).
> **Pre-stable note:** refactors may introduce breaking changes; update all call sites and related tests in the same change.

## Agent Router (Task → Minimal Docs)

- **Engineering conventions / code style** → `development_guide.md`
- **Crate layering / where new code goes / dependency direction** → `development_guide.md` §Workspace
- **Build and run commands / standard build configuration / which cargo profile and features to use** → `development_guide.md` §Build and run
- **Worktree builds / where a worktree's `target/` goes / disk budget for parallel builds / A/B binaries from another commit** → `development_guide.md` §Worktree builds
- **Crate dependency graph / blast radius / what depends on X** → `crate-graph.md` (generated); live queries via `cargo run -p xtask -- crate-graph --rdeps <crate>`
- **Context file writing / updates** → `context_style_guide.md`
- **Testing** → `testing_guide.md`
- **Asserting on log output / log capture in tests** → `testing_guide.md` §3 · entry point `crates/test-log-capture`
- **Rendering pipeline / lighting** → `rendering_pipeline.md`
- **SH probe streaming / cluster residency / warm set / read scheduling / streaming diagnostics** → `rendering_pipeline.md` §Cluster SH residency · ids 46/49/50: `build_pipeline.md` §PRL section IDs
- **Lightmap/shadowmask residency / cell blocks / lightmap pool / vertex block table / lightmap miss policy / `POSTRETRO_LIGHTMAP_STREAMING` / shared SH+lightmap read issuer and drain budget** → `rendering_pipeline.md` §4 (Lightmap cell-block residency) · ids 22/42/51: `build_pipeline.md` §PRL section IDs
- **Frame capture / offscreen readback / headless (surfaceless) rendering** → `rendering_pipeline.md` §7.8
- **Render resolution / HiDPI / scene extent vs surface extent / integer upscale / native UI layer / covers-HUD effect switches** → `rendering_pipeline.md` §7.8 · `player_options.md` §4 · `ui.md` §5
- **Projectile visuals / emissive billboards / flipbook sprite bodies / mover-attached dynamic lights / impact-flash light / animated light radius** → `rendering_pipeline.md` §4, §7.4 · `resource_management.md` §6
- **PRL format / level compiler / runtime portal vis** → `build_pipeline.md` §PRL Compilation
- **Portal walk cost bound / bounded-region walk / visible-set superset contract (what consumers may assume)** → `rendering_pipeline.md` §2
- **Cell→cell coupling relation / baked cell-visibility substrate / network relevance, audio occlusion, AI-perception broad phase, or VFX cull shared foundation** → `build_pipeline.md` §PRL section IDs
- **Brush roles / which brushes participate in the BSP** → `build_pipeline.md` §Compiler pipeline
- **Bake stage ordering / what a compiler stage may depend on / where atlas preparation runs** → `build_pipeline.md` §Compiler pipeline
- **Build cache / warm vs cold builds / why a stage re-baked / cache disk footprint / stage epochs and keys** → `build_pipeline.md` §Build Cache
- **Audio / spatial sound / positional sound events / descriptor sound fields / reverb zones** → `audio.md` · emitter token: `scripting.md` §12
- **Entity model / game objects / sprites** → `entity_model.md`
- **Enemy AI / behavior state graph / transition guards / brain component** → `entity_model.md` §7c · `scripting.md` §11
- **Hierarchical enemy behavior / statecharts / nested activities / layers / committed attack phases** → `entity_model.md` §7c · `scripting.md` §11
- **Build pipeline / FGD / TrenchBroom** → `build_pipeline.md`
- **Distribution / packaging a runnable build / `dist` payload layout / launcher / `.dist-incomplete` / which levels ship** → `build_pipeline.md` §Distribution packaging
- **Modder SDK bundle / shipping the content tools / `prl-build` + `scripts-build` in a distribution / `sdk-dist`** → `build_pipeline.md` §Distribution packaging (§SDK bundle)
- **`postretro-tool` / `postretro.toml` project marker / project discovery / helper-binary resolution / why `xtask` cannot ship** → `build_pipeline.md` §Distribution packaging (§The project marker, §Why the tool is not xtask)
- **Player-data directory / where `settings.toml` and `state.json` live / `--app-name` / per-game user directories** → `build_pipeline.md` §Distribution packaging (§Player-data directory) · `player_options.md` §2
- **Authoring launch against a project / `postretro-tool run` / `--core-root` / `--install-root` / who supplies `--baked-root` and `--cache-dir`** → `build_pipeline.md` §Distribution packaging (§Authoring launch) · `ui.md` §5 · `docs/external-projects.md` (author-facing)
- **Where `.prm` sidecars live / materials-root derivation / mod root shape / `--baked-root` / game content in a repo outside the engine install** → `build_pipeline.md` §Baked texture mips
- **Input format adapters / adding a new map source format / what Quake or TrenchBroom vocabulary may cross into shared compiler stages** → `build_pipeline.md` §Source-format neutrality
- **Input handling / gamepad** → `input.md`
- **Remapping / rebinding / key bindings / author default bindings / command relevance / tap-hold activators / UI nav from the binding table / console menu conventions (restore focus, hold-to-repeat, tabs, scroll, glyphs)** → `player_options.md` §6 · `input.md` §2, §5, §7 · `ui.md` §4
- **Player options / settings persistence / mouse sensitivity / invert-Y / view_feel_scale** → `player_options.md`
- **Window modes / fullscreen / exclusive display mode / mode confirm / `--windowed`** → `player_options.md` §7 · `boot_sequence.md` §1 (Window mode) · `ui.md` §4.1
- **Accessibility preferences / OS preference seeding / `accessibility.*` slots / reduce motion / per-field settings fallback** → `player_options.md` §5, §2
- **Accessibility panel / `ui.openAccessibility` / accessibility field actions / missing-entry warning / first-launch panel hold** → `ui.md` §4.1 · `input.md` §5 · `boot_sequence.md` §First-launch hold
- **Animated lightmap compact atlas / block table / section 25 paging / lightmap-family memory meter** → `rendering_pipeline.md` §7.1 (Animated lightmap compose), §7.8 (Lightmap-family byte meter) · format: `build_pipeline.md` §PRL section IDs
- **Photosensitivity / flash limiter / strobe safety** → `rendering_pipeline.md` §7.8 (Photosensitivity limiter)
- **Screen reader / assistive technology / accessibility snapshot / hidden-window adapter boot** → `ui.md` §4.2 · `boot_sequence.md` §Window visibility
- **Theme variants / high contrast / text scale / contrast diagnostic / focus visuals** → `ui.md` §1, §2
- **Captions / subtitles / sound-direction cues / mono audio / bus volume options** → `audio.md` §1 (Mixer bus tree), §5
- **Damage direction indicator / player damage bearing** → `networking.md` §Presentation events vs. replicated state
- **UI layer / HUD / widgets / theming / UI state binding** → `ui.md`
- **Engine-owned assets / `core/` tree / built-in UI descriptors / splash image / where engine assets live vs. mod content** → `ui.md` §5 · `build_pipeline.md` §Distribution packaging
- **Resource management / textures / materials** → `resource_management.md`
- **Surface Depth / height maps / `_h.png` / texel-space parallax / per-texel surface carve / two-channel surface map** → `resource_management.md` §4.6 · `rendering_pipeline.md` §7.3 · bake and `.prm` format: `build_pipeline.md` §Baked texture mips
- **Texture memory accounting / per-slot or per-mip byte cost / what a level's textures cost** → `build_pipeline.md` §Baked texture mips (Byte accounting)
- **3D model / glTF import (scale, pivot, material format)** → `resource_management.md` §7
- **Scripting / primitives / SDK types / scripting crate boundaries / VM compile firewall** → `scripting.md`
- **Reaction dispatch model / event sources / dispatch scopes / reaction parameters / occupancy exposure** → `scripting.md` §12
- **Entity addressing / map members (`getMapEntities`) vs. groups (`npcs`, `players`) vs. subject tokens / per-member `.on` sources / spawned-NPC tags** → `scripting.md` §12 (Entity addressing)
- **Netcode / multiplayer / co-op / replication / transport / wire format** → `networking.md`
- **Live introspection channel / observe-live / localhost debug socket / reading a running session's state over a socket** → `networking.md` §Not netcode: the live introspection channel
- **Joining a session / admission vs content parity / slot lifecycle / host level change / what gates vs what replicates** → `networking.md` §Admission and content parity · §Slot lifecycle · §What gates, and what replicates instead
- **First-person weapon placement / viewmodel offset / where a weapon sits in view / placement vs view-feel / FP vs TP weapon vantage** → `networking.md` §Weapon placement is content
- **Projectile fire origin / muzzle point / where a shot spawns / camera-eye vs barrel** → `networking.md` §Weapon placement is content (Fire origin composes on placement)
- **Weapon descriptor vocabulary / one canonical weapon type / no player-vs-enemy weapon kind / wield restriction as attribute** → `entity_model.md` §Components (Weapon vocabulary)
- **Weapon resources / ammo vs heat vs cell / overheat lockout / cell regen / holstered weapons cooling / weapon-resource HUD slots** → `entity_model.md` §Components (Weapon resources) · connected-client prediction: `networking.md` §Combat authority
- **Dynamic weapon accuracy / spread / bloom / recoil-as-accuracy-loss / movement inaccuracy / crosshair spread ring** → `entity_model.md` §Components (Weapon vocabulary — Dynamic accuracy) · `networking.md` §engine-randomness carve-out
- **Splash / AoE / area damage / blast radius / distance falloff / rocket explosion** → `entity_model.md` §Components (Splash / area effects)
- **Knockback / hit impulses / rocket jumping** → `entity_model.md` §Components (Knockback) · `movement.md` §6 · `networking.md` §Game-logic-owned apply invariant
- **Game / mod author docs (human-facing, not agent context)** → `docs/` — every command there must be runnable from an SDK bundle alone; `cargo run -p xtask -- …` never appears in it
- **Game content in an author's own repository, outside an engine install (human-facing walkthrough)** → `docs/external-projects.md`; the contract behind it is `build_pipeline.md` §Baked texture mips and §Distribution packaging
- **Collision (world/entity)** → `entity_model.md` §7
- **Radial entity query / one-to-many overlap / all entities within radius / non-ray query family** → `entity_model.md` §7 (Radial entity overlap)
- **Navigation / navmesh / pathfinding representation** → `build_pipeline.md` §Navigation bake
- **Multi-threaded AI pathfinding readiness / scheduling blocker** → `ai_pathfinding_mt_readiness.md`
- **Player movement / movement states / FPS feel / slide camera dip and FOV** → `movement.md`
- **Frame timing / game loop** → `rendering_pipeline.md` §1 · `entity_model.md` §5
- **CPU profiling / per-stage CPU timing / Tracy / GPU pass timing / Metal System Trace when timestamps are unsupported / Mac perf measurement confounders / diagnostic env vars vs features** → `rendering_pipeline.md` §12 · `development_guide.md` §6.4
- **Boot / startup / splash / level-load sequence / mod loading** → `boot_sequence.md`
- **Experimental spikes / build-to-learn specs** → `experimental_spikes.md`
- **3rd party library docs** → use `context7` tool (wgpu, winit, kira, glam).

---

## 1. Product Definition

**Retro-inspired FPS engine** — a hybrid of new and old. Doom/Quake boomer shooter with a cyberpunk aesthetic. Monster closets and scripted reveals are first-class set-pieces rather than engine-fighting workarounds, making for theatrical gameplay experiences. Inspired by retro look and feel but game design is a meaningful iteration beyond games of the period.

**Aesthetic:** Low-poly 3D environments + blocky pixelated textures; with modern embellishments like baked volumetric indirect lighting (SH irradiance volumes), normal-mapped surfaces, dynamic direct lighting, and billboard sprite volumetrics that react to light.

**Architectural northstar:** Lean, wgpu-driven pipeline — not a resource heavy modern engine with retro filters. Near-instant boot, tiny binary, and _some_ retro filters, but used sparingly.

### 1.1 Targets and altitude

Design inputs for every renderer, baker, and netcode decision, not stretch goals.

- **Pre-RTX.** No dependence on hardware ray-tracing APIs. Lightweight ray work is in bounds when bounded and either offline in the baker or a cheap runtime trace against baked structures (BVH, SDF, probe volumes). Mac/Metal is a perf target.
- **Ambitious perf inside that era.** Bake over compute. Bound every bake and pass to where the player can be and see: no probes or texels in the void outside the hull, no bake rays through solid geometry or past a light's reach. Measure, don't assume.
- **Smooth PvE co-op.** Prediction, reconciliation, and interpolation done well, validated at realistic latency. PvP, live service, and full lag compensation stay non-goals (§4).
- **Altitude.** Destination clear → build the full shape in strides. Cut breadth before correctness or performance (`development_guide.md` §1.3).

---

## 2. Architectural Principles

| Principle | Invariant |
|-----------|-----------|
| **Renderer owns GPU** | All wgpu calls live in the renderer module. Other subsystems never touch wgpu types. |
| **Baked over computed** | Spatial data and indirect lighting are baked offline; portal traversal normally computes visibility per frame from baked portal geometry. Defined fallback cases use per-cell AABB frustum culling; an over-budget walk falls back too, bounded to frustum-culled sets that drive drawing and fog reach. Direct light may be baked (static lightmaps; baked layers for movers) or evaluated at runtime — whether a light is authored static (baked) or dynamic (runtime) is an **authoring choice, not an engine rule**. The one engine invariant: a physical light's contribution must never be **double-counted on a given receiver** — overlapping static and dynamic light must not over-brighten the same fragment. Lighting techniques compose additively in the forward pass. |
| **Subsystem boundaries** | Renderer, audio, input, game logic are distinct modules with explicit contracts. |
| **Frame ordering** | Input → Game logic → Audio → Render → Present. Later stages depend on earlier ones. |
| **No `unsafe`** | The crate stack provides safe APIs. If `unsafe` appears necessary, stop and consult the project owner. |
| **Primitive surface is a contract** | Engine parameters exposed as scripting primitives carry API contracts. Changing semantics, valid ranges, or clamping behavior requires updating the scripting surface — SDK types, validation rules, and reaction constructors — in the same pass. |

---

## 3. Baked Data Strategy

Single authoring pipeline today: TrenchBroom `.map` → `prl-build` → `.prl`. Engine loads `.prl` as the sole runtime map format. One input format is a content decision, not an architectural one — the compiler's `format/` adapter translates source vocabulary to canonical engine terms so a second front end can target PRL without touching a shared stage. See `build_pipeline.md` §Source-format neutrality.

prl-build uses a BSP tree as a compiler intermediate to produce cells, portal geometry, and per-cell draw chunks. The runtime consumes cells, a cell locator, portals, and BVH arrays; it does not load or walk BSP nodes for rendering or visibility. Portal traversal normally computes visibility; solid-cell, exterior-camera, and no-portals cases fall back to per-cell AABB frustum culling, as does an over-budget walk — bounded frustum-culled sets, one for drawing and one for fog reach. Designed to subsume all baked data in engine-native coordinates. See `build_pipeline.md`.

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
