# Task 07: Engine PRL Loader

> **Phase:** PRL Phase 1 — Minimum Viable Compiler
> **Dependencies:** Task 01 (binary format definition), Task 06 (pack + write — a .prl file must exist to load).
> **Produces:** engine rendering a .prl level as wireframe with visibility culling.

---

## Goal

Add a PRL loader to the engine that reads a .prl file and produces cluster-based engine data structures. Implement file extension dispatch so both .bsp and .prl files work. The engine's PRL path works natively with clusters — no BSP tree traversal, no adapter between cluster PVS and per-leaf PVS. Validate visually: the same map loaded from .bsp and .prl should render identically.

---

## Implementation Guidance

### File extension dispatch

In the engine's level loading path, select the loader based on file extension:
- `.bsp` → existing `load_bsp()` via qbsp
- `.prl` → new `load_prl()` via postretro-level-format

The two loaders produce different types. The BSP loader produces `BspWorld` (existing, unchanged). The PRL loader produces a new `LevelWorld` type that represents cluster-based level data. The renderer and visibility system accept either.

### PRL loading

Use the format crate to:
1. Read and validate the header.
2. Read the section table.
3. Load the Geometry section: vertex buffer, index buffer, per-face metadata, per-cluster face ranges.
4. Load the Cluster Visibility section: cluster bounding volumes, compressed PVS bitsets.

No coordinate conversion at load time — the .prl stores engine-native Y-up coordinates.

### LevelWorld type

Introduce a new `LevelWorld` struct for PRL-loaded data:
- Vertex and index data (for GPU upload)
- Cluster array: per-cluster bounding volume, face range (start + count into face metadata), PVS bitset
- Face metadata array: per-face index buffer offset and count

This is a simpler type than `BspWorld` — no BSP nodes, no BSP leaves, no per-leaf PVS decompression. The engine's runtime data model for PRL levels is flat: clusters containing faces.

### Visibility with clusters

The PRL visibility flow:
1. **Point-in-cluster:** scan cluster bounding volumes to find which cluster contains the camera position. For 100-300 clusters this is a trivial linear scan.
2. **PVS lookup:** decompress the camera cluster's PVS bitset to get the set of visible cluster IDs.
3. **Frustum cull:** for each visible cluster, test its bounding volume against the view frustum. Skip clusters entirely outside the frustum.
4. **Draw:** for each surviving cluster, draw its face range.

This replaces the BSP path's point-in-leaf → PVS decompress → per-leaf frustum cull flow. The logic is structurally identical, just with clusters instead of leaves and a bounding volume scan instead of BSP tree traversal.

### Renderer integration

The renderer currently draws face ranges from `BspWorld`. It needs to also accept face ranges from `LevelWorld`. Options:

- **Trait-based:** define a common interface (`fn visible_face_ranges(&self, camera: &Camera) -> &[FaceRange]`) that both types implement.
- **Enum-based:** an enum `Level { Bsp(BspWorld), Prl(LevelWorld) }` with a match in the draw call.
- **Shared draw data:** both loaders produce the same GPU buffer handles and draw metadata; only the visibility determination differs.

Choose the approach that minimizes renderer changes. The renderer's job is the same regardless of format: upload vertices, draw indexed face ranges. Only the visibility system changes.

### Dependency

postretro-level-format is already listed as a dependency of the postretro crate (set up during workspace reorganization).

---

## Key Decisions

| Item | Resolution |
|------|------------|
| PRL engine type | New `LevelWorld` struct. Not `BspWorld` — PRL data is cluster-based, not BSP-based. |
| BSP loader | Unchanged. `BspWorld` still works for .bsp files. |
| Point-in-cluster | Linear scan of cluster bounding volumes. No spatial index needed for 100-300 clusters. |
| Coordinate transform | None at load time — .prl stores engine-native coordinates. |
| Missing sections | Log warning, degrade gracefully. Missing PVS → draw all clusters. |

---

## Acceptance Criteria

1. `cargo run -p postretro -- assets/maps/test.prl` renders the level as wireframe without error.
2. Visual parity: same map loaded from .bsp and .prl renders identically — same geometry, same room shapes, no missing or extra faces.
3. PVS culling works: navigate through the PRL-loaded level, draw counts change based on camera position.
4. File extension dispatch: .bsp loads via qbsp, .prl loads via format crate. Passing either file type works.
5. Invalid .prl file (wrong magic, truncated) produces a clear error message, not a panic.
6. Missing sections degrade gracefully: a .prl without PVS data loads and renders all geometry with a warning.
7. Renderer changes are minimal — the draw path is the same regardless of source format.
