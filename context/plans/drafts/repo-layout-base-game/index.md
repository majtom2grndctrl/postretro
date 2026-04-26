# Repo Layout: Base Game + Mods

## Goal

Reorganize the repo to make "base game" and "mod" content first-class alongside engine development. Establishes a content root convention, cleanly separates engine crates from game content, and gives modders a coherent SDK home.

## Scope

### In scope

- Move Rust workspace crates under `crates/`
- Introduce `content/` tree — `base/` for first-party game content, `tests/` for engine test fixtures
- Extend map-path → content root derivation to scripts loading (replace hardcoded `assets/scripts/`)
- Consolidate `sdk/types/` to a single copy at repo root; add `sdk/TrenchBroom/` as FGD generation target
- Reorganize `tools/`: external pre-compiled binaries to `tools/ext/`, automation scripts to `tools/scripts/`
- Update `.gitignore`, `CLAUDE.md`, and context docs to reflect new paths

### Out of scope

- Implementing `dist/` packaging automation (deployment bundle is a future step)
- Mod discovery, loading, or VFS mount ordering at runtime
- `mod.json` schema definition (deferred to mod-system plan)
- `pr-cli` tooling (new-mod, new-level, doctor scripts — deferred)
- TrenchBroom game config generation (`.games` file) — FGD output path wired up; game config install is a separate doc task

## Acceptance criteria

- [ ] `cargo build` succeeds with workspace members declared as `crates/postretro`, `crates/level-compiler`, `crates/level-format`, `crates/script-compiler`
- [ ] `cargo run -p postretro -- content/base/maps/test.prl` loads correctly; textures resolve from `content/base/textures/`, scripts from `content/base/scripts/`
- [ ] `cargo run -p postretro -- content/tests/maps/test-3.prl` also works (engine test fixtures accessible)
- [ ] `cargo run -p postretro` (no argument) still boots; default map path updated to point at `content/base/`
- [ ] No references to `assets/` remain in engine source or CLAUDE.md
- [ ] `sdk/types/` exists at repo root only — `crates/postretro/sdk/types/` directory removed; engine gen-types output path updated to `sdk/types/`
- [ ] `sdk/TrenchBroom/postretro.fgd` exists (moved from `assets/postretro.fgd`)
- [ ] `tools/ext/` contains external binaries (qbsp, vis, bspinfo, maputil, lightpreview.app, embree .dylib); `tools/README.md` updated
- [ ] `dist/` is listed in `.gitignore`

## Tasks

### Task 1: Move Rust crates to `crates/`

Move each workspace member to `crates/<short-name>`:

| Old path | New path |
|----------|----------|
| `postretro/` | `crates/postretro/` |
| `postretro-level-compiler/` | `crates/level-compiler/` |
| `postretro-level-format/` | `crates/level-format/` |
| `postretro-script-compiler/` | `crates/script-compiler/` |

Update `Cargo.toml` workspace `members` and the `postretro-level-format` path dependency. Update any intra-workspace `path = "postretro-level-format"` references in crate `Cargo.toml` files to the new path. Update CLAUDE.md build commands. Verify `cargo build --workspace` passes.

### Task 2: Reorganize content

Move game content from `assets/` to `content/`:

| Old path | New path | Notes |
|----------|----------|-------|
| `assets/maps/*.map` + `*.prl` | `content/base/maps/` | Game maps only — see below for test fixtures |
| `assets/textures/` | `content/base/textures/` | Full directory tree |
| `assets/scripts/` | `content/base/scripts/` | `.ts`, `.js`, `.luau`, `tsconfig.json` |
| `assets/maps/test-*.map` + `test_animated*` | `content/tests/maps/` | Engine test fixtures — not game content |
| `assets/maps/autosave/` | `content/tests/maps/autosave/` | |
| `assets/maps/gen_*.py` | `content/tests/maps/` | Map generation scripts stay with test maps |
| `assets/postretro.fgd` | `sdk/TrenchBroom/postretro.fgd` | |

