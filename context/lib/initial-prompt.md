I'm building a lean, retro-style FPS engine in Rust. This is a hobby/experimental project — it may never ship, but I want the architecture to be solid enough to push the boundaries of what we can accomplish with AI-assisted software engineering.

**The vision:** A Doom/Quake-style boomer shooter with a cyberpunk aesthetic. Low-poly 3D environments with baked lightmaps, billboard sprite characters, and a few modern embellishments (dynamic colored lights, bloom, particles). Think Prodeus's visual targets achieved through genuinely retro technology rather than a modern engine with retro filters. The engine should boot nearly instantly and produce a tiny binary.

**The stack we've settled on:**
- **Rust** as the language
- **winit** for windowing
- **wgpu** for GPU access (Vulkan, Metal, DX12 — no direct OpenGL dependency)
- **kira** for audio
- **gilrs** for gamepad input
- **glam** for math (pinned to 0.30 for qbsp compatibility)
- **qbsp** (0.14) for loading Quake-format BSP files at runtime
- **TrenchBroom** for level design (external tool)
- **ericw-tools** (2.0.0-alpha) for BSP compilation (offline build step)

**Target BSP format:** BSP2 (`qbsp -bsp2`). BSP2 removes BSP29's geometry limits (65K faces, 32K clipnodes, +/-32K coordinate range) with no downside for a custom engine. The qbsp Rust crate defaults to BSP2 and abstracts format differences — BSP29 files work transparently, but BSP2 is the compilation target.

**The renderer should support:**
- BSP tree traversal for visibility and front-to-back rendering
- Baked lighting from ericw-tools, consumed via BSPX lumps and lightmap data:
  - **Colored lightmaps** (`RGBLIGHTING` BSPX lump, written by `light -bspx`) sampled as a second texture per face. The qbsp crate parses this natively.
  - **Directional lightmaps** (`LIGHTINGDIR` BSPX lump, also written by `light -bspx`) storing dominant light direction per texel, enabling per-pixel specular on wet streets, metal, chrome — cheap and fully baked. The qbsp crate does not parse this natively — access via `get_unparsed("LIGHTINGDIR")`.
  - **Ambient occlusion** (baked into lightmap data via worldspawn key `_dirt 1`) for depth in corners, recesses, and contact edges. Not a separate lump — AO modulates the lightmap samples directly.
  - **Volumetric light probes** (`LIGHTGRID_OCTREE` BSPX lump via `light -lightgrid`) for consistently lighting dynamic objects — billboard sprites, particles, the player's weapon — so they don't look flat against the baked world. Note: `-lightgrid` was developed primarily for Quake 2 and is marked experimental. Verify it produces usable output for Q1 BSP2 maps early in development.
- Billboard sprites for characters/enemies (camera-facing textured quads), lit by sampling the nearest light probe
- A small number of dynamic point lights (muzzle flash, neon signs, explosions) via forward rendering — these supplement the baked lighting, not replace it. wgpu's explicit render pipeline makes multi-pass forward lighting straightforward
- Emissive textures (fullbright surfaces for neon, screens, indicators)
- Post-processing pass for bloom, fog, and optional CRT/scanline effects
- Custom vertex format: position, base texture UV, lightmap UV, vertex color (defined as a wgpu `VertexBufferLayout`)

**Asset pipeline — PNG across the board, no WAD files:**

TrenchBroom supports loose PNG textures via a custom game configuration, eliminating the need for WAD files entirely. ericw-tools 2.0.0-alpha reads PNGs for texture dimensions when `-notex` is set.

The full pipeline:
1. **Author textures** as PNG files in `textures/<collection>/<name>.png` (TrenchBroom requires exactly one subdirectory level)
2. **TrenchBroom** displays them via a custom `Postretro` game configuration that points at the textures directory
3. **ericw-tools** reads PNGs for dimensions only. qbsp auto-adds the map file's parent directory as a search path, so if `textures/` is alongside the `.map` file, no extra flags are needed: `qbsp -bsp2 -notex map.map`. For textures in a separate location, use `-path <dir>` to add search paths. The BSP stores texture headers (name + dimensions) with no pixel data.
4. **Engine** loads PNGs at runtime, matched by the texture name strings in the BSP

This applies to all visual assets: world textures, billboard sprites, UI elements. Sprite animations use sequentially-named frames within a collection directory.

A custom FGD file defining Postretro entities for TrenchBroom is a project deliverable alongside the engine.

**Baked data strategy — using ericw-tools to the fullest without extending it:**

We're building a custom BSP renderer, which means we control what data we consume and how. Rather than extending ericw-tools (C++, GPLv2), we lean on its existing BSPX output and supplement with authored metadata through TrenchBroom's entity system.

*Computed by ericw-tools (mapper gets this for free):*
- Colored lightmaps, directional lightmaps, AO/dirt, and volumetric light probes — all enabled by compile flags or worldspawn keys, no mapper effort required
- The build pipeline runs three tools in sequence: `qbsp` (compile geometry) -> `vis` (compute visibility) -> `light` (calculate lighting). Key `light` flags: `-bspx` (writes `RGBLIGHTING` + `LIGHTINGDIR` BSPX lumps), `-lightgrid` (writes `LIGHTGRID_OCTREE` BSPX lump). AO is enabled via worldspawn key `_dirt 1` (bakes directly into lightmap data, no separate lump).

