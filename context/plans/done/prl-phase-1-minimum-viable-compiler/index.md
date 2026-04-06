# PRL Phase 1: Minimum Viable Compiler

> **Status:** ready for implementation.
> **Depends on:** Phase 1 BSP Wireframe (complete). Engine renders BSP wireframe with PVS culling.
> **Related:** `context/plans/prl-spec-draft.md`, `context/lib/rendering_pipeline.md`, `context/lib/build_pipeline.md`, `context/lib/development_guide.md`

---

## Goal

Build a level compiler that reads a TrenchBroom `.map` file and produces a `.prl` binary level file. Implement a PRL loader in the engine that renders the result as wireframe with visibility culling — identical visual output to the current BSP path. This proves the full pipeline: authored map to compiled binary to engine rendering.

---

## Scope

### In scope

- PRL binary format: header, section table, typed sections (postretro-level-format crate)
- Compiler binary: .map parsing via shambler, spatial partitioning, clustering, portal generation, PVS (postretro-level-compiler crate)
- Engine PRL loader: read .prl, produce cluster-based engine data structures, render wireframe
- File extension dispatch: .bsp loads via qbsp, .prl loads via format crate
- Coordinate convention: compiler stores geometry in engine Y-up convention. No runtime transform.

### Out of scope

- Collision hulls — engine has no collision system yet (engine roadmap Phase 7). Section ID reserved, not populated.
- Texture data — engine renders wireframe. Section ID reserved, not populated.
- Light influence maps, light probes — PRL spec Phase 2
- Navigation mesh, spawn table, zone metadata — PRL spec Phase 3
- Audio propagation, destruction states, moving element variants — PRL spec Phase 4
- Incremental compilation
- Removing the BSP loading path or qbsp dependency

---

## Architecture: clusters, not BSP

The .prl format stores **clusters** (spatial regions with face assignments, bounding volumes, and precomputed visibility). The compiler may use a BSP tree internally to derive these clusters, but the BSP tree is a compiler implementation detail — it is not serialized into the .prl file and the engine never sees it.

At runtime the engine works with clusters directly:
1. Scan cluster bounding volumes to find the camera's cluster.
2. Read the PVS bitset for that cluster.
3. Frustum-cull visible clusters.
4. Draw face ranges for surviving clusters.

No BSP tree traversal. No per-leaf visibility decompression. The data model is simpler than Quake's — and if a better partitioning algorithm is found later, only the compiler changes. The format stays the same.

---

## Task List

| ID | Task | File | Dependencies | Description |
|----|------|------|-------------|-------------|
| 01 | Binary Format Definition | `task-01-binary-format.md` | none | Define PRL container format in postretro-level-format: header, section table, section IDs, read/write |
| 02 | Compiler Scaffold + Map Parsing | `task-02-compiler-scaffold.md` | 01 | CLI tool, shambler integration, brush classification, stat logging |
| 03 | Spatial Partitioning | `task-03-spatial-partitioning.md` | 02 | BSP tree (compiler-internal) + clustering into spatial regions with face assignments |
| 04 | Geometry Extraction | `task-04-geometry-extraction.md` | 03 | Fan-triangulate faces, vertex/index buffers, face-to-cluster mapping, coordinate transform |
| 05 | Portal Generation + PVS | `task-05-clustering-pvs.md` | 03 | Portal generation from BSP tree, cluster-to-cluster visibility, compressed bitsets |
| 06 | Pack + Write | `task-06-pack-write.md` | 01, 04, 05 | Assemble sections into .prl binary, validate offsets |
| 07 | Engine PRL Loader | `task-07-engine-loader.md` | 01, 06 | Load .prl, produce cluster-based engine types, visual validation against .bsp |

---

## Execution Order

### Dependency graph

```
01 ──► 02 ──► 03 ──┬──► 04 ──┐
                    │         ├──► 06 ──► 07
                    └──► 05 ──┘
```

1. **Task 01** — Binary format. Defines the container that all other tasks serialize into.
2. **Task 02** — Compiler scaffold. Establishes the binary crate, parses .map, extracts brush data.
3. **Task 03** — Spatial partitioning. Builds BSP tree (compiler-internal), groups leaves into clusters. Produces cluster definitions with face assignments.
4. **Tasks 04 + 05** — Geometry extraction and portal generation/PVS. Both consume task 03's output; independent of each other. Can run in parallel.
5. **Task 06** — Pack. Collects all section data and writes the .prl file.
6. **Task 07** — Engine loader. Reads the .prl and proves the pipeline works end-to-end.

### Parallelism

Tasks 04 and 05 are independent after task 03 completes. All other tasks are sequential.

### Notes for the orchestrator

- Task 03 (spatial partitioning) is the highest-risk task — BSP construction + clustering. If the partitioning is wrong, everything downstream produces wrong output. Invest in test coverage here.
- Task 05 (portal generation + PVS) is the second highest-risk task — the portal algorithm has subtle numerical edge cases.
- Task 07 modifies the engine crate, not the compiler. It introduces a new `LevelWorld` type for cluster-based level data.
- The test .map file must exist before task 02 can run. The source .map file for the existing test BSP should be available in assets/maps/.

---

## Acceptance Criteria

1. **Compiler produces a .prl file.** `prl-build assets/maps/test.map -o assets/maps/test.prl` completes without error.
2. **Engine loads the .prl file.** `cargo run -p postretro -- assets/maps/test.prl` renders wireframe geometry.
3. **Visual parity with BSP.** Same map rendered from .bsp and .prl is visually identical — same geometry, same room shapes, no missing faces.
4. **PVS culling works.** Navigate through the PRL-loaded level. Draw counts drop when facing away from open areas. Behavior matches BSP-loaded PVS culling.
5. **File extension dispatch works.** Engine loads .bsp files via qbsp and .prl files via format crate. Both paths produce the same rendering.
6. **Format is self-describing.** A .prl file with a missing section loads without error — the engine skips absent sections gracefully.

---

## What Carries Forward

### Durable outputs

| Output | Consumed by |
|--------|-------------|
| PRL binary format (postretro-level-format crate) | Every PRL phase — all compiler output and engine loading uses this format |
| Compiler scaffold and .map parsing | Every PRL phase — all compilation starts from .map data |
| Spatial partitioning algorithms (compiler-internal) | PRL Phase 2+ — light analysis, nav mesh need spatial queries during compilation |
| Cluster-based PVS | Engine rendering — always determines visible set before drawing |
| Engine PRL loader + LevelWorld type | Every phase — loader extends to handle new sections |

### Replaced in later phases

| Phase 1 artifact | Replaced by | When |
|------------------|-------------|------|
| BSP loading path (qbsp) | PRL-only loading | After PRL reaches feature parity with BSP for all engine needs |
| Position-only vertex data in .prl | Full vertex format (position, UV, lightmap UV) | PRL Phase 2 (lighting) or engine Phase 3 (textured world) |
| Empty collision/texture sections | Populated sections | When engine needs them |
