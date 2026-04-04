# Task 02: Lightmap Atlas Construction

> **Phase:** 3 — Textured World
> **Dependencies:** task-01 (full vertex format with face metadata including texture dimensions).
> **Produces:** CPU-side RGBA8 atlas texture, lightmap UVs written into vertex buffer data. Consumed by task-05 (renderer uploads atlas to GPU, shader samples it).

---

## Goal

Build a shared lightmap atlas from BSP lighting data. Compute per-face lightmap dimensions, extract pixel data from the LIGHTING or RGBLIGHTING lump, pack faces into an atlas using shelf-packing, and compute atlas-space lightmap UVs for each face vertex. Write lightmap UVs into the vertex data produced by task-01.

---

## Implementation Guidance

### Lightmap dimension computation

Two paths based on BSPX lump presence:

**Standard path (no DECOUPLED_LM):**

For each face:
1. Get the face's `BspTexInfo` via `face.texture_info_idx`.
2. Project all face vertices via `texinfo.projection.project(vertex)` — same projection used for base texture UVs, but here we compute the bounding box.
3. Find min and max in both axes of the projected coordinates.
4. Lightmap dimensions: `width = floor((max_s - min_s) / 16) + 1`, `height = floor((max_t - min_t) / 16) + 1`. Quake standard: one lightmap sample per 16 texels.
5. Store `min_s` and `min_t` — needed for lightmap UV computation.

**DECOUPLED_LM path:**

Check `BspxData.decoupled_lm`. If present, for each face:
1. Read `DecoupledLightmap.size` directly — `U16Vec2` giving width and height.
2. Read `DecoupledLightmap.projection` — lightmap-specific `PlanarTextureProjection` (different from texture info projection).
3. Use this projection (not texinfo) for lightmap UV computation.

Detect which path to use once at load time. Log which path is active.

### Lighting data extraction

BSP lighting data lives in `BspLighting`:
- `BspLighting::Grayscale(Vec<u8>)` — standard LIGHTING lump.
- `BspLighting::Colored(Vec<[u8; 3]>)` — from RGBLIGHTING BSPX lump if present.

Check `BspxData.rgb_lighting` first. If present, use colored data. Otherwise fall back to `BspData.lighting` (grayscale). If neither exists, generate a flat white lightmap.

For each face:
1. Read `face.lightmap_offset`. Byte offset into the lighting data.
2. Read `face.lightmap_styles[4]`. Phase 3 handles style index 0 only. If `lightmap_styles[0]` is 255, the face has no lightmap — skip, use white.
3. Extract `width * height` samples starting at the offset. Grayscale: 1 byte per sample. Colored: 3 bytes per sample.

**Lightmap styles note:** Each face can have up to 4 lightmap layers. Style 0 data starts at `lightmap_offset`. Style 1 data follows immediately after style 0's `width * height` samples. Phase 3 reads only the first layer.

### Atlas packing (shelf algorithm)

Shelf-packing. No external crate needed.

1. Sort faces by lightmap height (tallest first).
2. Maintain a current shelf: y-offset, shelf height, x-cursor.
3. For each face's lightmap rectangle:
   - If it fits in the current shelf (`x_cursor + width <= atlas_width`), place it. Advance x-cursor.
   - Otherwise, start a new shelf: advance y-offset by current shelf height, reset x-cursor, set new shelf height.
4. If y-offset exceeds atlas height, the atlas is too small. Double height and retry.

Starting atlas size: 1024x1024. Most Q1-scale maps fit. Grow if needed, capped at 4096x4096.

Add 1-pixel padding between lightmap rectangles to prevent bilinear filtering bleed. Pad pixels replicate the nearest edge texel.

### Atlas image construction

CPU-side RGBA8 image (`Vec<u8>`, width * height * 4 bytes):

- **Colored lightmaps:** RGB from lighting data, A = 255.
- **Grayscale lightmaps:** R = G = B = intensity, A = 255.
- **No lightmaps (flat white):** All pixels (255, 255, 255, 255).
- **Unoccupied atlas regions:** Black (0, 0, 0, 255). Not sampled by any face.

### Lightmap UV computation

For each face vertex, compute atlas-space UVs:

**Standard path:**
1. Project vertex using texinfo projection: `projected = texinfo.projection.project(vertex_position)`.
2. Face-local lightmap coordinates: `lm_s = (projected.x - face_min_s) / 16.0`, `lm_t = (projected.y - face_min_t) / 16.0`. Range: [0, lightmap_width) and [0, lightmap_height).
3. Atlas space: `atlas_u = (face_atlas_x + 0.5 + lm_s) / atlas_width`, `atlas_v = (face_atlas_y + 0.5 + lm_t) / atlas_height`. The 0.5 offset centers sampling within the texel.

**DECOUPLED_LM path:**
Same structure, but use `DecoupledLightmap.projection` instead of texinfo projection, and `DecoupledLightmap.size` for dimensions. No divide-by-16 — decoupled dimensions are already in lightmap-sample space. Project all face vertices with the decoupled projection, find min, subtract min from each vertex's projected coordinates.

### Write lightmap UVs into vertex data

Task-01 produces vertex data with placeholder `[0.0, 0.0]` lightmap UVs. This task overwrites those values. Design the loader's internal API so task-01 produces preliminary vertex data and task-02 completes it. The renderer receives the final vertex buffer.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Atlas starting size | 1024x1024. Grow by doubling height up to 4096x4096. |
| Packing algorithm | Shelf packing sorted by height. No external crate. |
| Padding | 1-pixel border per rectangle. Edge texel replicated. |
| Atlas format | RGBA8. RGB = lightmap color (or intensity for grayscale), A = 255. |
| DECOUPLED_LM detection | Check `BspxData.decoupled_lm` once at load time. Log active path. |
| Lightmap styles | Style 0 only. Styles 1–3 deferred. |
| No-lightmap face | `lightmap_styles[0] == 255` means no lightmap. Use white (fully lit). |

---

## Acceptance Criteria

1. Per-face lightmap dimensions computed correctly. Standard path produces small dimensions (typically 2–30 per axis for normal faces).
2. DECOUPLED_LM path activates when lump is present. Dimensions come from the lump, not texinfo computation. Log confirms active path.
3. Lighting data extracted from correct offset per face. Atlas populated with non-uniform data (verifiable by saving atlas as PNG during development).
4. Atlas packing succeeds without gaps or overlaps. Each face's lightmap occupies a unique atlas region.
5. Lightmap UVs in vertex buffer point to correct atlas region. UVs are in [0,1] range (atlas-normalized).
6. RGBLIGHTING produces colored atlas samples. Monochrome LIGHTING produces grayscale. Missing lighting produces flat white. Each path works without error.
7. 1-pixel padding prevents visible seams between adjacent face lightmaps.
8. Atlas image is CPU-side RGBA8, ready for GPU upload by task-05.
