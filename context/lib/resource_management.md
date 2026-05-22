# Resource Management

> **Read this when:** loading textures, working with materials, adding billboard sprites, or changing how the engine consumes visual assets.
> **Key invariant:** authored visual assets are PNGs. World-material textures are baked into per-texture `.prm` mip sidecars at compile time; UI textures load PNGs directly at runtime. PRL stores texture names plus a `blake3` cache key per name — never pixel data. The resource subsystem owns all textures and GPU resources; the renderer borrows handles.
> **Related:** [Architecture Index](./index.md) · [Build Pipeline](./build_pipeline.md) · [Rendering Pipeline](./rendering_pipeline.md) · [Entity Model](./entity_model.md)

---

## 1. Texture Pipeline

Visual assets are authored as PNG files. World-material textures bake into `.prm` mip sidecars at compile time; UI textures (splash, HUD) load PNGs directly at runtime. No WAD files. No embedded pixel data in PRL.

### 1.1 Authoring Layout

Textures live under `content/<mod>/textures/` (where `<mod>` is `base` for first-party content or `dev` for engine test fixtures) with one required subdirectory level:

```
content/<mod>/textures/<collection>/<name>.png
```

Example paths: `content/base/textures/concrete/wall.png`, `content/dev/textures/metal/panel.png`.

TrenchBroom requires the collection subdirectory structure for texture browsing. Collections group related textures (e.g., `concrete/`, `metal/`, `trim/`). The texture root is not accessed at runtime for world materials — source PNGs are consumed by `prl-build` only.

### 1.2 PRL Texture References

PRL stores a deduplicated texture name list (`TextureNames` section) plus a parallel `TextureCacheKeys` section — one 32-byte `blake3` hash per name entry, same ordering. No pixel data.

**Compile time.** `prl-build` resolves each `TextureNames` entry to its PNG bundle: `{name}.png` (diffuse), `{name}_s.png` (specular), and `{name}_n.png` (normal-map) discovered by suffix via case-insensitive lookup. All three are optional — a bundle is baked whenever at least one is found; when none are found, a zero key signals the runtime to substitute placeholders without warning. The Mitchell-Netravali baker (B = C = 1/3) produces full mip chains in linear space — sRGB diffuse decoded to linear before filtering and re-encoded on output; R8 specular filtered linearly; Rgba8 normal filtered linearly with per-output-texel renormalization. Output is one `.prm` sidecar per content-addressed bundle under `<workspace>/.build-caches/prm-cache/<blake3-hex>.prm`. If no PNG is found for a name, the compiler writes a zero key (`[0u8; 32]`) and emits no `.prm`.

**Level load.** For each `TextureCacheKeys[i]`, the engine opens `<workspace>/.build-caches/prm-cache/<hex>.prm`, parses it with `PrmFile::from_bytes_partial`, and uploads each present slot's mip chain directly. A zero key produces a silent placeholder. A corrupt or missing sidecar logs a `warn!` and substitutes per-slot placeholders; cleanly-parsed slots from a partially-corrupt file are used. The runtime never opens a PNG for world materials.

**UI textures.** `UiTexture` (`crates/postretro/src/ui_texture.rs`) loads PNGs directly at runtime via the splash and HUD paths. CPU-side only; no wgpu handles.

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
| `emissive` | Emissive — sole type with rendering bypass (see below) |
| `neon` | Neon — aesthetic type (shininess, future audio behaviors); not emissive |
| `metal` | Metal |
| `concrete` | Concrete |
| `grate` | Grate |

The material enum and prefix derivation are implemented. Behavior hooks are planned for later phases:

| Behavior | Status |
|----------|--------|
| **Emissive rendering bypass** | Implemented — `emissive_` surfaces render at full albedo brightness, bypassing lightmap and dynamic light modulation. `emissive_intensity` controls per-surface bypass strength (1.0 = full bypass; values above 1.0 are bloom intensity targets for a future bloom pass). The `emissive_` prefix is the bloom opt-in signal — no threshold-driven framebuffer extraction. See §4.5. |
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

Optional sibling textures provide per-texel surface properties. Suffixes are appended to the diffuse texture name (e.g., `wall.png` → `wall_s.png`). Siblings are discovered by `prl-build` at compile time, not by the runtime.

### 4.1 Specular Maps (Milestone 5)

Per-texel specular intensity modulates the direct lighting highlight.

- **Naming:** `{name}_s.png` suffix.
- **Format:** R8Unorm (sampled as `.r` in shader).
- **Color Space:** Linear.
- **Dimensions:** Must match the diffuse texture.
- **Fallback:** Absent or missing sibling bakes to `NotPresent` in the `.prm`; the runtime substitutes a shared 1×1 black texture (zero specular response).

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
- **Baking:** Filtered linearly in `f32`; per output texel the result is renormalized to unit length (degenerate normals below `1e-4` substitute `(0, 0, 1)`).
- **Placeholder:** Shared 1×1 neutral-normal texture encoding `(127, 127, 255)` — decodes to approximately `(0, 0, 1)` (tangent-space +Z). Engine-lifetime; survives level unload. Used when `_n.png` is absent or baking fails.
- **Fallback:** Missing sibling bakes to `NotPresent`; the runtime substitutes the placeholder silently. Flat mesh-normal shading is preserved — the placeholder is a true no-op through the TBN path.