*Derived from texture naming conventions (implicit authoring):*
- **Surface material types** — texture name prefixes map to material enums (`metal_floor_01` -> Metal, `concrete_wall_03` -> Concrete, `grate_rusty_02` -> Grate). Drives footstep sounds, bullet impact particles, ricochet behavior, decal selection. Lookup table maintained alongside the engine, no special mapper workflow.

*Authored via custom FGD entities (placed by mapper in TrenchBroom):*
- **`env_fog_volume`** (brush entity) — defines a region with fog `color`, `density`, `falloff`. Resolved to BSP leaves at load time for per-leaf atmospheric haze (colored smog at street level, clear air on rooftops).
- **`env_cubemap`** (point entity) — marks where to bake reflection probes. At build time, a separate tool renders a cubemap from that position. Used for cheap environment-mapped reflections on wet floors, chrome, glass. *Note: the cubemap bake tool is out of scope for the initial spec — define the entity format and how cubemaps are consumed at runtime. The bake tool is a separate task.*
- **`env_reverb_zone`** (brush entity) — defines acoustic properties (`reverb_type`, `decay_time`, `occlusion_factor`). Resolved to BSP leaves so the audio system can spatially vary reverb and occlusion.

**What I'd like you to produce:**

A set of architecture documents for the engine, written as **separate context files** in `context/lib/`. All context files must follow the writing style in `context/lib/context_style_guide.md` — direct, brief, durable prose; tables over paragraphs for mappings; no implementation detail that breaks when code is renamed.

Each file should cover topics that an agent working on that subsystem needs, without overloading context with unrelated information. The files should match the agent router in `context/lib/index.md`:

1. **`rendering_pipeline.md`** — The rendering pipeline in detail: what happens each frame (in order), how each BSPX lump is consumed (directional lightmaps -> specular, lightgrid -> sprite/particle lighting), BSP loading pipeline (how qbsp data and BSPX lumps map to wgpu buffers and engine data structures), the custom vertex format. Note that AO is baked into lightmap data (not a separate lump) and `LIGHTINGDIR` requires custom BSPX parsing via `get_unparsed()`.
2. **`resource_management.md`** — Resource management approach: textures (PNG loading, atlas packing), materials (texture name -> material enum derivation), sprites, cubemaps
3. **`input.md`** — Input handling architecture: keyboard/mouse via winit, gamepad via gilrs, input mapping
4. **`audio.md`** — Audio integration: kira setup, how `env_reverb_zone` data drives spatial audio, sound triggering from game events
5. **`build_pipeline.md`** — The build pipeline: TrenchBroom -> ericw-tools -> .bsp -> game loads it, specific ericw-tools flags, the FGD file design for custom entities (`env_fog_volume`, `env_cubemap`, `env_reverb_zone`), surface material derivation scheme, the custom TrenchBroom game configuration
6. **`entity_model.md`** — A proposed entity/game object model: how game objects are represented, updated, and interact with subsystems. Keep it simple — this isn't an ECS engine. Define the *framework* only; specific entity types (enemies, doors, pickups, triggers) are implementation tasks, not spec scope.

Additionally, produce:

7. **`context/plans/ready/wgpu-migration.md`** — Task to replace the current glow/glutin code with a wgpu backend (empty window with clear color). Note: the context docs already assume wgpu throughout; this task is about migrating the code in `src/main.rs` and `Cargo.toml`. This is the prerequisite for all other work.
8. **`context/plans/drafts/grounded-movement.md`** — Task for player controller with BSP collision, gravity, and ground detection.
9. **`context/plans/drafts/entity-types.md`** — Task to define and implement specific entity types (enemies, doors, pickups, triggers, projectiles).
10. **`context/plans/drafts/cubemap-bake-tool.md`** — Task to build the tool that renders cubemaps from `env_cubemap` positions.

Also update `context/lib/index.md` to reflect any changes to the agent router or architectural decisions.

**Game loop and roadmap:**

The spec should cover game loop structure (fixed timestep for game logic, variable framerate rendering with interpolation) in `rendering_pipeline.md`, since frame scheduling is tightly coupled with the render loop.

Include a **phased implementation roadmap** at `context/plans/roadmap.md`, starting from "wgpu window exists" through "free-fly camera in a lit BSP level with billboard sprites," followed by a phase for grounded player movement (BSP collision, gravity, ground detection). Each phase should produce something visible and testable.

The roadmap is neither fully durable (like architecture docs) nor fully ephemeral (like a single task). It's a medium-lived planning document — phases are checked off or removed as work progresses, but the document persists across many tasks. It should note this lifecycle at the top.

Keep the spec grounded in what a solo hobbyist can realistically build incrementally. Don't over-engineer — this is a retro shooter, not a general-purpose engine.

**When this prompt is fully executed**, move this file to the reference folder:
```bash
git mv context/lib/initial-prompt.md context/reference/initial-prompt.md
```
