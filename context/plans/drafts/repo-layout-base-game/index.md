# Repo Layout: Base Game + Mods

## Goal

Reorganize the repo to make "base game" and "mod" content first-class alongside engine development. Establishes a content root convention, cleanly separates engine crates from game content, and gives modders a coherent SDK home.

## Scope

### In scope

- Move Rust workspace crates under `crates/`
- Introduce `content/` tree — `base/` for first-party game content, `tests/` for engine test fixtures
- Extend map-path → content root derivation to scripts loading (replace hardcoded `assets/scripts/`)
- Consolidate `sdk/types/` to a single copy at repo root; add `sdk/lib/` for core modder-facing TS/Luau libraries; add `sdk/TrenchBroom/` as FGD generation target
- Clean up `tools/`: remove all ericw-tools binaries and docs (unused); keep `gen_specular.py`; add `tools/scripts/` placeholder
- Update `.gitignore`, `CLAUDE.md`, and context docs to reflect new paths

### Out of scope

- Implementing `dist/` packaging automation (deployment bundle is a future step)
- Mod discovery, loading, or VFS mount ordering at runtime
- `mod.json` schema definition (deferred to mod-system plan)
- `pr-cli` tooling (new-mod, new-level, doctor scripts — deferred)
- TrenchBroom game config generation (`.games` file) — FGD output path wired up; game config install is a separate doc task

## Acceptance criteria

- [ ] `cargo build` succeeds with workspace members declared as `crates/postretro`, `crates/level-compiler`, `crates/level-format`, `crates/script-compiler`
- [ ] `cargo test --workspace` passes — level-compiler fixture tests find assets under `content/tests/`
- [ ] `cargo run -p postretro -- content/tests/maps/test-3.prl` loads correctly; textures resolve from `content/tests/textures/`, scripts from `content/tests/scripts/` (or absent — engine tolerates missing scripts dir)
- [ ] `cargo run -p postretro` (no argument) still boots; default map path updated to `content/tests/maps/test-3.prl`
- [ ] No references to `assets/` remain in engine source or CLAUDE.md
- [ ] `sdk/types/` exists at repo root only — `crates/postretro/sdk/types/` directory removed; engine gen-types output path updated to `sdk/types/`
- [ ] `sdk/TrenchBroom/postretro.fgd` exists (moved from `assets/postretro.fgd`)
- [ ] All ericw-tools binaries and docs removed from `tools/`; `tools/gen_specular.py` and `tools/README.md` remain; `tools/scripts/` placeholder exists
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
| `assets/maps/*.map` + `*.prl` + sidecar files | `content/tests/maps/` | All existing maps are test fixtures |
| `assets/maps/gen_*.py` | `content/tests/maps/` | Map generation scripts stay with test maps |
| `assets/textures/` | `content/tests/textures/` | All existing textures are test fixtures |
| `assets/scripts/*.ts` + `*.js` + `*.luaurc` + `tsconfig.json` | `content/tests/scripts/` | Game behavior scripts — test fixtures |
| `assets/scripts/sdk/` | `sdk/lib/` | Core modder-facing TS/Luau libraries |
| `assets/postretro.fgd` | `sdk/TrenchBroom/postretro.fgd` | |

All maps, textures, and scripts in `assets/` are test fixtures. `content/base/` starts empty — maps, textures, and scripts grow there as first-party game content is authored.

TrenchBroom autosave directories (`autosave/` under any maps folder) are not migrated — add `autosave/` to `.gitignore` instead.

Delete `assets/` once all files are accounted for.

### Task 3: Content root abstraction in the engine

Currently `load_scripts()` in `crates/postretro/src/main.rs` hardcodes `Path::new("assets/scripts")`. Extend the content root derivation already present in `resolve_texture_root()` to produce a full content root:

- Given a map path `content/base/maps/e1m1.prl`, the content root is `content/base/` (parent of the `maps/` directory).
- `resolve_texture_root(map_path)` becomes `content_root_from_map(map_path).join("textures")`.
- `load_scripts()` accepts a `content_root: &Path` and loads from `content_root.join("scripts")`.
- Update `DEFAULT_MAP_PATH` to `content/tests/maps/test-3.prl` — base starts empty so the default points at a test fixture.

Note: `resolve_texture_root` is currently called in two places in `main.rs` — once during PRL load (around line 119) and again during smoke/volumetric collection setup (around line 567), where the second site re-derives the path by re-parsing `std::env::args()`. The content root should be derived once at level load and stored (or passed through), not re-derived on each call site. Both `resolve_texture_root` usages and `load_scripts` must read from the single stored content root. Suggested approach: derive the content root `PathBuf` from the map path early in the startup/level-load flow and thread it into `load_scripts` and the texture loading sites (e.g., as a field on `App` or as an explicit parameter).

