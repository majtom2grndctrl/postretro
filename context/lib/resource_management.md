# Resource Management

> **Read this when:** loading textures, working with materials, adding billboard sprites, or changing how the engine consumes visual assets.
> **Key invariant:** all visual assets are PNGs loaded at runtime. PRL stores texture names only, never pixel data. The resource subsystem owns all textures and GPU resources; the renderer borrows handles.
> **Related:** [Architecture Index](./index.md) · [Build Pipeline](./build_pipeline.md) · [Rendering Pipeline](./rendering_pipeline.md) · [Entity Model](./entity_model.md)

---

## 1. Texture Pipeline

All visual assets — world textures, billboard sprites, UI elements — are authored as PNG files and loaded at runtime. No WAD files. No embedded texture data in PRL.

### 1.1 Authoring Layout

Textures live under `content/<mod>/textures/` (where `<mod>` is `base` for first-party content or `tests` for engine test fixtures) with one required subdirectory level:

```
content/<mod>/textures/<collection>/<name>.png
```

Example paths: `content/base/textures/concrete/wall.png`, `content/tests/textures/metal/panel.png`.

TrenchBroom requires the collection subdirectory structure for texture browsing. Collections group related textures (e.g., `concrete/`, `metal/`, `trim/`). At runtime, the engine derives the texture root from the loaded map's path: given a map at `content/<mod>/maps/level.prl`, the content root is `content/<mod>/`, and textures resolve from `content/<mod>/textures/`.

### 1.2 PRL Texture References

PRL files store a deduplicated texture name list (TextureNames section). No pixel data. At load time, the engine matches texture name strings from PRL face data against PNG filenames in the `textures/` tree. For each loaded diffuse, the loader also probes for `{name}_s.png` (specular) and `{name}_n.png` (normal-map) siblings in the same collection directory; missing or invalid siblings fall back to shared 1×1 placeholders (see §4). Missing diffuse at runtime falls back to a checkerboard placeholder and logs a warning; sibling probes are skipped when the diffuse itself is a placeholder.

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

World textures use individual bind groups — one per unique material. Draw calls batch by material to minimize bind group switches. No atlas; atlas packing is an unscheduled optimization.

### 2.1 SH Irradiance Volume (Milestone 5)

> **Not yet implemented.**

Indirect lighting is carried by an SH L2 irradiance volume (3D probe grid), not by per-face lightmaps. The probe section is loaded from PRL and sampled trilinearly per fragment in the world shader. See `rendering_pipeline.md` §4.

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
| **Emissive flag** | Implemented — flag on enum variant. Rendering bypass planned. |
| **Shininess** | Implemented (Milestone 5) — specular exponent on enum variant. |
| **Footstep sounds** | Planned. |
| **Bullet impact particles** | Planned. |
| **Ricochet behavior** | Planned. |
| **Decal selection** | Planned. |
| **Environment-mapped reflections** | Planned. See §5. |

Each behavior is a property of the material enum variant. Which prefixes carry which flags is a content concern — the engine provides the mechanism.

Mappers use the naming convention; no special tooling or workflow required on the authoring side.

Unknown prefix maps to a default material. Engine logs a warning at load time identifying the unrecognized prefix and the texture name.

---

## 4. Surface Map Convention

Optional sibling textures provide per-texel surface properties. Suffixes are appended to the diffuse texture name (e.g., `wall.png` → `wall_s.png`).

### 4.1 Specular Maps (Milestone 5)

Per-texel specular intensity modulates the direct lighting highlight.

- **Naming:** `{name}_s.png` suffix.
- **Format:** R8Unorm (sampled as `.r` in shader).
- **Color Space:** Linear.
- **Dimensions:** Must match the diffuse texture.
- **Fallback:** If `{name}_s.png` is absent or dimensions mismatch, the engine uses a shared 1x1 black texture (zero specular response). Mismatch logs a warning at load time.

### 4.2 Generation Tool

`tools/gen_specular.py` generates specular maps from diffuse textures using material-prefix heuristics.

- **Heuristics:** `metal_` (high intensity, low gamma), `concrete_` (low intensity, high gamma), `wood_` (moderate).
- **Dependency:** Requires `Pillow`, managed via `uv`.
- **Setup:** `uv venv && source .venv/bin/activate && uv pip install Pillow`.
- **Usage:** `python3 tools/gen_specular.py --input <path> --recursive`.
- **Linear-output guarantee:** outputs are written without `sRGB`, `gAMA`, or `iCCP` PNG chunks, so they pass `prl-build`'s linear color-space validation (see §4.1, §4.3). The script strips any color-management metadata that Pillow would otherwise carry forward from the diffuse source.

### 4.3 Normal Maps (Phase 5+)

Optional per-texture normal maps for fine surface detail.

- **Naming:** `{name}_n.png` suffix alongside the diffuse texture.
- **Format:** `Rgba8Unorm` (linear). Must not be sRGB-tagged — prl-build rejects sRGB-tagged siblings at compile time.
- **Encoding:** Tangent-space RGB. Decode: `n = sample.rgb * 2.0 - 1.0`. Dimensions must match the diffuse texture.
- **Placeholder:** Shared 1×1 neutral-normal texture encoding `(127, 127, 255)` — decodes to approximately `(0, 0, 1)` (tangent-space +Z; exact value `(-0.004, -0.004, 1.0)`). Engine-lifetime; survives level unload. Used when `_n.png` is absent, dimensions mismatch (logs a warning), or decode fails (logs an error).
- **Fallback:** Missing or invalid sibling falls back to the placeholder silently except for the log entry. Flat mesh-normal shading is preserved — the placeholder is a true no-op through the TBN path.

### 4.4 Normal Map Generation Tool

`tools/gen_normal.py` generates `_n.png` siblings from diffuse textures via Sobel filtering on luminance. Material-prefix strength heuristics: `metal_` (1.5), `stone_` (1.2), `concrete_`/`wood_` (1.0), `plaster_` (0.8).

- **Dependencies:** `Pillow` + `numpy`. Managed via `uv`.
- **Setup:** `uv venv && source .venv/bin/activate && uv pip install Pillow numpy`.
- **Usage:** `python3 tools/gen_normal.py --input <path> --recursive`.
- **Fallback:** without `numpy`, emits flat `(127, 127, 255)` maps with no surface detail.
- **Linear guarantee:** no `sRGB`, `gAMA`, or `iCCP` chunks — passes `prl-build` validation.

---

## 5. Cubemap Handling

### 5.1 Entity Format

`env_cubemap` is a point entity placed in TrenchBroom. It marks a position where a cubemap should be baked. Properties:

| Property | Description |
|----------|-------------|
| origin | Bake position (inherited from entity placement) |
| size | Resolution per face in pixels (default: 256) |

### 5.2 Bake Pipeline

> **Not yet implemented.** Entity format defined; bake tool deferred.

A separate offline tool bakes one cubemap per `env_cubemap` entity position. Baked output lives alongside the map file. See `build_pipeline.md` when this is planned.

### 5.3 Runtime Consumption

Reflective surfaces (wet floors, chrome, glass) sample from the nearest `env_cubemap` probe by world-space distance.

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
