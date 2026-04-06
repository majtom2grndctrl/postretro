# Task 02: Compiler Scaffold and Map Parsing

> **Phase:** PRL Phase 1 — Minimum Viable Compiler
> **Dependencies:** Task 01 (binary format definition).
> **Produces:** compiler binary that parses .map files, consumed by tasks 03-06.

---

## Goal

Set up the postretro-level-compiler binary crate. Parse TrenchBroom .map files using the shambler crate. Classify brushes into world geometry and entity brushes. Extract face data (vertices, planes, texture names) from world brushes. This task establishes the compiler's front end — all later compilation stages consume its output.

---

## Implementation Guidance

### CLI

The compiler is invoked as:

```
prl-build <input.map> -o <output.prl>
```

Use clap or plain std::env argument parsing. Input is required. Output defaults to the input filename with .prl extension if -o is omitted.

### Map parsing

The Quake .map parsing ecosystem in Rust is a two-crate design:
- **shalrath** — the parser. Reads .map text into an AST of entities, brushes, and raw half-plane definitions.
- **shambler** — the geometry processor. Takes shalrath's output and computes vertices, face polygons, normals, UVs, and triangle indices from plane intersections.

Add shambler as a dependency (it re-exports shalrath). shambler's `GeoMap` struct provides:
- Entity properties as string key-value pairs (`Property { key, value }`)
- Brush-to-entity association (`BTreeMap<EntityId, Vec<BrushId>>`)
- Per-face data: planes, texture names, texture offsets, angles, scales

### Geometry computation via shambler

shambler computes face geometry from brush half-planes through a chain of free functions. Call them in order:

1. `face_planes()` — converts 3-point plane definitions to normal+distance form (`Plane3d`)
2. `brush_hulls()` — builds convex hulls from plane sets
3. `face_vertices()` — computes vertex positions via triplanar intersection (the expensive step)
4. `face_indices()` — sorts vertices into winding order per face

shambler handles the plane intersection math internally (triplanar intersection with convex hull containment testing, deduplication within epsilon). No need to implement this ourselves.

### nalgebra to glam conversion

shambler uses nalgebra 0.32 for its math types (`Vector3<f32>`). The engine pins glam 0.30. Convert nalgebra types to glam at the shambler output boundary. Both represent `[f32; 3]` — conversion is trivial field-by-field copy. Keep nalgebra types confined to the parsing stage; all downstream compiler stages and the format crate use glam.

### Brush classification

Separate brushes into:
- **World brushes:** belong to the worldspawn entity (classname "worldspawn"). These form the static level geometry that the BSP tree partitions.
- **Entity brushes:** belong to any other entity (func_door, func_plat, etc.). Stored separately — not included in BSP construction.

Use shambler's `entity_properties` to find each entity's classname, then `entity_brushes` to collect the associated brush IDs.

### Face extraction from world brushes

For each world brush classified as worldspawn, collect from shambler's computed data:
- Vertex positions per face (from `face_vertices`, converted to glam `Vec3`)
- Vertex winding order per face (from `face_indices`)
- Face plane normal + distance (from `face_planes`, converted to glam)
- Texture name per face (from `face_textures` + `textures` lookup)

No manual plane intersection or convex hull computation is needed — shambler does this.

### Stat logging

After parsing, log:
- Total brush count (world + entity)
- World brush count
- Entity brush count
- Total face count (world brushes only)
- Total vertex count
- Entity classnames found (deduplicated list)

This confirms the parser is extracting the expected data before downstream tasks consume it.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| .map parser crate | shambler (re-exports shalrath) — Quake-format .map parsing + geometry computation |
| Math type boundary | nalgebra types from shambler converted to glam at parser output. All downstream code uses glam. |
| Brush classification | worldspawn entity → world brushes, everything else → entity brushes |
| Entity brushes | Extracted and stored but not processed further in Phase 1 |
| Texture data | Extracted per-face but not written to .prl sections in Phase 1 |
| CSG | Not performed. shambler computes per-brush geometry only — no brush-to-brush face removal. BSP construction handles spatial partitioning. |

---

## Acceptance Criteria

1. `cargo run -p postretro-level-compiler -- assets/maps/test.map` parses the map without error.
2. Stat log shows non-zero counts for brushes, faces, and vertices.
3. World brushes are correctly separated from entity brushes. Verify with a .map that contains both worldspawn and func_ entities.
4. Face polygons have valid vertex counts (3+ vertices per face).
5. Face planes have unit normals.
6. Missing input file produces a clear error message, not a panic.
7. Invalid .map syntax produces a clear error from shambler, not an obscure crash.
