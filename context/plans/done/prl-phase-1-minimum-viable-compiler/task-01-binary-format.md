# Task 01: Binary Format Definition

> **Phase:** PRL Phase 1 — Minimum Viable Compiler
> **Dependencies:** none. First task in the phase.
> **Produces:** postretro-level-format crate with container format read/write, consumed by all other tasks.

---

## Goal

Define the PRL binary container format in the postretro-level-format crate. Implement serialization and deserialization for the header, section table, and section envelope. Individual section data types are added by later tasks — this task builds the container they live in.

---

## Implementation Guidance

### Crate structure

The postretro-level-format crate already exists as a stub (created during workspace reorganization). All format types, section IDs, and read/write logic live here. Both the compiler (writes .prl) and the engine (reads .prl) depend on this crate.

### Header

```
magic: [u8; 4]      — b"PRL\0"
version: u16         — format version, starts at 1
section_count: u16   — number of sections in the file
```

8 bytes total. Reader validates magic and version before proceeding.

### Section table

Immediately follows the header. Each entry:

```
section_id: u32      — identifies the section type
offset: u64          — byte offset from file start to section data
size: u64            — byte length of section data
version: u16         — per-section version for independent evolution
```

22 bytes per entry, packed with no padding. Read and write each field individually using explicit little-endian conversions — do not cast a byte slice to a struct. Reader uses offset + size to seek directly to any section.

### Section IDs

Define an enum of known section types. Phase 1 populates:

| ID | Name | Populated by |
|----|------|-------------|
| 1 | Geometry | Task 04 |
| 2 | Cluster Visibility | Task 05 |

Reserve IDs for future sections (collision, light influence, light probes, nav mesh, audio, zones, spawns, textures) but don't implement their data types yet. Note: there is no BSP Tree section. The BSP tree is a compiler-internal intermediate — the format stores clusters, not BSP nodes.

### Read/write API

Provide functions to:
- Write a header + section table + raw section blobs to a writer (compiler uses this)
- Read a header + section table from a reader, returning section metadata (engine uses this)
- Read a specific section's raw bytes by section ID (engine uses this)

Section-specific deserialization is the consumer's responsibility. The format crate handles the container; each compilation stage defines its own section data types.

### Byte order

Little-endian throughout. Use explicit `to_le_bytes()` / `from_le_bytes()`. No platform-dependent behavior.

### Error handling

Define a format error type using thiserror. Variants for: invalid magic, unsupported version, truncated header, truncated section table, section offset out of bounds, section size exceeds file.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Magic bytes | `b"PRL\0"` — 4 bytes, null-terminated |
| Byte order | Little-endian, explicit conversion |
| Section table position | Fixed: immediately after 8-byte header |
| Section data position | Variable: referenced by offset in section table |
| Unknown section handling | Reader skips sections with unrecognized IDs |
| Missing section handling | Reader returns None for requested-but-absent section IDs |

---

## Acceptance Criteria

1. Format crate compiles and is depended on by both postretro and postretro-level-compiler.
2. Round-trip test: write a header + section table + dummy section blobs, read them back, all values match.
3. Reader rejects files with wrong magic bytes. Error message identifies the problem.
4. Reader rejects files with unsupported version. Error message includes the version found.
5. Reader skips unknown section IDs without error.
6. Reader returns None when a known section ID is absent from the file.
7. Section offsets and sizes are validated against file length — no silent out-of-bounds reads.
8. All integers are little-endian. Tests verify this explicitly (write known values, read raw bytes, confirm byte order).
