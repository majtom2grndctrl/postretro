I'm exploring building a lean, retro-style FPS engine in Rust. This is a hobby/experimental project — it may never ship, but I want the architecture to be solid enough to prototype with.

**The vision:** A Doom/Quake-style boomer shooter with a cyberpunk aesthetic. Low-poly 3D environments with baked lightmaps, billboard sprite characters, and a few modern embellishments (dynamic colored lights, bloom, particles). Think Prodeus's visual targets achieved through genuinely retro technology rather than a modern engine with retro filters. The engine should boot nearly instantly and produce a tiny binary.

**The stack we've settled on:**
- **Rust** as the language
- **winit** for windowing
- **wgpu** for GPU access (Vulkan, Metal, DX12 — no direct OpenGL dependency)
- **kira** for audio
- **gilrs** for gamepad input
- **glam** for math
- **qbsp** for loading Quake-format BSP files at runtime
- **TrenchBroom** for level design (external tool)
- **ericw-tools** for BSP compilation (offline build step)

**The renderer should support:**
- BSP tree traversal for visibility and front-to-back rendering
- Baked lighting from ericw-tools consumed via BSPX lumps:
  - **Colored lightmaps** (RGB, standard `RGBLIGHTING` lump) sampled as a second texture per face
  - **Directional lightmaps** (`LIGHTINGDIR` lump via `-bspxlux`) storing dominant light direction per texel, enabling per-pixel specular on wet streets, metal, chrome — cheap and fully baked
  - **Ambient occlusion** (baked via ericw-tools `-dirt` flag) for depth in corners, recesses, and contact edges
  - **Volumetric light probes** (`LIGHTGRID_OCTREE` lump via `-lightgrid`) for consistently lighting dynamic objects — billboard sprites, particles, the player's weapon — so they don't look flat against the baked world
- Billboard sprites for characters/enemies (camera-facing textured quads), lit by sampling the nearest light probe
- A small number of dynamic point lights (muzzle flash, neon signs, explosions) via forward rendering — these supplement the baked lighting, not replace it. wgpu's explicit render pipeline makes multi-pass forward lighting straightforward
- Emissive textures (fullbright surfaces for neon, screens, indicators)
- Post-processing pass for bloom, fog, and optional CRT/scanline effects
- Custom vertex format: position, base texture UV, lightmap UV, vertex color (defined as a wgpu `VertexBufferLayout`)

**Baked data strategy — using ericw-tools to the fullest without extending it:**

We're building a custom BSP renderer, which means we control what data we consume and how. Rather than extending ericw-tools (C++, GPLv2), we lean on its existing BSPX output and supplement with authored metadata through TrenchBroom's entity system.

*Computed by ericw-tools (mapper gets this for free):*
- Colored lightmaps, directional lightmaps, AO/dirt, and volumetric light probes — all enabled by compile flags, no mapper effort required
- Build flags: `-bspxlux` (directional lightmaps), `-dirt` (AO), `-lightgrid` (volumetric probes), plus standard RGB lighting

*Derived from texture naming conventions (implicit authoring):*
- **Surface material types** — texture name prefixes map to material enums (`metal_floor_01` → Metal, `concrete_wall_03` → Concrete, `grate_rusty_02` → Grate). Drives footstep sounds, bullet impact particles, ricochet behavior, decal selection. Lookup table maintained alongside the engine, no special mapper workflow.

*Authored via custom FGD entities (placed by mapper in TrenchBroom):*
- **`env_fog_volume`** (brush entity) — defines a region with fog `color`, `density`, `falloff`. Resolved to BSP leaves at load time for per-leaf atmospheric haze (colored smog at street level, clear air on rooftops).
- **`env_cubemap`** (point entity) — marks where to bake reflection probes. At build time, renders a cubemap from that position. Used for cheap environment-mapped reflections on wet floors, chrome, glass.
- **`env_reverb_zone`** (brush entity) — defines acoustic properties (`reverb_type`, `decay_time`, `occlusion_factor`). Resolved to BSP leaves so the audio system can spatially vary reverb and occlusion.

The FGD file defining these entities is a project deliverable alongside the engine.

**What I'd like you to produce:**
A technical spec document covering the engine architecture. This should include:
1. Crate/module structure and responsibility boundaries
2. The rendering pipeline in detail (what happens each frame, in order), including how each BSPX lump is consumed (directional lightmaps → specular, lightgrid → sprite/particle lighting, AO → occlusion modulation)
3. BSP loading pipeline (how qbsp data and BSPX lumps map to wgpu buffers and engine data structures)
4. The resource management approach (textures, meshes, materials, cubemaps)
5. Input handling architecture
6. Game loop structure (fixed timestep vs variable, how frames are scheduled)
7. A proposed entity/game object model (keeping it simple — this isn't an ECS engine)
8. Audio integration approach, including how `env_reverb_zone` data drives spatial audio
9. The build pipeline (TrenchBroom → ericw-tools → .bsp → game loads it), including specific ericw-tools flags and the role of the FGD file
10. Surface material derivation scheme (texture name → material enum → gameplay feedback)
11. FGD design for custom entities (`env_fog_volume`, `env_cubemap`, `env_reverb_zone`)
12. A phased implementation roadmap starting from "empty window" to "walking through a lit BSP level with billboard sprites"

Keep the spec grounded in what a solo hobbyist can realistically build incrementally. Each phase should produce something visible and testable. Don't over-engineer — this is a retro shooter, not a general purpose engine.

**When this prompt is fully executed**, move this file to the reference folder:
```bash
git mv context/lib/initial-prompt.md context/reference/initial-prompt.md
```
