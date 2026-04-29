# Map to Render Pipeline

> **Read this when:** you want to understand how the whole system works, end to end.
> **Scope:** what the code does today. Compiler, format, and engine.
> **Related:** `context/lib/build_pipeline.md` · `context/lib/rendering_pipeline.md`

---

## Big picture

Three crates, two distinct phases.

```
TrenchBroom (.map + textures/)
        │
        ▼
 postretro-level-compiler  (offline, run once per map)
        │
        ▼  writes
  .prl file
        │
        ▼  read at startup
     postretro  (engine, runs every frame)
        │
        ▼
  Rendered frame
```

`postretro-level-format` is a shared library crate. Both the compiler and the engine depend on it for the binary container types — it defines how sections are laid out on disk and provides the serialization/deserialization logic for each section.

---

## Phase 1: Compiler (`postretro-level-compiler`)

Binary name: `prl-build`. Invoked as:

```
prl-build input.map -o output.prl [--format idtech2]
```

Entry point: `src/main.rs`. Stages run sequentially; each stage's output feeds the next.

---

### Stage 1 — Parse (`parse.rs`)

**Crates:** `shambler` (wraps `shalrath` for .map syntax), `glam` (math)

Reads the text .map file. `shambler` parses the Quake/idTech2 brush format: entities, brushes, and face planes. Each face has three coplanar points (defining the plane), a texture name, and texture projection parameters.

Two transforms happen here and only here:

1. **Axis swizzle.** Quake uses Z-up (X forward, Y left, Z up). The engine uses Y-up (X right, Y up, −Z forward). Every vertex and normal is transformed: `engine = (−quake_y, quake_z, −quake_x)`.

2. **Unit scale.** Quake maps are in inches. Engine units are meters. Every position is multiplied by 0.0254.

After parsing, all coordinates are in engine space for the rest of the compile. Normals are only swizzled, not scaled.

`shambler`'s `face_indices` function also sorts polygon vertices into a consistent winding order. It takes a `FaceWinding` parameter; the compiler passes `FaceWinding::Clockwise` — which, due to shambler's naming convention being relative to the brush interior, produces CCW-from-front vertices as the engine expects.

**Output:** `MapData` — world faces (geometry from the `worldspawn` entity only), brush volumes (half-planes for CSG), and entity list.

---

### Stage 2 — CSG clipping (`csg.rs`, `geometry_utils.rs`)

**Crates:** `glam`

When brushes overlap, their shared interior faces produce z-fighting (two surfaces at the same depth flickering against each other). This stage removes them.

For each face, the stage tests every brush volume for overlap (AABB pre-filter first; most pairs are rejected here). For any brush that overlaps, it clips the face polygon to the portion outside the brush using **Sutherland-Hodgman clipping**: iterates polygon edges, classifies each vertex as inside/outside the clipping plane, and emits intersection points where edges cross. The result is a new polygon containing only the visible portion.

Faces entirely inside a brush are discarded. Faces partially inside are replaced with their clipped remnant.

**Output:** Clipped face list — no z-fighting, reduced face count.

---

### Stage 3 — BSP partitioning (`partition/bsp.rs`, `partition/mod.rs`)

**Crates:** `glam`, `anyhow`

Builds a **Binary Space Partition tree** from the face set. A BSP tree recursively divides space with planes until each leaf contains a manageable number of faces. The engine uses this tree at runtime to find which spatial region the camera is in.

Plane selection is greedy: for each node, the compiler tries every candidate plane (taken from face planes already present in the scene) and picks the one with the best score. The score penalizes both face splits (a face straddling the plane must be duplicated) and tree imbalance (too many faces on one side). When a node's faces fit within a threshold, it becomes a leaf.

After the tree is built, each leaf is classified as **empty** (reachable by the player) or **solid** (inside brush geometry). Empty leaves are where geometry lives; solid leaves are skipped by the engine.

Faces that span a splitting plane are clipped by the same Sutherland-Hodgman algorithm from Stage 2 and placed on both sides.

**Output:** `BspTree` — flat arrays of nodes and leaves, with faces split and assigned to leaves.

---

### Stage 4 — Portal generation (`portals.rs`)

**Crates:** `glam`

The BSP tree knows which leaves exist, but not which leaves are adjacent. Portals are convex polygons on splitting planes that connect two empty leaves — a door in the spatial partition.

The algorithm is a **recursive portal distribution**. At each BSP node, the compiler creates a large quad on the node's splitting plane, clips it down to size using all ancestor planes (so it fits within the region of space the current node covers), and records which empty leaf is on each side. This is repeated recursively through the entire tree.

The portal graph is what the engine uses at runtime to traverse from the camera's leaf outward, finding visible leaves without testing every leaf in the scene.

