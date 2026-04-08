# Map Format Abstraction

> **Status:** ready
> **Depends on:** Phase 1.5 (PRL compiler baseline), Phase 1 (BSP loader)
> **Related:** `context/lib/build_pipeline.md` · `postretro-level-compiler/src/parse.rs` · `postretro/src/bsp.rs`

---

## Goal

Establish 1 unit = 1 meter as the engine canonical unit. Introduce a `--format` flag to the PRL compiler so that the coordinate transform and unit scale are explicit, format-specific decisions rather than hardcoded Quake assumptions. Update the BSP loader to apply the same scale so both loading paths agree on units. This makes it possible to support non-idTech map sources in the future — each format supplies its own transform.

---

## Scope

### In scope

- `MapFormat` enum with variants `IdTech2` (default), `IdTech3`, `IdTech4`
- `--format <FORMAT>` CLI flag; default `idtech2`
- `idtech_to_engine` transform: axis swizzle + 0.0254 uniform scale (Quake inches → meters)
- Scale applied to: positions, entity origins, plane distances
- Plane normals: swizzle only — unit vectors, no scale
- `IdTech3` and `IdTech4` variants: accepted by the CLI, fail fast with "not yet supported"
- Matching 0.0254 scale in the engine BSP loader (`bsp.rs`)
- `build_pipeline.md` updated to document canonical unit and multi-format intent

### Out of scope

- IdTech3 parser (no library; bezier patches, shader references)
- IdTech4 parser (brushDef3, mesh primitives)
- Any format other than idTech
- Detecting map format from file content or extension
- Engine gameplay constants (no player movement exists yet; nothing to update)

---

## Shared Context

### Canonical unit

Engine canonical unit: **1 unit = 1 meter**. All geometry in PRL sections and BSP vertex buffers is in meters after this change.

Quake maps are authored in Quake units where 1 unit ≈ 1 inch. Conversion factor: `0.0254` (exact: 1 inch = 0.0254 m).

### Transform invariant

The idTech-to-engine transform is: axis swizzle (Z-up → Y-up) + uniform scale (0.0254).

- **Positions and direction vectors:** swizzle + scale via `quake_to_engine`.
- **Plane normals:** swizzle only — unit vectors, scale does not apply.
- **Plane distances:** scale only. A plane `n·x = d` with `x` in Quake units becomes `n·x' = d * 0.0254` with `x'` in meters. This is the key correctness concern: prior code and docs explicitly said "do not transform distance fields" — that held for a pure rotation, but breaks for scale. Both the compiler and the BSP loader have distance handling that must be updated.

### Breaking change

Existing `.prl` files are in Quake units. After this change they are invalid — recompile required. Acceptable at current project stage.

---

## Task List

| ID | Task | File | Dependencies |
|----|------|------|-------------|
| 01 | MapFormat enum and CLI flag | `task-01-map-format-enum.md` | none |
| 02 | Unit conversion at PRL parse boundary | `task-02-unit-conversion.md` | 01 |
| 03 | BSP loader unit scale | `task-03-bsp-loader-scale.md` | none |

---

## Execution Order

| Wave | Tasks | Notes |
|------|-------|-------|
| Wave 1 (parallel) | 01, 03 | Different crates — zero file overlap |
| Wave 2 | 02 | Depends on 01; same parse.rs |

---

## Acceptance Criteria

1. `prl-build assets/maps/test.map` (no flag) produces a `.prl` with geometry in meters. A 64-unit Quake cube outputs vertices at 0 and 1.6256 m on its edges (64 × 0.0254).
2. `prl-build --format idtech2 test.map` is equivalent to the default.
3. `prl-build --format idtech3 test.map` exits with a clear "not yet supported" error.
4. `prl-build --format idtech4 test.map` exits with a clear "not yet supported" error.
5. `cargo run -- assets/maps/test.bsp` renders at the same scale as the compiled `.prl`.
6. `cargo test -p postretro-level-compiler` and `cargo test -p postretro` pass.
7. Plane distances are consistent with scaled vertex positions in both paths — geometry is not sheared.
