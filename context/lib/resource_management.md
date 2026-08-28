# Resource Management

> **Read this when:** loading textures, working with materials, adding billboard sprites, or changing how the engine consumes visual assets.
> **Key invariant:** authored visual assets are PNGs. World-material textures are baked into per-texture `.prm` mip sidecars at compile time; UI textures load PNGs directly at runtime. PRL stores texture names plus a `blake3` cache key per name — never pixel data. Renderer owns GPU textures, samplers, bind groups, and buffers; other subsystems use opaque handles and never call wgpu.
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

**Compile time.** `prl-build` resolves each `TextureNames` entry to its PNG bundle: `{name}.png` (diffuse), `{name}_s.png` (specular), `{name}_n.png` (normal-map), and `{name}_e.png` (emissive) discovered by suffix via case-insensitive lookup. `TextureNames` entries are stored verbatim from the `.map`, so a name may be **collection-qualified** (`collection/stem`) — TrenchBroom identifies materials by their path relative to the textures root — or a bare stem (hand-authored maps). The resolver indexes each PNG under its collection-relative key (lowercased, forward-slashed, no extension) and also under a **bare-stem alias** when that stem is unique across collections (ambiguous stems get no alias and log a warning). Incoming names are normalized (lowercase, `\`→`/`, leading `textures/` stripped). A qualified base stays selected when any of its four slots exists, including sibling-only bundles; only an entirely missing qualified bundle falls back to the bare last segment. All slots then resolve from that selected base. All four are optional — a bundle is baked whenever at least one is found; when none are found, a zero key signals the runtime to substitute placeholders without warning. The Mitchell-Netravali baker (B = C = 1/3) produces full mip chains in linear space — sRGB diffuse and emissive color decode to linear before filtering and re-encode on output; R8 specular filters linearly; Rgba8 normal filters linearly with per-output-texel renormalization. Output is one `.prm` sidecar per content-addressed bundle under `<workspace>/baked/materials/<blake3-hex>.prm` (runtime-required compiled output, not the disposable `.build-caches/` stage cache — see `build_pipeline.md` §Build Cache). If no PNG is found for a name, the compiler writes a zero key (`[0u8; 32]`) and emits no `.prm`.

**Level load.** For each `TextureCacheKeys[i]`, the engine opens `<workspace>/baked/materials/<hex>.prm` and parses it with `PrmFile::from_bytes_partial`. Legacy world and model loaders upload present slot mip chains only from single-layer sidecars. A valid layered sidecar logs a `warn!` and replaces the full material with placeholders until a `D2Array` PRM upload path exists. A zero key produces a silent placeholder. A corrupt or missing single-layer sidecar logs a `warn!` and substitutes per-slot placeholders; cleanly parsed slots from a partially-corrupt file are used. The runtime never opens a PNG for world materials. Model materials use diffuse-only addressing and share sidecars only with diffuse-only world bundles. They consume only diffuse; specular and normal remain neutral and emissive remains black.

**Model helper.** `cargo run -p xtask -- bake-model-textures <scene.gltf>` bakes glTF base-color sidecars without compiling a map. Output is `<workspace>/baked/materials/*.prm`: gitignored, regenerable, runtime-required.

**UI textures.** `postretro_ui::UiTexture` (`crates/ui/src/ui_texture.rs`, package `postretro-ui`) loads PNGs directly at runtime via the splash and HUD paths. CPU-side only; no wgpu handles.

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

### 2.1 SH Irradiance Volume

Indirect lighting is carried by an SH L2 irradiance volume (3D probe grid), not by per-face lightmaps. The probe section is loaded from PRL. Sampling is a manual 8-corner `textureLoad` blend: invalid (in-wall) corners are dropped via a baked per-probe validity bit, backfacing corners are downweighted (forward pass only), and surviving weights are renormalized. See `rendering_pipeline.md` §4.

---

## 3. Material System

Texture name prefix determines material type. The engine derives a material enum from the first token of the texture name (delimited by `_`).

Texture names may be **collection-qualified** (`collection/stem`, or root-inclusive `textures/collection/stem`) because TrenchBroom identifies materials by their path relative to the textures root. The collection path is not part of the material identity: material prefix derivation lives in `postretro-render-data` (`crates/render-data/src/material.rs`). It first strips any leading path (taking the substring after the last `/`), then splits the bare name on its first `_`. So `50-free-textures/concrete_pavement_036` derives `concrete`, exactly as the bare `concrete_pavement_036` would. Bare names keep their existing behavior.

The engine provides the mechanism: prefix lookup, material enum, and per-material behavior hooks (footstep sounds, impact effects, decals). Which prefixes exist and what they map to is a game content concern — the prefix table grows as content requires it. The engine does not aim for a complete material table; it aims to make adding new materials trivial.

Example prefixes (illustrative, not exhaustive):

| Prefix | Material |
|--------|----------|
| `metal` | Metal |
| `concrete` | Concrete |
| `grate` | Grate |
| `neon` | Neon — shininess plus a static emissive multiplier for `_e` textures; planned audio/impact behaviors. |
| `glass` | Glass |
| `wood` | Wood |

The material enum and prefix derivation are implemented. Behavior hooks are planned for later phases:

| Behavior | Status |
|----------|--------|
| **Emissive surfaces** | Implemented — world and kinematic-brush `_e` texels add static self-illumination to HDR scene color, scaled by the prefix-derived material multiplier. They never replace or inject into direct/indirect lighting; bright values bloom in the renderer compositor. See §4.5. |
| **Shininess** | Implemented (Epic 5) — specular exponent on enum variant. |
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

### 4.1 Specular Maps (Epic 5)

Per-texel specular intensity modulates the direct lighting highlight.

- **Naming:** `{name}_s.png` suffix.
- **Format:** R8Unorm (sampled as `.r` in shader).
- **Color Space:** Linear.
- **Dimensions:** Must match the diffuse texture.
- **Fallback:** Absent or missing sibling bakes to `NotPresent` in the `.prm`; the runtime substitutes a shared 1×1 black texture (zero specular response).

### 4.2 Generation Tool

`tools/gen_specular.py` generates specular maps from diffuse textures using material-prefix heuristics.

- **Heuristics:** `metal_` (high intensity, low gamma), `concrete_` (low intensity, high gamma), `wood_` (moderate).
- **Dependency:** `Pillow`, managed via `uv`. Setup and invocation: `tools/README.md`.
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

- **Dependencies:** `Pillow` + `numpy`. Managed via `uv`. Setup and invocation: `tools/README.md`.
- **Fallback:** without `numpy`, emits flat `(127, 127, 255)` maps with no surface detail.
- **Linear guarantee:** no `sRGB`, `gAMA`, or `iCCP` chunks — passes `prl-build` validation.

### 4.5 Emissive Surfaces

Optional emissive color maps add static, per-texel self-illumination to world and
kinematic-brush surfaces. They are not lights: they do not change the lightmap,
SH irradiance, dynamic-light buffer, or neighboring surfaces. To light a scene,
an author places a separate light entity.

- **Naming:** `{name}_e.png` beside the diffuse texture.
- **Format:** `Rgba8UnormSrgb`, sampled as linear color by hardware. Dimensions
  must match the diffuse texture; `prl-build` rejects a mismatch at compile time.
- **Color space:** sRGB content. `prl-build` accepts `_e` regardless of its PNG
  color-space tag, so the untagged output from `tools/gen_emissive.py` is valid.
- **Baking:** the fourth optional `.prm` slot (slot-mask bit 3), after diffuse,
  specular, and normal. It uses the same sRGB decode → linear filter → sRGB
  encode path as diffuse.
- **Strength:** a prefix-derived `Material::emissive_strength()` multiplier scales
  the sampled color. `neon_` is 4.0 so an authored bright texel exceeds the
  renderer bloom threshold; other current material prefixes are zero until
  content establishes a use for them. There is no per-surface runtime scalar in
  v1.
- **Composite and fallback:** forward rendering writes `lit + emissive` into the
  renderer-owned HDR scene target. An absent `_e` uses a shared 1×1 black sRGB
  placeholder, an additive no-op. Bloom is a screen-space effect after this
  composite, not light emitted onto neighbors.

`tools/gen_emissive.py` creates a bright-texel starting point from a diffuse
texture. Its output is deliberately untagged: PNG metadata does not determine
the authored sRGB-content convention for this sibling.

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

At collection load, the PNG decode fallback creates one renderer-owned `D2Array`
texture: each decoded frame becomes one array layer. The former stitched horizontal
strip layout is retired. This is a runtime PNG path only; it neither adds a baked
sprite `.prm` sidecar nor a sprite `.prm` loader/uploader. Baked sprite sidecars and
their `D2Array` PRM upload path remain downstream scope.

### 6.2 Lighting

Sprite lighting is per-sprite, not per-pixel. Lighting behavior and fallback paths are defined in `rendering_pipeline.md` §7.4.

---

## 7. Model Geometry

Skinned or static 3D models are the character/prop alternative to billboard sprites (§6). Authored as external glTF under `content/<mod>/models/<name>/` (`.gltf` + `.bin` + sibling textures), referenced by a `MeshComponent` model path (`entity_model.md`). Model glTF loader: `crates/model`.

Weapon `thirdPersonModel` and `viewmodel` assets resolve through the same content-root join as mesh models. Their descriptor paths must be non-empty, use forward slashes, remain relative, and contain no `..` segment. Windows drive, UNC, and backslash forms are invalid on every platform.

**Geometry renders at its authored glTF scale.** The loader consumes vertex POSITION as world-space geometry — no node-transform bake into vertices, no import-time normalization or fit-to-size. World units are meters (idTech2 map geometry converts at 1 unit = 0.0254 m; models are authored directly in meters). Author characters at final size (~2 m tall) with the origin between the feet (`y = 0`) — the standing foot-level pivot the reference enemy fixture uses. Skinned meshes render their rest (bind) pose at raw POSITION too, so bind-pose geometry must already be final-scale; a model left in a large export coordinate space renders at that raw scale until baked down.

**One mesh node per model.** The loader loads a single glTF mesh — sibling mesh nodes are ignored, so a multi-mesh export renders only its first chunk. Multiple primitives within that mesh are supported: each becomes a submesh drawn with its own material (the reference models pack several). Exports that split geometry across mesh nodes — one per material, common from Sketchfab/Rodin — must be merged into one mesh whose primitives carry the per-material split. TANGENT is optional and strictly validated: one degenerate tangent rejects the whole model, so omit tangents rather than ship near-degenerate ones (only base color is consumed regardless).

**Materials must be metallic-roughness glTF.** Model loading consumes the diffuse slot only (§1.2, §8.1). The loader rejects any glTF that lists an unsupported extension in `extensionsRequired` — notably `KHR_materials_pbrSpecularGlossiness` (common in Sketchfab exports). Convert spec-gloss materials to metallic-roughness (base color = diffuse) before import.

Placed mesh entities carry `Transform.scale = 1`; no FGD or descriptor scale override exists (`entity_model.md` §4). On-screen size is entirely the authored geometry.

**Sockets (named attachment points).** A model node may carry a glTF `extras` socket tag naming an attachment point — the same per-node extras channel as hit-zone and pose tags. On skinned models the tag rides a skin joint (resolved to that joint and posed with the skeleton). On rigid models it rides the mesh node itself (identity transform) or a descendant (static rest transform in the mesh node's local frame). A descriptor's `attachments` map assigns socket names to content-relative prop-model paths. At spawn, it resolves into the `MeshComponent` attachment list (`entity_model.md`), whose entries mount loaded prop model handles at resolved socket bindings. Attachments render as rigid instances at the holder's posed socket. Presentation-only — sockets never participate in collision, hit-zone raycasts, or gameplay queries.

**Socket-mounted weapons and props.** Verify mount orientation in the engine frame, never a Blender render. Engine reads raw glTF joint matrices; Blender imports reorient bones and flip the up-axis. Normal workflow: `xtask solve-weapon-mount` declares raw-frame weapon barrel/up axes, emits a `prop_to_gltf.py` bake, then checks the baked asset. `extras.mount` persists the axes and baked Euler correction, so checks reuse declared intent. Bake scale must be finite and positive.

Solver uses a rigid, proper socket frame at its reference pose. Invalid, sheared, or reflected frames are rejected. Geometric barrel/up detection is an unverified assist, never authoring truth. `socket_dump` is a diagnostic/parity tool, not a required author workflow.

---

## 8. Resource Ownership

The renderer owns all GPU-side resources: wgpu buffers, textures, samplers. CPU-side decoded data (UI textures) lives outside the renderer. At level load, baked `.prm` bytes are parsed and uploaded to the GPU; the renderer returns opaque handles. Other subsystems borrow these handles — they never call wgpu directly.

### 8.1 Texture Types

| Type | Location | Description |
|------|----------|-------------|
| `postretro_ui::UiTexture` | `crates/ui/src/ui_texture.rs` (`postretro-ui`) | CPU-side `{ data, width, height }`. RGBA8 decoded from PNG. Used for splash and HUD blits. No wgpu handles. |
| `LoadedTexture` | `crates/renderer/src/render/loaded_texture.rs` | World- and model-material GPU resources: wgpu handles for diffuse, specular, normal, and emissive slots plus `mip_count`. World loading consumes all available slots; model loading consumes diffuse only and binds neutral/black placeholders for the rest. Lives inside the renderer module to preserve the "Renderer owns GPU" invariant. |

### 8.2 Lifecycle

| Phase | Action |
|-------|--------|
| Level load | Parse PRL `TextureNames` and `TextureCacheKeys`. Open each `.prm` sidecar, upload mip chains to GPU. During model upload, resolve each glTF-derived content key, load only the diffuse slot from its `.prm`, and bind neutral specular and normal placeholders. Build sampler pool. Distribute handles. |
| Gameplay | Handles are stable. No allocation or deallocation during gameplay. |
| Debug descriptor reload | Visual asset path additions or changes stay deferred in the installed descriptor snapshot. The latest authored snapshot promotes before the next level install preload. Gameplay never uploads a model or sprite collection. |
| Level unload | Release all GPU resources. Drop all texture data. Handles become invalid. |

Resources are loaded once at level load and released on level unload. No incremental loading during gameplay. No reference counting — the level owns everything, and everything dies with the level.

### 8.3 Material Sampler Pools

The renderer maintains two engine-lifetime sampler pools, each with one sampler per distinct `mip_count` and `lod_max_clamp = (mip_count - 1) as f32`. World and mover materials select from `mip_count_aniso_samplers: HashMap<u32, wgpu::Sampler>`: fully linear filtering with 16× anisotropy. It is eagerly populated after `load_textures` returns, unconditionally including `{1}` for placeholders; a lookup miss is a logic error. Skinned-model materials instead select from `mip_count_character_model_samplers: HashMap<u32, wgpu::Sampler>`, seeded with `{1}` and extended as model diffuse textures load. Its sampler uses nearest magnification, linear minification and mip filtering, and anisotropy `1`, keeping close character texels crisp while retaining mip-filtered distance stability. Both pools accumulate entries across level reloads but never shrink. Each material binds its selected sampler at the existing group-1 binding 5, so the separate pool does not consume another shader sampler binding.

### 8.4 Renderer Contract

CPU asset and decoded pixel data may live outside renderer. GPU resources do not. Renderer creates and owns textures, samplers, bind groups, and buffers, then returns opaque handles for other subsystems to store. Other subsystems never call wgpu or inspect GPU resources.

Renderer uses handles to bind textures and buffers during draw calls. If a handle is invalid (stale reference after level unload), the engine must prevent use — this is a logic error, not a recoverable condition.

---

## 9. Non-Goals

- **Import-time model normalization.** Models render at their authored glTF scale — no fit-to-size, unit conversion, or auto-rescale at load. Author geometry at final world size (§7).
- **WAD file support.** All textures are PNGs. No Quake/Half-Life WAD import or export.
- **Runtime texture generation.** No render-to-texture for mirrors, portals, or security cameras.
- **GPU asset hot-reload.** Textures and models are loaded once per level. Descriptor tuning may refresh, but visual asset path changes wait for the next level install.
- **Procedural textures.** No noise-based or shader-generated textures. All surfaces use authored PNGs.
- **Texture streaming / virtual textures.** All textures for a level are loaded upfront. No partial or on-demand loading.
- **Cubemap bake tool.** The entity format and runtime consumption path are defined. The offline bake tool is deferred.