No new struct or abstraction needed — a single `content_root_from_map(map_path: &str) -> PathBuf` free function is sufficient. The convention (`maps/` lives one level below the content root) also applies to future mod content trees, so engine content loading will extend naturally to mods without further restructuring.

### Task 4: SDK consolidation

- Remove `crates/postretro/sdk/` — this is a duplicate of root `sdk/types/`.
- The current default in `gen_script_types.rs` is already `PathBuf::from("sdk/types")` (CWD-relative). No code change is required; the only work here is deleting the stale `postretro/sdk/types/` directory. Optionally, switch to a workspace-root-anchored path (`concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/types")`) for CWD-independence — this is a quality-of-life improvement, not a correctness fix.
- Create `sdk/TrenchBroom/` directory; `assets/postretro.fgd` moves there (covered by Task 2 above).
- Create `sdk/lib/` and move `assets/scripts/sdk/` contents there (covered by Task 2 above): `light_animation.ts`, `light_animation.luau`, `world.ts`, `world.luau`.
- Add `sdk/templates/` with a starter `tsconfig.json` (copy from `content/tests/scripts/tsconfig.json`).

### Task 5: Tools cleanup

The `tools/` directory currently contains ericw-tools pre-compiled binaries (`light`, `qbsp`, `vis`, `bspinfo`, `maputil`, `bsputil`, `lightpreview.app`), embree/TBB dylibs (`libembree4.4.dylib`, `libtbb.12.dylib`, `libtbbmalloc.2.dylib`), license files (`LICENSE-embree.txt`, `gpl_v3.txt`), and a `doc/` tree of Sphinx HTML for ericw-tools. These tools are not used — the project's own `prl-build` crate handles all map compilation. Delete all of them.

Retain:
- `tools/gen_specular.py` — actively used specular map generation script
- `tools/README.md` — update to remove ericw-tools documentation and describe `gen_specular.py` and `scripts/` only

Create `tools/scripts/` as a placeholder for future automation scripts (`new-mod.sh`, `new-level.sh`, etc.) with a brief README.

Also remove the `/tools` entry from `.gitignore` (it was added to ignore the large tracked binaries; once the binaries are gone, `tools/` has no gitignore-worthy content).

### Task 6: Housekeeping

- `.gitignore`: add `dist/`, `autosave/`; ensure `target/` is present; remove `postretro/sdk/` entry (stale after Task 4 deletes the directory); remove `/tools` entry (was suppressing the large ericw-tools binaries; once deleted in Task 5, `tools/` needs no ignore rule); add `sdk/types/` if generated types should remain untracked.
- `CLAUDE.md`: update all build commands and path references (`assets/` → `content/`); update `prl-build` example to use `content/base/maps/` as output target.
- Update `postretro-level-compiler/` source and test files: audit `src/` and `tests/` for hardcoded `assets/maps/` and `assets/textures/` paths (doc comments, CLI `--help` examples, test fixture path helpers — confirmed call sites include `src/main.rs`, `src/parse.rs`, `src/partition.rs`, `src/portals.rs`, `src/geometry.rs`, `src/pack.rs`, and `tests/animated_weight_maps_fixtures.rs`) and update them to `content/tests/maps/` and `content/tests/textures/` respectively. Run `cargo test --workspace` to verify fixtures pass after the move.
- `context/lib/build_pipeline.md`: update texture authoring path (`textures/` is now under `content/<mod>/textures/`), update FGD location.
- `context/lib/resource_management.md`: update §1.1 authoring layout to reflect `content/<mod>/textures/` convention.
- `context/lib/scripting.md`: update §7 SDK type path references — `postretro/sdk/types/postretro.d.ts` and `postretro/sdk/types/postretro.d.luau` should become `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau`.
- `README.md`: update getting-started instructions.

## Sequencing

**Phase 1:** Tasks 2 and 5 run concurrently with Task 1. Task 4's SDK directory deletion has a sequencing dependency with Task 1 because both touch the `postretro/sdk/` directory (Task 1 moves the entire `postretro/` crate, including `sdk/`, to `crates/postretro/`; Task 4 deletes that duplicate `sdk/` tree). To avoid one agent carrying the duplicate forward while the other tries to delete it, run Task 4's SDK deletion *just before* Task 1 — delete `postretro/sdk/` at its current path so nothing is carried into `crates/postretro/sdk/` (cleaner than deleting after the move). The remainder of Task 4 (creating `sdk/TrenchBroom/`, `sdk/lib/`, `sdk/templates/`, optional `gen_script_types.rs` path tweak) is independent and may run concurrently with Tasks 1, 2, and 5.
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

