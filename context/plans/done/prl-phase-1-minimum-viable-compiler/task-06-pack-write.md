# Task 06: Pack and Write

> **Phase:** PRL Phase 1 — Minimum Viable Compiler
> **Dependencies:** Task 01 (binary format), Task 04 (geometry), Task 05 (clustering + PVS).
> **Produces:** .prl binary file consumed by task 07.

---

## Goal

Collect all section data produced by previous tasks (geometry, cluster visibility) and write a complete .prl file. This task ties the compiler pipeline together — it's the output stage.

---

## Implementation Guidance

### Pipeline integration

Wire the compiler's main function to run all stages sequentially:

1. Parse .map (task 02)
2. Spatial partitioning — BSP tree + clustering (task 03)
3. Extract geometry (task 04)
4. Portal generation + PVS (task 05)
5. Pack and write (this task)

Each stage produces its section data as in-memory structs. This task serializes them and writes the .prl file. The BSP tree from task 03 is consumed by tasks 04 and 05 but is not serialized — only the derived data (geometry organized by cluster, cluster visibility) goes into the .prl.

### Section assembly

Serialize each section to bytes using the format crate's serialization:
- Geometry section (from task 04): vertices, indices, per-face metadata, per-cluster face ranges
- Cluster Visibility section (from task 05): cluster bounding volumes, compressed PVS

Build the section table with correct offsets and sizes. Section order in the file doesn't matter — the section table provides direct access by offset.

### File writing

Use the format crate's write API from task 01:
1. Write header (magic, version, section count)
2. Write section table
3. Write section data blobs
4. Compute offsets as: header size + section table size + cumulative section data before each section

### Validation

After writing, re-read the file and verify:
- Header magic and version are correct
- Section table entries have valid offsets and sizes
- Each section deserializes without error
- Deserialized data matches the in-memory data that was serialized

This read-back validation runs as part of the compiler's output stage, not as a separate tool. It catches serialization bugs before the engine ever sees the file.

### Logging

Log the output file path, total file size, per-section sizes, and section count.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Section ordering | Arbitrary — section table provides offset-based access |
| Validation | Read-back validation after write, built into the compiler |
| Output path | CLI -o argument, defaults to input filename with .prl extension |
| Overwrite behavior | Overwrite existing .prl without prompting |

---

## Acceptance Criteria

1. `prl-build test.map -o test.prl` produces a .prl file without error.
2. Output file starts with `PRL\0` magic bytes.
3. Section table lists both sections (Geometry, Cluster Visibility) with non-zero sizes.
4. Read-back validation passes: file is re-read and all sections deserialize to matching data.
5. Log output shows file size, section count, and per-section sizes.
6. Output to a non-existent directory produces a clear error, not a panic.
