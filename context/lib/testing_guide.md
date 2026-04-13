# Testing Guide

> **Read this when:** writing new tests, deciding what to test, or reasoning about test strategy.
> **Key invariant:** tests document Postretro-specific behavior and cross-subsystem interactions — not language features or crate internals.
> **Related:** [Development Guide](./development_guide.md)

---

## 1. Test Targets

### Priority targets

| Category | Examples |
|----------|----------|
| Cross-subsystem interactions | Map loader producing data the renderer consumes, game events triggering audio, input state driving game logic |
| Data-driven behavior | Texture name prefix → material enum derivation, PRL section presence/absence shaping renderer paths |
| Boundary parsing | PRL loading, FGD entity parsing, config file parsing — anywhere external data enters the engine |
| Domain logic | BSP traversal ordering, lightmap atlas packing, collision detection, entity resolution from BSP leaves |
| Degradation paths | Missing optional PRL sections (lightmaps, PVS), malformed textures, absent assets |

### Decision criteria

Test it if **all** of these hold:
- Postretro-specific behavior (not a language feature or crate API)
- Crosses a boundary or shows how the system behaves at a seam
- Captures a real scenario or documents a contract for future readers

Skip it otherwise.

---

## 2. Exclusions

- **Standard library behavior.** Vec operations, string formatting, HashMap lookups. Rust's std is well-tested.
- **Language semantics.** Ownership transfers, pattern matching exhaustiveness, iterator chaining. The compiler enforces these.
- **Crate internals.** glam math correctness, kira playback mechanics, serde deserialization logic. Test that *we use them correctly*, not that they work.
- **Trivial type conversions.** `From`/`Into` impls that the compiler verifies at the type level.

---

## 3. Test Patterns

### Behavior over implementation

Assert observable outcomes: output data structures, subsystem responses, state transitions. Avoid asserting internal data structures that could change without affecting behavior.

### Real interaction flows

Model the actual data path: level load → parsed data → engine structs → renderer-ready buffers. Over-mocking hides the interactions tests exist to document.

### Seam-crossing tests

When testing code that bridges two subsystems, derive mock inputs from the source subsystem's actual output format and assertions from the destination subsystem's contract. If both come from the same mental model, the test proves the adapter is a passthrough — not that the passthrough is correct.

**Example:** testing that PRL face data becomes renderable geometry. The input should match what prl-build actually produces (vertex layout, face indices, texture metadata). The assertion should match what the renderer expects (correct vertex format, valid atlas UVs, proper winding order).

### Test naming

Names describe the exact behavior and boundary under test. Pattern: `<subject>_<verb>_<expected_outcome>`.

```rust
#[test]
fn loader_falls_back_to_white_lightmap_when_rgb_lump_missing() { ... }

#[test]
fn material_derivation_maps_metal_prefix_to_metal_enum() { ... }
```

### Regression tests

When a test is written in response to a bug, include a comment naming the bug it covers. One line: what broke, not how it was fixed.

```rust
// Regression: sector with zero-height floor caused divide-by-zero in span generation.
#[test]
fn span_generator_handles_zero_height_floor() { ... }
```

### Floating-point comparison

Game engine code is full of floating-point math (positions, UVs, colors, interpolation). Never assert exact equality on floats. Use approximate comparison with an explicit epsilon.

### Deterministic time

Game logic tests must control time. Inject a fixed delta time rather than reading wall-clock time. This makes tests reproducible and eliminates timing-dependent flakiness.

### No GPU context in tests

Tests run via `cargo test` with no window and no GPU context. The renderer's data-logic/GPU-interaction split ([Development Guide](./development_guide.md) §4.1) makes this a non-issue: data logic is testable as pure functions, and the thin GPU layer is verified by running the engine.

---

## 4. Test Organization

Rust convention: unit tests co-locate with source in `#[cfg(test)] mod tests` blocks at the bottom of the file. Integration tests live in `tests/` at the crate root.

| Location | Purpose |
|----------|---------|
| `mod tests` (in source file) | `#[test]` functions for the module's internal logic |
| `*_test_fixtures.rs` (sibling module) | Test infrastructure: geometry builders, struct constructors, shared helpers |
| `tests/` | Integration tests that exercise subsystem interactions across module boundaries |
| `tests/fixtures/` | Test data: minimal PRL files, texture samples, config files |

### Co-location rule of thumb

`#[test]` functions belong next to the code they verify. **Test infrastructure** — fixture builders, helpers, procedural data constructors — belongs in a separate `#[cfg(test)]` sibling module when large enough to obscure feature code or tests.

Extract into a sibling fixture module when:
- Helper/builder code exceeds ~200 lines
- Fixtures obscure `#[test]` functions
- Multiple test modules need the same builders

```
// Parent module declares the fixture module:
#[cfg(test)]
mod <module>_test_fixtures;

// <module>.rs — feature code + #[test] functions
// <module>_test_fixtures.rs — builders, helpers
```

Test files (both `mod tests` blocks and fixture modules) are exempt from the source file size guidance in [Development Guide](./development_guide.md) §2.1. Test suites are flat and linear — large is fine.

### Test fixtures

Prefer minimal, purpose-built test data over production assets. A test PRL should contain only the sections needed for the behavior under test — not a full level. Small fixtures keep tests fast and make the input-output relationship legible.

---

## 5. Non-Goals

- **Snapshot testing** — fragile, low signal. Prefer explicit assertions on specific fields.
- **100% coverage targets** — coverage is a tool, not a goal.
- **Testing crate behavior** — if glam's `Vec3::normalize()` is wrong, that's glam's problem.
- **Visual verification in CI** — rendering correctness is verified manually. Tests cover the logic that *produces* render data, not the rendered output.
- **Testing the GPU layer** — the thin GPU interaction layer is verified by running the engine, not by unit tests. See §3.
