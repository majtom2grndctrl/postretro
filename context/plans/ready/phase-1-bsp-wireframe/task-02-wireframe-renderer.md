# Task 02: Wireframe Renderer

> **Phase:** 1 — BSP Loading and Wireframe
> **Dependencies:** task-01 (BSP loading — provides vertex/index data and face metadata).
> **Produces:** a renderer module that draws all BSP faces as wireframe. Consumed by task-03 (camera provides the view-projection matrix).

---

## Goal

Create the renderer module. Upload vertex and index buffers to GPU. Build a wgpu render pipeline that draws BSP geometry as wireframe with a hardcoded view-projection. Establish the renderer module boundary that all later phases build on.

---

## Implementation Guidance

### Renderer module

Create a renderer module that owns all wgpu state: device, queue, surface, pipelines, buffers. Other modules never import wgpu types. The GPU initialization currently in `main.rs` (`create_gpu_state()`, `GpuState` struct) moves into the renderer module — the renderer takes ownership of device/adapter creation and feature negotiation.

Upload the vertex buffer (`[f32; 3]` positions) and index buffer (`u32` indices) produced by task-01 to GPU buffers via `device.create_buffer_init()` or equivalent.

### Wireframe pipeline

Build a wgpu render pipeline for wireframe drawing:

- **Vertex shader:** transform position by a view-projection uniform (mat4x4). For initial testing before task-03 provides a camera, use a hardcoded view-projection that looks at the map origin from a reasonable distance.
- **Fragment shader:** emit a constant color. Use green or cyan for the cyberpunk wireframe aesthetic.
- **Polygon mode:** `wgpu::PolygonMode::Line` (requires `Features::POLYGON_MODE_LINE`).
- **Fallback:** if `POLYGON_MODE_LINE` is unavailable on the adapter, build a line-list index buffer from face edges instead of triangle indices, and use `PrimitiveTopology::LineList`. Check feature support at adapter request time.

### Uniform buffer

Create a uniform buffer for the 4x4 view-projection matrix. Bind it via a bind group. Update each frame (initially with a static matrix; task-03 replaces with camera-driven updates).

### Draw call structure

For now, draw all faces in a single draw call (or one indexed draw covering the entire index buffer). No culling yet — tasks 04 and 05 add that.

### No depth buffer

Wireframe rendering doesn't need a depth buffer. Phase 3 adds one with solid rendering.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| `PolygonMode::Line` availability | Request `Features::POLYGON_MODE_LINE` at adapter init. If unavailable, build line-list index buffer from face edges. Metal and Vulkan support it; some WebGPU adapters may not. |
| Initial view-projection | Hardcode a matrix that places the camera outside the map looking at origin. Replaced by task-03 camera. |
| Depth buffer | None for Phase 1. Added in Phase 3 with solid rendering. |

---

## Acceptance Criteria

1. Running `cargo run -- assets/maps/test.bsp` shows BSP geometry as wireframe in the window.
2. Wireframe is recognizable as the test map — no missing faces, no degenerate triangles, no coordinate-space errors.
3. Renderer module owns all wgpu state. No wgpu imports outside the renderer.
4. Polygon mode fallback path compiles and can be forced via a CLI flag (e.g., `--force-line-list`). Verified on at least one path at runtime.
5. Uniform buffer for view-projection is in place and updatable per frame.
