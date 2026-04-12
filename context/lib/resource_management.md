# Resource Management

> **Read this when:** loading textures, working with materials, adding billboard sprites, or changing how the engine consumes visual assets.
> **Key invariant:** all visual assets are PNGs loaded at runtime. PRL stores texture names only, never pixel data. The resource subsystem owns all textures and GPU resources; the renderer borrows handles.
> **Related:** [Architecture Index](./index.md) · [Build Pipeline](./build_pipeline.md) · [Rendering Pipeline](./rendering_pipeline.md) · [Entity Model](./entity_model.md)

---

## 1. Texture Pipeline

All visual assets — world textures, billboard sprites, UI elements — are authored as PNG files and loaded at runtime. No WAD files. No embedded texture data in PRL.

### 1.1 Authoring Layout

Textures live under `textures/` with one required subdirectory level:

```
textures/<collection>/<name>.png
```

TrenchBroom requires this subdirectory structure for texture browsing. Collections group related textures (e.g., `textures/concrete/`, `textures/metal/`, `textures/trim/`).

### 1.2 PRL Texture References

PRL files store a deduplicated texture name list (TextureNames section). No pixel data. At load time, the engine matches texture name strings from PRL face data against PNG filenames in the `textures/` tree. Missing texture at runtime falls back to a checkerboard placeholder and logs a warning.

### 1.3 Sprite Animations

Animated sprites use sequentially-named frames within a collection directory:

```
textures/explosions/fireball_00.png
textures/explosions/fireball_01.png
textures/explosions/fireball_02.png
```

Frame ordering derives from the numeric suffix. Playback rate is defined by the entity or particle system consuming the animation, not by the texture data.

---

## 2. Texture Binding

World textures are currently bound individually — one bind group per unique texture. Draw calls are grouped by (leaf, texture) to minimize bind group changes without allocating an atlas.

### 2.1 Lightmap Atlas

> **Phase 4/5+. Not yet implemented.**

Per-face lightmap data packed into a shared atlas. Renderer references atlas regions via UV coordinates baked into the vertex buffer. One atlas per level; dimensions determined at load time from total lightmap area.

### 2.2 World Texture Atlas

> **Unscheduled optimization.**

Pack world textures into a shared atlas for further draw-call reduction. Same UV-remapping approach. Textures exceeding atlas capacity fall back to individual binds.

---

## 3. Material System

Texture name prefix determines material type. The engine derives a material enum from the first token of the texture name (delimited by `_`).

The engine provides the mechanism: prefix lookup, material enum, and per-material behavior hooks (footstep sounds, impact effects, decals). Which prefixes exist and what they map to is a game content concern — the prefix table grows as content requires it. The engine does not aim for a complete material table; it aims to make adding new materials trivial.

Example prefixes (illustrative, not exhaustive):

| Prefix | Material |
|--------|----------|
| `metal` | Metal |
| `concrete` | Concrete |
| `grate` | Grate |

The material enum and prefix derivation are implemented. Behavior hooks are planned for later phases:

| Behavior | Status |
|----------|--------|
| **Emissive flag** | Flag set on enum variant. Rendering bypass is Phase 5+. |
| **Footstep sounds** | Phase 4. |
| **Bullet impact particles** | Phase 5+. |
| **Ricochet behavior** | Phase 5+. |
| **Decal selection** | Phase 5+. |
| **Environment-mapped reflections** | Phase 5+. See §5. |

Each behavior is a property of the material enum variant. Which prefixes carry which flags is a content concern — the engine provides the mechanism.

Mappers use the naming convention; no special tooling or workflow required on the authoring side.

Unknown prefix maps to a default material. Engine logs a warning at load time identifying the unrecognized prefix and the texture name.

---

## 4. Normal Maps

> **Phase 5+. Not yet implemented.**

Optional per-texture normal maps for fine surface detail. Convention: `_n` suffix alongside the diffuse texture (`floor_01.png` / `floor_01_n.png`). Absence is the common case — no warning. Tangent-space RGB PNGs matching the diffuse dimensions.

When implemented, normal maps combine with LIGHTINGDIR data for view-dependent specular highlights. See `rendering_pipeline.md` §7.1.

---

## 5. Cubemap Handling

### 5.1 Entity Format

`env_cubemap` is a point entity placed in TrenchBroom. It marks a position where a cubemap should be baked. Properties:

| Property | Description |
|----------|-------------|
| origin | Bake position (inherited from entity placement) |
| size | Resolution per face in pixels (default: 256) |

### 5.2 Bake Pipeline

A separate offline tool renders six axis-aligned views from each `env_cubemap` position, producing a cubemap texture per entity. The bake tool is out of initial scope — define the entity format now, build the tool later.

Baked cubemaps are stored as six-face PNG sets or a cross-layout PNG per probe, in a directory alongside the map file.

### 5.3 Runtime Consumption

At load time, cubemap PNGs are loaded and uploaded as cubemap textures. Surfaces that use environment-mapped reflections (wet floors, chrome, glass) sample from the nearest `env_cubemap` probe. Probe assignment is spatial — each reflective surface binds to the closest probe by world-space distance.

---

## 6. Billboard Sprites

Camera-facing textured quads used for characters, pickups, projectiles, and decorative elements.

### 6.1 Asset Format

Loaded from PNG through the same texture pipeline as world textures. Sprite sheets are not used — each frame is an individual PNG. Animated sprites follow the sequential naming convention described in section 1.3.

### 6.2 Lighting

Sprite lighting is per-sprite, not per-pixel. Lighting behavior and fallback paths are defined in `rendering_pipeline.md` §7.3.

---

## 7. Resource Ownership

The resource subsystem owns logical assets: loaded PNGs, atlas layouts, metadata. The renderer owns GPU-side resources: wgpu buffers, textures, samplers. At level load, the resource subsystem prepares CPU-side data and hands it to the renderer, which uploads to the GPU and returns opaque handles. Other subsystems borrow these handles — they never call wgpu directly.

### 7.1 Lifecycle

| Phase | Action |
|-------|--------|
| Level load | Parse PRL texture names. Load all referenced PNGs. Upload to GPU. Distribute handles. |
| Gameplay | Handles are stable. No allocation or deallocation during gameplay. |
| Level unload | Release all GPU resources. Drop all texture data. Handles become invalid. |

Resources are loaded once at level load and released on level unload. No incremental loading during gameplay. No reference counting — the level owns everything, and everything dies with the level.

### 7.2 Renderer Contract

Renderer receives opaque resource handles at level load. It uses these handles to bind textures and buffers during draw calls. Renderer never interprets raw texture data or manages GPU memory directly. If a handle is invalid (stale reference after level unload), the engine must prevent use — this is a logic error, not a recoverable condition.

---

## 8. Non-Goals

- **WAD file support.** All textures are PNGs. No Quake/Half-Life WAD import or export.
- **Runtime texture generation.** No render-to-texture for mirrors, portals, or security cameras.
- **Hot-reload.** Textures are loaded once per level. No file-watching or live refresh during gameplay.
- **Procedural textures.** No noise-based or shader-generated textures. All surfaces use authored PNGs.
- **Texture streaming / virtual textures.** All textures for a level are loaded upfront. No partial or on-demand loading.
- **Cubemap bake tool.** The entity format and runtime consumption path are defined. The offline bake tool is deferred.