Test-fixture map boundary: all maps in `assets/maps/` are test fixtures — including `campaign-test.map`. None go to `content/base/maps/`. The base game map directory starts empty and grows as authored content is added.

Delete `assets/` once all files are accounted for.

### Task 3: Content root abstraction in the engine

Currently `load_scripts()` in `crates/postretro/src/main.rs` hardcodes `Path::new("assets/scripts")`. Extend the content root derivation already present in `resolve_texture_root()` to produce a full content root:

- Given a map path `content/base/maps/e1m1.prl`, the content root is `content/base/` (parent of the `maps/` directory).
- `resolve_texture_root(map_path)` becomes `content_root_from_map(map_path).join("textures")`.
- `load_scripts()` accepts a `content_root: &Path` and loads from `content_root.join("scripts")`.
- Update `DEFAULT_MAP_PATH` to `content/base/maps/test.prl` (or the first available game map after Task 2).
- Update `tools/gen_specular.py` input examples/docs to reference `content/base/textures/`.

No new struct or abstraction needed — a single `content_root_from_map(map_path: &str) -> PathBuf` free function is sufficient. The convention (`maps/` lives one level below the content root) also applies to future mod content trees, so engine content loading will extend naturally to mods without further restructuring.

### Task 4: SDK consolidation

- Remove `crates/postretro/sdk/` — this is a duplicate of root `sdk/types/`.
- Update `crates/postretro/src/bin/gen_script_types.rs` output path from `postretro/sdk/types/` to `sdk/types/`. Use `concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/types")` — after the crate moves to `crates/postretro/`, two levels up reaches the workspace root.
- Create `sdk/TrenchBroom/` directory; `assets/postretro.fgd` moves there (covered by Task 2 above).
- Add `sdk/templates/` with a starter `tsconfig.json` (copy from `content/base/scripts/tsconfig.json`).

### Task 5: Tools reorganization

- Move external pre-compiled binaries and dylibs to `tools/ext/`: `qbsp`, `vis`, `bspinfo`, `maputil`, `bsputil`, `lightpreview.app`, `libembree4.4.dylib`, `libtbb.12.dylib`, `libtbbmalloc.2.dylib`, `LICENSE-embree.txt`, `gpl_v3.txt`.
- Keep `tools/gen_specular.py` and `tools/README.md` at `tools/` root.
- Create `tools/scripts/` as a placeholder for future automation scripts (`new-mod.sh`, etc.) with a brief README.
- Update `tools/README.md` to document `ext/` as external binaries not built from source.

### Task 6: Housekeeping

- `.gitignore`: add `dist/`; remove `target/` if not already present (add if missing).
- `CLAUDE.md`: update all build commands and path references (`assets/` → `content/`); update `prl-build` example to use `content/base/maps/` as output target.
- `context/lib/build_pipeline.md`: update texture authoring path (`textures/` is now under `content/<mod>/textures/`), update FGD location.
- `context/lib/resource_management.md`: update §1.1 authoring layout to reflect `content/base/textures/` convention.
- `context/lib/scripting.md`: update §8 hot reload path reference.
- `README.md`: update getting-started instructions.

## Sequencing

**Phase 1 (concurrent):** Tasks 1, 2, 4, 5 — all file moves; no cross-task file conflicts.
**Phase 2 (sequential):** Task 3 — engine code changes; depends on Tasks 1 (new crate path) and 2 (new content paths).
**Phase 3 (sequential):** Task 6 — doc/config cleanup; depends on everything settling.

## Rough sketch

`content_root_from_map` derivation (extends existing `resolve_texture_root` logic):

```rust
// Proposed design — remove after implementation
fn content_root_from_map(map_path: &str) -> PathBuf {
    Path::new(map_path)
        .parent()
        .and_then(|maps_dir| maps_dir.parent())
        .unwrap_or(Path::new("."))
        .to_path_buf()
}
```

This means any content tree that places maps under a `maps/` subdirectory gets correct texture and script resolution automatically — both engine content and future mods.

