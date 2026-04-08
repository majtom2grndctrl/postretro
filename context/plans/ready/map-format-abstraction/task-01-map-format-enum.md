# Task 01: MapFormat Enum and CLI Flag

**Crate:** `postretro-level-compiler`
**Files:** `src/main.rs`, `src/parse.rs`, new `src/map_format.rs`
**Depends on:** nothing

---

## Context

The PRL compiler's coordinate transform is hardcoded in `parse.rs`. It assumes idTech/Quake input with no way to override. A `--format` flag makes this assumption explicit and routes to format-specific behavior. `MapFormat` owns the transform logic so future formats can supply different axis orientations, scale factors, and feature sets (bezier patches, meshDef, etc.) without touching shared compiler stages.

This task adds the scaffolding only. The transform itself (unit scale) is Task 02.

---

## What to Change

Add `src/map_format.rs`:

```rust
// Proposed design — remove after implementation

pub enum MapFormat {
    IdTech2,  // Quake 1 + Quake 2 (default)
    IdTech3,  // Quake 3: bezier patches — not yet supported
    IdTech4,  // Doom 3: meshDef, brushDef3 — not yet supported
}

pub const DEFAULT_MAP_FORMAT: MapFormat = MapFormat::IdTech2;

impl MapFormat {
    pub fn from_str(s: &str) -> Result<Self, String> { ... }

    /// False for variants whose parsers are not yet implemented.
    /// Compiler aborts early with a clear error rather than silently ignoring
    /// format-specific features.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::IdTech2)
    }
}
```

In `src/main.rs`:
- Add `--format <FORMAT>` argument, default `"idtech2"`.
- Parse via `MapFormat::from_str`; propagate unknown-format error.
- Call `format.is_supported()` before `parse_map_file`; if false, `anyhow::bail!("map format '{}' is not yet supported", format_str)`.

In `src/parse.rs`:
- Accept `MapFormat` as a parameter to `parse_map_file`. No behavioral change yet — Task 02 uses it to drive the scale.

---

## Acceptance Criteria

- `prl-build test.map` uses `idtech2` by default (no flag required).
- `prl-build --format idtech2 test.map` compiles successfully.
- `prl-build --format idtech3 test.map` exits with a "not yet supported" error and non-zero exit code.
- `prl-build --format idtech4 test.map` same.
- `prl-build --format bogus test.map` exits with an "unknown map format" error and non-zero exit code.
- `cargo test -p postretro-level-compiler` passes.
