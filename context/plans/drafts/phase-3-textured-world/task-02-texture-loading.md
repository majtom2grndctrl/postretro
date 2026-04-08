# Task 02: Texture Loading

> **Phase:** 3 — Textured World
> **Dependencies:** none. Can run in parallel with tasks 01, 03, and 05.
> **Produces:** CPU-side RGBA8 texture data for each BSP-referenced texture, indexed by texture index. Consumed by task-04 (renderer uploads to GPU and creates per-texture bind groups).

---

## Goal

Load PNG textures from disk at runtime, matched by BSP texture name strings. Produce CPU-side RGBA8 image data for each texture. Generate a checkerboard placeholder for missing textures. The texture loader is a separate module from the renderer — it produces data, never touches wgpu.

---

## Implementation Guidance

### Dependency addition

Add the `image` crate to `Cargo.toml`:

```toml
image = { version = "0.25", default-features = false, features = ["png"] }
```

Disable default features to avoid pulling in unused codec support. Only PNG decoding is needed.

### Texture name extraction

BSP stores texture references as `BspMipTexture` entries. Each entry has `header.name` (texture name string) and `header.width`/`header.height` (dimensions used by qbsp for UV mapping).

Build a list of unique texture names from BSP data. Each face references a texture via `BspTexInfo.texture_idx` pointing into this list.

### File search

Textures live under `textures/` with one subdirectory level: `textures/<collection>/<name>.png`.

For each BSP texture name:
1. Search all `textures/<collection>/` directories for a file whose stem matches the texture name.
2. Case-insensitive match on the filename stem (BSP names may be lowercase; filenames may vary).
3. First match wins. If multiple collections contain the same name, log a warning and use the first found.

Search happens once at level load. Cache the name-to-path mapping.

### PNG loading

For each found PNG:
1. Open with `image::open()` or `image::io::Reader`.
2. Convert to RGBA8 via `.to_rgba8()`.
3. Store pixel data, width, and height.

### Checkerboard placeholder

For missing textures, generate a 64x64 RGBA8 checkerboard:
- 8x8 pixel squares alternating magenta (`[255, 0, 255, 255]`) and black (`[0, 0, 0, 255]`).

Log a warning per missing texture at load time: texture name and "using checkerboard placeholder."

### Output structure

Indexed collection: texture index (matching BSP miptexture array index) maps to a loaded texture struct containing:
- Pixel data (`Vec<u8>`, RGBA8)
- Width, height
- Whether this is a placeholder (useful for diagnostics)

### Module boundary

The texture loader produces CPU-side data. It does not create wgpu textures, texture views, or samplers. The renderer receives loaded texture data and performs all GPU uploads in task-04.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Image crate features | PNG only. No default features. |
| Search strategy | Recursive scan of `textures/` at load time. Case-insensitive stem matching. |
| Duplicate names across collections | First match wins. Log warning. |
| Placeholder dimensions | 64x64 checkerboard, magenta/black, 8x8 squares. |
| Output format | RGBA8 for all textures, including placeholders. Uniform format simplifies GPU upload. |

---

## Acceptance Criteria

1. All BSP-referenced textures loaded from `textures/` when PNG files exist. Pixel data is RGBA8.
2. Missing textures produce a checkerboard placeholder. Warning logged per missing texture.
3. Case-insensitive matching works: `METAL_FLOOR_01` in BSP matches `metal_floor_01.png` on disk.
4. Texture loader module contains zero wgpu imports.
5. Output indexed by BSP texture index for direct lookup during rendering.
6. Corrupt or unreadable PNGs produce a checkerboard and warning, not a crash.