### 4.4 Normal Map Generation Tool

`tools/gen_normal.py` generates `_n.png` siblings from diffuse textures via Sobel filtering on luminance. Material-prefix strength heuristics: `metal_` (1.5), `stone_` (1.2), `concrete_`/`wood_` (1.0), `plaster_` (0.8).

- **Dependencies:** `Pillow` + `numpy`. Managed via `uv`.
- **Setup:** `uv venv && source .venv/bin/activate && uv pip install Pillow numpy`.
- **Usage:** `python3 tools/gen_normal.py --input <path> --recursive`.
- **Fallback:** without `numpy`, emits flat `(127, 127, 255)` maps with no surface detail.
- **Linear guarantee:** no `sRGB`, `gAMA`, or `iCCP` chunks — passes `prl-build` validation.

### 4.5 Emissive Masks

Per-texel emissive weight for `emissive_` prefixed surfaces. The mask blends each texel between normal lighting (weight 0.0) and the emissive bypass (weight 1.0). Non-emissive buckets bind the shared white placeholder at binding 5 without sampling it — the pipeline layout requires all bindings to be satisfied.

- **Naming:** `{name}_e.png` suffix.
- **Format:** R8Unorm (sampled as `.r` in shader). Must not be sRGB-tagged — sRGB-tagged files are rejected at load time and fall back to the white placeholder with a warning.
- **Color Space:** Linear.
- **Dimensions:** Must match the diffuse texture. Mismatch logs a warning and falls back to the white placeholder.
- **Fallback:** Shared 1×1 white texture (entire surface emissive). Silent substitution for missing `_e.png`.
- **Bloom opt-in:** The `emissive_` prefix is the bloom opt-in signal for a future bloom pass. `emissive_intensity > 1.0` sets per-surface bloom brightness. No threshold-driven framebuffer extraction.

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

Camera-facing textured quads used for pickups, projectiles, and decorative elements. Characters (enemies, player, NPCs) may use either billboard sprites or 3D models.

### 6.1 Asset Format

Loaded from PNG at runtime. Sprite sheets are not used — each frame is an individual PNG. Animated sprites follow the sequential naming convention described in section 1.3.

### 6.2 Lighting

Sprite lighting is per-sprite, not per-pixel. Lighting behavior and fallback paths are defined in `rendering_pipeline.md` §7.3.

---

## 7. Resource Ownership

The renderer owns all GPU-side resources: wgpu buffers, textures, samplers. CPU-side decoded data (UI textures) lives outside the renderer. At level load, baked `.prm` bytes are parsed and uploaded to the GPU; the renderer returns opaque handles. Other subsystems borrow these handles — they never call wgpu directly.

### 7.1 Texture Types

| Type | Location | Description |
|------|----------|-------------|
| `UiTexture` | `crates/postretro/src/ui_texture.rs` | CPU-side `{ data, width, height }`. RGBA8 decoded from PNG. Used for splash and HUD blits. No wgpu handles. |
| `LoadedTexture` | `crates/postretro/src/render/loaded_texture.rs` | World-material GPU textures: wgpu handles for diffuse, specular, and normal slots plus `mip_count`. Lives inside the renderer module to preserve the "Renderer owns GPU" invariant. |

### 7.2 Lifecycle

| Phase | Action |
|-------|--------|
| Level load | Parse PRL `TextureNames` and `TextureCacheKeys`. Open each `.prm` sidecar, upload mip chains to GPU. Build sampler pool. Distribute handles. |
| Gameplay | Handles are stable. No allocation or deallocation during gameplay. |
| Level unload | Release all GPU resources. Drop all texture data. Handles become invalid. |

Resources are loaded once at level load and released on level unload. No incremental loading during gameplay. No reference counting — the level owns everything, and everything dies with the level.

### 7.3 Sampler Pool

The renderer maintains a `mip_count_samplers: HashMap<u32, wgpu::Sampler>` pool — one sampler per distinct `mip_count` observed across loaded textures. Each sampler is identical except `lod_max_clamp = (mip_count - 1) as f32`. The pool is eagerly populated after `load_textures` returns, unconditionally including `{1}` for placeholders. Sampler lifetime is engine-lifetime; the pool accumulates new entries across level reloads but never shrinks. Material bind groups select the sampler whose key matches the texture's `mip_count`. A lookup miss is a logic error — eager population guarantees coverage.

### 7.4 Renderer Contract

Renderer receives opaque resource handles at level load. It uses these handles to bind textures and buffers during draw calls. Renderer never interprets raw texture data or manages GPU memory directly. If a handle is invalid (stale reference after level unload), the engine must prevent use — this is a logic error, not a recoverable condition.

---

## 8. Non-Goals

- **WAD file support.** All textures are PNGs. No Quake/Half-Life WAD import or export.
- **Runtime texture generation.** No render-to-texture for mirrors, portals, or security cameras.
- **Hot-reload.** Textures are loaded once per level. No file-watching or live refresh during gameplay.
- **Procedural textures.** No noise-based or shader-generated textures. All surfaces use authored PNGs.
- **Texture streaming / virtual textures.** All textures for a level are loaded upfront. No partial or on-demand loading.
- **Cubemap bake tool.** The entity format and runtime consumption path are defined. The offline bake tool is deferred.
