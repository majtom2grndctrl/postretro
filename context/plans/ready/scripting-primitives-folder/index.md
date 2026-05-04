# Scripting Primitives Folder Refactor

> **Status:** ready
> **Prerequisite for:** Grounded Movement (M7)
> **Scope:** pure refactor — no behavior change, no new primitives

---

> All paths are relative to `crates/postretro/src/` unless otherwise noted.

## Goal

Replace the flat `scripting/primitives.rs` + `scripting/primitives_light.rs` sibling pattern with a `scripting/primitives/` folder. Establishes the correct home for M7's collision and world primitive files before they're written.

---

## Tasks

### 1. Create `scripting/primitives/` and split existing files

- Create `crates/postretro/src/scripting/primitives/mod.rs` as the composition entry point. Move infrastructure symbols (`register_all`, `register_shared_types`, `register_world_query`) into `mod.rs`; it calls each domain file's registration function. Call sites converge on `primitives::register_all()`.
- Move entity-primitive content from `scripting/primitives.rs` → `scripting/primitives/entity.rs`. Infrastructure symbols move to `mod.rs` (see above), not `entity.rs`.
- Move `scripting/primitives_light.rs` → `scripting/primitives/light.rs`.
- Delete `scripting/primitives.rs` and `scripting/primitives_light.rs`.
- Fix all `use` paths and `mod` declarations across the crate — including `scripting/mod.rs` (`mod primitives` and `mod primitives_light`), all `use crate::scripting::primitives::*` and `use crate::scripting::primitives_light::*` call sites in the crate, and `src/bin/gen_script_types.rs`.
- Update intra-module references inside the moved files: four `super::primitives_light::*` call sites in `entity.rs` become `super::light::*`; update the `light.rs` doc comment that references `primitives.rs` to reference `entity.rs`.

### 2. Verify

- `cargo build -p postretro --bins` clean.
- `cargo test -p postretro` passes.
- No behavior change — this is a move only.

---

## Acceptance criteria

- `scripting/primitives/mod.rs`, `entity.rs`, `light.rs` exist; old flat files are gone.
- Build and tests pass.
- No new logic introduced.