**Output:** `Vec<Portal>` — each portal stores its polygon vertices and its two adjacent leaf indices.

---

### Stage 5 — Visibility (`visibility/`)

**Crates:** `rayon` (parallel iteration), `postretro-level-format`

The portal list from Stage 4 is encoded directly into the PRL file. The engine performs visibility traversal at runtime each frame — portal traversal is the sole visibility path. See the engine section for how this works.

**Output:** Encoded `BspNodesSection`, `BspLeavesSection`, and `PortalsSection`.

---

### Stage 6 — Geometry extraction (`geometry.rs`)

**Crates:** `glam`, `postretro-level-format`

Turns the face list into GPU-ready vertex and index buffers.

Faces are ordered by their BSP leaf (only empty leaves contribute; solid-leaf faces are skipped). For each face:

1. **UV computation.** Each vertex needs a texture coordinate. The compiler doesn't have access to the PNG files, so it computes raw texel-space UVs — coordinates in pixels, not normalized 0–1. Two projection modes are supported, both operating in Quake space (so each vertex position is inverse-transformed back before the projection math):
   - **Standard (idTech2) projection:** Projects the vertex onto the closest axis-aligned plane (floor/ceiling/wall), then applies rotation, per-axis scale, and offset.
   - **Valve 220 projection:** Uses explicit U and V axis vectors stored per-face in the .map file, plus per-axis offsets. More flexible; required for non-axis-aligned textures.

2. **Fan triangulation.** Each face polygon is split into triangles by fanning from vertex 0: triangles are `(0,1,2)`, `(0,2,3)`, `(0,3,4)`, etc. Indices are relative to a base vertex offset in the global buffer.

3. **Texture name deduplication.** A `texture_index` per face points into a deduplicated list of texture name strings. Faces with the same texture name share an index — this matters for draw call batching later.

**Output:** `GeometrySectionV2` (vertices as `[x,y,z,u,v]` floats, index buffer, face metadata) and `TextureNamesSection`.

---

### Stage 7 — Pack and write (`pack.rs`)

**Crates:** `postretro-level-format`, `std::io`

Assembles all sections and writes the binary PRL file. After writing, the compiler reads the file back and validates it (round-trip check).

Five sections are written:

| ID | Section | Content |
|----|---------|---------|
| 3  | GeometryV2 | Vertex buffer (position + UV), index buffer, face metadata |
| 12 | BspNodes | Splitting planes, front/back child references |
| 13 | BspLeaves | Face ranges, AABB bounds, solid flag |
| 15 | Portals | Portal polygon vertices + leaf adjacency records |
| 16 | TextureNames | Length-prefixed UTF-8 strings |

---

## PRL binary format (`postretro-level-format`)

The container is simple: a 4-byte magic (`PRL\0`), a version u16, a section count u16, then a section table (ID, byte offset, byte size per entry), then the section blobs. All integers are little-endian.

Each section is self-contained and independently seekable. The engine reads sections by ID — unknown section IDs are skipped gracefully. Old PRL files missing the `TextureNames` or `GeometryV2` sections still load; the engine falls back to zero UVs and checkerboard placeholders.

This is what lets new format features (like texture support) be added without breaking old compiled maps.

---

## Phase 2: Engine (`postretro`)

Entry point: `src/main.rs`. All of the following runs before the render loop starts:

1. Detects file extension (`.bsp` or `.prl`), routes to the appropriate loader.
2. Loads textures.
3. Normalizes UVs (PRL path only).
4. Creates the window and wgpu GPU context.
5. Uploads geometry to the GPU.

Then the event loop runs, driving frames.

---

### Loading PRL (`prl.rs`)

**Crates:** `postretro-level-format`, `glam`

Reads each section by ID. For a file with `GeometryV2` and `TextureNames` sections, it:

1. Reads vertices as `[x,y,z,u,v]` and converts them to `TexturedVertex` (position + raw UV + white vertex color).
2. Reads face metadata, maps each `texture_index` to the corresponding texture name.
3. Calls `derive_material()` per face using the texture name prefix (e.g. `metal/` → metal material enum).
4. Reads BSP nodes and leaves.
5. Reads the portal list.
6. Sorts the index buffer by `(leaf_index, texture_index)` — this ordering is required for the draw call batching strategy.
7. Builds `TextureSubRange` lists per leaf: contiguous index ranges that share a texture.

For legacy files with `Geometry` (ID 1) instead of `GeometryV2`, UVs are filled to zero.

**Output:** `LevelWorld` — nodes, leaves, vertex/index buffers, face metadata, texture names, portal graph.

---

### Loading BSP (`bsp.rs`)

**Crates:** `qbsp` (Quake BSP2 parser), `glam`

