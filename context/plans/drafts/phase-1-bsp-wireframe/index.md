# Phase 1: BSP Loading and Wireframe

> **Status:** draft — ready for implementation.
> **Depends on:** nothing. This is the starting point. A wgpu window with clear-color rendering already exists in `src/main.rs`.
> **Related:** `context/lib/rendering_pipeline.md`, `context/lib/build_pipeline.md`, `context/lib/development_guide.md`, `context/plans/roadmap.md`

---

## Goal

Load a BSP2 file, render its geometry as wireframe, and navigate it with a free-fly camera. This proves the core data path — BSP file on disk through qbsp parsing, engine-side vertex construction, GPU buffer upload, and wgpu draw calls — and establishes the renderer module boundary that all later phases build on. PVS culling validates that visibility data works before textured rendering adds complexity.

---

## Scope

### In scope

- BSP2 file loading via qbsp crate (vertices, edges, faces, models, visibility)
- Engine-side vertex construction (position only — no texture UVs, no lightmap UVs)
- Vertex and index buffer upload to wgpu
- Wireframe render pipeline (line topology or triangle wireframe via polygon mode)
- PVS culling: determine camera leaf, decompress PVS, skip non-visible leaves
- Free-fly camera with raw winit keyboard/mouse input
- BSP file path passed as command-line argument or hardcoded dev default
- Frustum culling (coarse — per-leaf AABB, not per-face)

### Out of scope

- Textures, lightmaps, materials, normal maps
- BSPX lump parsing (RGBLIGHTING, LIGHTINGDIR, LIGHTGRID_OCTREE, BRUSHLIST)
- Fixed timestep / frame timing (Phase 2)
- Input action mapping / gamepad (Phase 2)
- Depth buffer (not needed for wireframe — add in Phase 3 with solid rendering)
- Entity lump parsing
- Audio
- Collision detection

---

## Task List

| ID | Task | File | Dependencies | Description |
|----|------|------|-------------|-------------|
| 01 | BSP Loading | `task-01-bsp-loading.md` | none | Parse BSP2 file, extract geometry, build vertex/index buffers and face metadata |
| 02 | Wireframe Renderer | `task-02-wireframe-renderer.md` | 01 | Create renderer module, upload buffers to GPU, build wireframe pipeline |
| 03 | Free-Fly Camera | `task-03-free-fly-camera.md` | 02 | Camera with raw winit input, view-projection uniform upload each frame |
| 04 | PVS Culling | `task-04-pvs-culling.md` | 01, 03 | Leaf lookup, PVS decompression, filter draw set to visible leaves |
| 05 | Frustum Culling | `task-05-frustum-culling.md` | 03, 04 | Discard PVS-visible leaves outside view frustum |
| 06 | Diagnostics | `task-06-diagnostics.md` | 04, 05 | Log/display face counts, leaf index, camera position |

---

## Execution Order

### Dependency graph

```
01 ──► 02 ──► 03 ──► 04 ──► 05 ──► 06
```

Tasks are strictly sequential. Each step produces output the next step consumes:

1. **Task 01** — BSP loading. Produces vertex/index data and BSP structures.
2. **Task 02** — Renderer. Consumes vertex/index data, produces GPU pipeline and uniform buffer.
3. **Task 03** — Camera. Consumes uniform buffer update path, produces camera position and view-projection matrix.
4. **Task 04** — PVS culling. Consumes BSP tree + visibility data + camera position, produces filtered draw set.
5. **Task 05** — Frustum culling. Consumes PVS-visible set + view-projection matrix, produces final draw set.
6. **Task 06** — Diagnostics. Consumes all pipeline metrics, produces observable output.

### Parallelism

None. Each task depends on the output of the previous task. Assign sequentially.

### Notes for the orchestrator

- Each task is a self-contained unit. The assigned agent reads the task file and has enough context to implement without reading other task files.
- The test BSP file must exist before task-01 can verify its work visually. Task-01 includes the test map bootstrap instructions.
- Tasks 01 and 02 can be verified independently (data correctness via assertions and visual output respectively). Tasks 04-06 build on each other and are best verified by navigating the map and observing draw count changes.

---

## Acceptance Criteria

1. **BSP loads without error.** `cargo run -- assets/maps/test.bsp` shows BSP geometry as wireframe. No panic, no blank screen.
2. **Free-fly camera works.** WASD moves, mouse rotates, Q/E descend/ascend. Movement is smooth and frame-rate independent.
3. **Wireframe is correct.** BSP geometry is recognizable as the test map. No missing faces, no degenerate triangles, no coordinate-space errors (walls aren't floors, rooms aren't inside-out).
4. **PVS culling functions.** Navigate to a room not visible from another room. Draw counts drop. Compile without `vis` — draw count rises to total, confirming PVS was working.
5. **Frustum culling functions.** Face a wall in an enclosed room. Draw count drops further vs. PVS-only. Turn around — different faces are drawn.
6. **Missing PVS degrades gracefully.** Load a BSP compiled without `vis`. All faces draw. Warning logged once. No crash.
7. **Module boundary holds.** BSP loader contains zero wgpu imports. Renderer does not parse BSP structures. Data crosses the boundary as engine-defined types.

---

## What Carries Forward

### Durable outputs (used by later phases as-is or extended)

| Output | Consumed by |
|--------|-------------|
| BSP loading and geometry extraction | Every phase — all rendering starts from BSP data |
| PVS culling pipeline | Every phase — always determines visible set before drawing |
| Frustum culling | Every phase — always refines visible set |
| Camera projection parameters (FOV, clip planes) | Phase 2 carries these forward when replacing input handling |
| Renderer module structure | Every phase — render pipeline grows but module boundary is established |
| Test map BSP | Every phase as development fixture |

### Replaced in later phases

| Phase 1 artifact | Replaced by | When |
|------------------|-------------|------|
| Free-fly camera input (raw winit) | Action-mapped camera (input subsystem) | Phase 2 |
| Position-only vertex format | Full vertex format (position, base UV, lightmap UV, vertex color) | Phase 3 |
| Wireframe render pipeline | Textured render pipeline with depth buffer | Phase 3 |
| Wall-clock delta time | Fixed-timestep frame loop | Phase 2 |