Uses the `qbsp` crate to parse `.bsp` files compiled by ericw-tools. The same coordinate transform (Quake Z-up → engine Y-up, scale 0.0254) is applied at load time. UV computation uses the same projection logic as the compiler.

---

### Texture loading (`texture.rs`)

**Crates:** `image` (PNG decoding)

Takes a list of texture names (strings like `"metal/floor_01"`), resolves them to PNG files under `assets/textures/`. Matching is case-insensitive. Files that don't exist or fail to decode fall back to a 64×64 magenta/black checkerboard placeholder. No crash on missing textures.

After loading, the engine knows each texture's dimensions — needed to normalize PRL UVs.

---

### UV normalization (PRL path, `main.rs`)

The compiler stored texel-space UVs (raw pixel coordinates). The shader expects normalized 0–1 UVs. After texture loading, the engine does a one-time pass: for each vertex, divide its UV by the dimensions of the texture assigned to that face. This is the step that connects the two phases — the compiler didn't need PNG access, but the engine does.

---

### Frame loop (`main.rs`, `frame_timing.rs`)

winit owns the event loop. On each iteration:

1. **Input** — poll window and device events, update `InputSystem` state.
2. **Game logic** — fixed-timestep update (default 60 Hz). Player movement, camera angles. `FrameTiming` accumulates elapsed time and steps forward in fixed increments, clamping to 250 ms to avoid catch-up ticks after stalls.
3. **Render** — determine visible leaves, build draw commands, submit to GPU.
4. **Present** — present the rendered frame via the wgpu surface.

Audio is stubbed (step 3 in the intended order, skipped in practice).

---

### Visibility (`visibility.rs`, `portal_vis.rs`)

**Crates:** `glam`

**Point-in-leaf lookup.** Before visibility can be determined, the engine finds which BSP leaf the camera is in. It descends the BSP node tree: at each node, dot-product the camera position against the splitting plane, go left (front) or right (back) based on sign. O(log n) for the tree height.

**Visibility path.** Portal traversal (`portal_vis.rs`) is the sole path. A BFS from the camera's leaf through the portal graph. At each portal, the engine clips the current view frustum to the portal's polygon, producing a narrower frustum for the next leaf. This naturally handles around-the-corner visibility without any precomputed data.

**Fallbacks.** Solid-leaf camera, exterior-camera, and missing-portals cases fall back to per-leaf AABB frustum culling against all leaves.

**Frustum culling.** Each candidate leaf is tested against the view frustum. The frustum is 6 planes extracted from the view-projection matrix (**Griess-Hartmann method**: add or subtract the projection matrix rows). Each plane is tested against the leaf's AABB using the **positive vertex (p-vertex) test**: find the AABB corner that lies farthest along the plane's normal; if that corner is behind the plane, the entire box is outside the frustum.

**Output:** A list of draw ranges — index buffer regions to draw this frame.

---

### Rendering (`render.rs`)

**Crates:** `wgpu` (GPU abstraction layer, supports Vulkan, Metal, DX12), `glam`

The renderer owns all GPU state. Nothing outside `render.rs` touches wgpu types.

**Vertex format.** Each vertex is 36 bytes: position `[f32; 3]`, base UV `[f32; 2]`, vertex color `[f32; 4]` (currently always white; reserved for per-vertex lighting).

**Shader (WGSL, embedded in the binary).** Vertex stage: multiplies position by view-projection matrix, passes UV and vertex color through. Fragment stage: samples the face's texture at the UV, multiplies by `ambient_light` and `vertex_color`.

**Pipeline state.** Triangle list topology, CCW front face, back-face culling enabled, depth test Less with writes enabled.

**Draw call batching.** The index buffer is sorted by `(leaf, texture)`. For each visible leaf, `TextureSubRange` entries describe contiguous index ranges that share a texture. One draw call per sub-range — typically one per texture per visible leaf. The sorted order means the GPU can batch these efficiently without pipeline state changes between faces of the same texture.

Per draw call: bind the texture's bind group, set the index offset and count, issue `draw_indexed`.

---

## Coordinate systems at a glance

| Space | Convention | Where used |
|-------|------------|------------|
| Quake space | Z-up, X-forward, Y-left; inches | .map file, shambler output, UV projection math |
| Engine space | Y-up, X-right, −Z-forward; meters | Everything after `parse.rs`; all PRL data on disk; all engine structs |
| Clip space | wgpu NDC (+Y up, depth 0–1) | Inside the vertex shader only |

The Quake→engine transform is `engine = (−quake_y, quake_z, −quake_x)` × 0.0254 for positions; same swizzle, no scale for normals. It is applied in exactly two places: `parse.rs` in the compiler (positions and normals), and `bsp.rs` in the engine (BSP loader). The PRL format stores engine-space data, so `prl.rs` applies no transform.
