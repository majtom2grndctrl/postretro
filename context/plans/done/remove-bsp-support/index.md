# Remove `.bsp` Legacy Map Support

> **Status:** ready
> **Depends on:** none. This is a simplification — no prerequisites land first.
> **Related:** `context/plans/done/map-format-abstraction/index.md` (ancestor: built the format-abstraction layer this plan collapses) · `postretro/src/bsp.rs` · `postretro/src/main.rs` · `postretro/src/visibility.rs` · `postretro/Cargo.toml` · `CLAUDE.md`

---

## Goal

Remove the `.bsp` loader, the `.bsp` visibility path, and the `qbsp` dependency. PRL becomes the sole runtime map format. Behavior on `.prl` maps is unchanged. The engine binary shrinks by ~1100 lines plus whatever `qbsp` pulls in.

---

## Context

PRL has parity for everything Postretro actually ships — the `.prl` compiler produces geometry, BSP tree, portals, and textures; portal traversal supersedes precomputed PVS at runtime. `assets/maps/` contains zero `.bsp` files; every map is a `.prl` compiled from a TrenchBroom `.map` source. Nothing to port forward.

The dual-path tax compounds on every future map-format feature. Lightmap delivery, entity resolution, material hooks, fog volumes — each new concern has had to exist twice or carry a "BSP path: TODO" note. Removing the legacy path now is cheaper than carrying it another release cycle.

Not-audited finding worth flagging: `TexturedVertex` and `TextureSubRange` live in `bsp.rs` today, and both `prl.rs` and `render.rs` import them from that module. A clean `rm bsp.rs` is not sufficient — those types must move first (or, equivalently, `bsp.rs` must be reduced to a "types-only" shim and then the shim moved). See Task 1.

---

## Approach

Three sequenced commits, each independently revertible:

- **Task 1** — Relocate shared vertex/texture types out of `bsp.rs`, then delete the BSP loader, the BSP visibility path, and the extension dispatch in `main.rs`. `qbsp` is still in `Cargo.toml` after this step (the dep graph still has live references until the loader module is gone, but removing it is its own commit).
- **Task 2** — Drop the `qbsp` dependency from `postretro/Cargo.toml`. Also drop the single `qbsp::BspData` call site in `texture.rs` (dead code behind `#[allow(dead_code)]`).
- **Task 3** — Unpin `glam` in the workspace `Cargo.toml`, update `CLAUDE.md` to drop the qbsp-compatibility note, and absorb any transitive-dep churn. Two `glam` versions currently coexist in the lockfile (0.30 and 0.32); the pin removal lets them converge on whichever upstream version the workspace consumers actually want.

Tasks 1 → 2 → 3 are strictly sequenced — each depends on the previous compiling cleanly. Task 4 (docs sweep) is parallelizable against 1–3 or can run after. Splitting the glam unpin from the BSP deletion matters because the unpin may ripple through transitive deps and shouldn't be tangled with the loader removal if something breaks.

```
T1 (delete loader) ───► T2 (drop qbsp) ───► T3 (unpin glam)
       │                      │                    │
       └──────────── T4 (docs sweep) ───────────────┘
                     (parallel to any of T1–T3)
```

---

## Tasks

### Task 1 — Delete BSP loader, BSP visibility path, and main.rs dispatch

**Crate:** `postretro` · **Files:** `src/bsp.rs`, `src/main.rs`, `src/visibility.rs`, `src/prl.rs`, `src/render.rs`

**Step 1 — Relocate shared vertex/texture types.** Before anything can be deleted, `TexturedVertex` (bsp.rs:21) and `TextureSubRange` (bsp.rs:87) must move somewhere format-agnostic. Two acceptable options:

1. Move both into a new `src/geometry.rs` module. Update `prl.rs:17`, `render.rs:11`, `render.rs:208`, `render.rs:212`, `render.rs:236`, `render.rs:539`, `render.rs:615`, `render.rs:1068–1069`, `render.rs:1132`, `render.rs:1137` to import from `crate::geometry` instead of `crate::bsp`.
2. Or: move both into `prl.rs` (the only non-test, non-renderer consumer) and have `render.rs` import from `crate::prl`. Simpler, but conflates format-neutral types with the PRL module.

Pick option 1 unless the implementer has a reason to prefer (2). `TexturedVertex::STRIDE` and the `#[repr(C)]` layout contract stay with it.

**Step 2 — Delete BSP loader.** With imports updated, delete `postretro/src/bsp.rs` entirely (1086 lines). This includes the in-file test module (~452 lines starting at line 635), the `DEFAULT_BSP_PATH` test constant (line 13), and the `resolve_bsp_path` test helper (line 390) — all of which vanish with the file. The module declaration `mod bsp;` in `main.rs:4` goes too.

**Step 3 — Delete BSP visibility path.** In `src/visibility.rs`:

- Delete `find_camera_leaf` (line 213), `decompress_pvs` (line 253), `CollectedFaces` (line 307), `collect_visible_faces` (line 339), and `determine_visibility` (line 399). The latter is the one called from `main.rs` for `Level::Bsp`; nothing in the PRL path touches any of these.
- Delete the `use crate::bsp::BspWorld;` import (line 8).
- Delete the `VisibilityPath::BspPvs` variant (line 64) — only `determine_visibility` constructs it, and `main.rs` reads it only in the `path_label` match arm (see Step 4).
- Delete the BSP test module. The `#[cfg(test)] mod tests` block at line 760 is a mix of BSP tests (lines ~760–1557) and PRL tests (lines ~1559–end). The BSP test helpers build `BspWorld` — they all go. The PRL sub-section at line 1557 stays; its `use crate::bsp::TexturedVertex` changes to `use crate::geometry::TexturedVertex` (matching Step 1).
- **Keep:** `FrustumPlane`, `Frustum`, `extract_frustum_planes`, `is_aabb_outside_frustum`, `DrawRange`, `VisibleFaces`, `VisibilityStats`, `VisibilityPath` (minus `BspPvs`), `raw_pvs_face_count`, `determine_prl_visibility`. All format-agnostic or PRL-specific.

**Step 4 — Collapse main.rs dispatch.** In `src/main.rs`:

- Delete `Level::Bsp` variant (line 40). `Level` becomes either a type alias for `prl::LevelWorld` or a single-variant enum; prefer just replacing `Option<Level>` with `Option<prl::LevelWorld>` throughout.
- Delete the `"bsp"` match arm in `load_level` (lines 73–89) and the fallback arm at 101–112 that also calls `bsp::load_bsp`. The unknown-extension case logs an error and returns `Ok(None)`.
- Delete `build_texture_names_from_face_meta` (line 190) — only the deleted BSP branch used it.
- Change `DEFAULT_MAP_PATH` (line 36) from `"assets/maps/test.bsp"` to `"assets/maps/test-3.prl"` (or whichever `.prl` is the canonical test map — check `assets/maps/` and pick the newest).
- Update the comment on `resolve_texture_root` (line 53) so the example path is a `.prl`, not a `.bsp`.
- In the geometry-build match at line 320 delete the `Level::Bsp` arm. If `Level` collapsed to a single PRL type, the `match` becomes an `if let`.
- In the visibility dispatch at line 525 delete the `Some(Level::Bsp(world)) => visibility::determine_visibility(...)` arm.
- In the `path_label` match at line 554, delete the `VisibilityPath::BspPvs => "bsp-pvs"` arm.

**Step 5 — `qbsp` still imported.** At this point `postretro/src/texture.rs:200` still has a dead `extract_texture_names(bsp: &qbsp::BspData)` function behind `#[allow(dead_code)]`. Leave it for Task 2 — this keeps Task 1 diff-contained to "remove the BSP code path" and Task 2 diff-contained to "remove the dep."

**Acceptance:**

- `cargo build -p postretro` succeeds.
- `cargo run -p postretro -- assets/maps/test-3.prl` (or whichever `.prl` the implementer picked for `DEFAULT_MAP_PATH`) launches and renders the map.
- `cargo run -p postretro` (no args) launches using `DEFAULT_MAP_PATH` and renders.
- `cargo test -p postretro` passes. Expect some BSP-specific tests to disappear — verify the PRL tests and the format-agnostic visibility tests (frustum planes, AABB-outside-frustum) still run.
- No `unsafe` introduced.
- `grep -rn "bsp::" postretro/src/` returns nothing (all `crate::bsp::` references gone).
- `grep -rn "BspWorld\|load_bsp\|BspLoadError" postretro/src/` returns nothing.

---

### Task 2 — Remove `qbsp` dependency

**Crate:** `postretro` · **Files:** `postretro/Cargo.toml`, `src/texture.rs`

**Step 1.** Delete `postretro/src/texture.rs:194–209` (the `extract_texture_names(bsp: &qbsp::BspData)` function and its section comment `// --- Texture name extraction from BSP ---`). This is the last `qbsp::` reference in the crate; it's currently `#[allow(dead_code)]` and has no callers.

**Step 2.** Delete `qbsp.workspace = true` from `postretro/Cargo.toml` (line 19) and the `# BSP loading` comment (line 18).

**Step 3.** Delete `qbsp = "0.14"` from the workspace `Cargo.toml` (line 25) and its `# BSP loading` comment (line 24).

**Step 4.** Run `cargo update` to regenerate `Cargo.lock` without the qbsp subtree. Commit the lockfile diff as part of this task.

**Acceptance:**

- `cargo build --workspace` succeeds.
- `cargo tree -p postretro | grep -c qbsp` returns `0`.
- `grep -n qbsp Cargo.lock` returns nothing.
- `cargo test --workspace` passes.

---

### Task 3 — Unpin `glam`

**Crate:** workspace · **Files:** `Cargo.toml`, `CLAUDE.md`

**Step 1.** In the workspace `Cargo.toml` (line 22), change `glam = "0.30"  # pinned for qbsp compatibility` to a range that tracks upstream. Use Cargo's default-caret form (e.g., `glam = "0.30"` without the pinning comment becomes functionally the same; the real change is dropping the pin intent). If a newer minor (0.31, 0.32) is desired, update the value now — note that two `glam` majors already coexist in `Cargo.lock` (0.30.x via postretro and 0.32.x via a transitive path), so bumping postretro to 0.32 lets the lockfile de-duplicate.

**Step 2.** Run `cargo update -p glam` and confirm the workspace builds. Inspect the lockfile diff: if transitive crates are also picking up new versions, list them in the commit message so reviewers can spot unintended churn.

**Step 3.** Update `CLAUDE.md` (line 14) — delete the bullet `**glam pinned to 0.30** for type compatibility with qbsp. Do not bump without checking qbsp's dependency.`

**Acceptance:**

- `cargo build --workspace` succeeds.
- `cargo test --workspace` passes.
- `cargo run -p postretro` launches and renders on the default map.
- `CLAUDE.md` no longer mentions glam pinning.
- If Task 3 causes transitive breakage that would delay shipping, revert this commit only — Tasks 1 and 2 stand on their own. Call this out in the commit message so the revert path is obvious.

---

### Task 4 — Documentation sweep

**Files:** `CLAUDE.md`, `context/lib/build_pipeline.md`, `context/lib/index.md`, any other `context/lib/*.md` with stale BSP legacy references.

Durable docs (`context/lib/*.md`) must not name functions or files — only describe what survives refactoring. Keep the edits conceptual.

**`CLAUDE.md`:**

- Line 20: delete the `cargo run -p postretro -- assets/maps/test.bsp  # engine with a BSP map` build example.
- Line 14: delete the glam-pin bullet (already handled by Task 3; mention here for the parallel-work case).

**`context/lib/build_pipeline.md`:**

- Line 4 (Key invariant): reword. Current: `PRL is the primary compilation target; BSP loading remains for legacy asset support.` New: `PRL is the sole runtime map format.`
- Line 19 (`**PRL path (primary):**` bullet): drop the `(primary)` qualifier — there is no longer a secondary path to distinguish it from. The rest of the sentence (BSP tree, portal geometry, `postretro-level-format` crate) stays.
- Line 21: delete the entire `**BSP path (legacy support):**` bullet.
- Line 23: reword `Both paths share the TrenchBroom authoring workflow, FGD entity definitions, and PNG texture pipeline.` → `The PRL path uses the TrenchBroom authoring workflow, FGD entity definitions, and PNG texture pipeline.` (or delete outright — with only one path, "share" no longer makes sense).
- Lines 27–31: delete the entire `## BSP (Legacy Support)` section.
- Lines 43–45 (PNG texture pipeline table): the rows `| qbsp | ... |` and `| BSP output | ... |` describe the legacy flow. Replace with the PRL equivalent: `prl-build` reads PNGs for dimensions; texture metadata lives in the `.prl` TextureNames section (ID 16).
- Preserve every other BSP reference in this file — they describe the BSP *tree* that PRL bakes (`BspNodes`/`BspLeaves` sections, leaf-based visibility, brush-volume BSP construction). Those stay.

**`context/lib/index.md`:**

- Line 13: reword the router row `Rendering pipeline / BSP / lighting / BSPX → rendering_pipeline.md` — the "BSPX" token is Quake-ericw-tools specific and becomes dead after this plan. Reword to `Rendering pipeline / lighting → rendering_pipeline.md`.
- Line 14: reword `PRL format / level compiler / BSP / runtime portal vis → build_pipeline.md §PRL` — the middle `BSP` token referred to the legacy section. Reword to `PRL format / level compiler / runtime portal vis → build_pipeline.md`.
- Lines 50–52: delete the `BSP path (legacy support)` paragraph entirely. The preceding `PRL path (primary)` paragraph becomes the only path.

**`context/lib/rendering_pipeline.md`:**

- Line 53: delete the `**Status: deprecated.** .bsp runtime support exists for development against legacy ericw-tools-compiled maps...` paragraph. After this plan lands, BSP runtime support does not exist at all.
- Line 55: delete the paragraph about `.bsp` files carrying precomputed PVS only — there is no `.bsp` runtime path to describe.
- Line 73: delete the `**Deprecated path: BSP.** Loaded via the qbsp crate...` paragraph in the loader section.
- Line 89: reword the Phase 4 note — drop `not via BSPX lumps`. BSPX is a Quake-ericw concept and becomes dead terminology; Phase 4 simply bakes lighting into PRL via `prl-build`.
- Lines 168–176 (`### BSP loader produces` sub-section and its table): rewrite as `### Map loader produces` describing the PRL runtime load. The output contract (vertex buffer sorted by leaf/texture, loaded textures, per-face metadata, per-leaf sub-ranges) is format-agnostic and carries over; only the header and any `BSP`-prefixed column text needs adjusting.
- Line 189 (`## Boundary rule`): reword `BSP loader, game logic, audio, and input never import wgpu types` to `map loader, game logic, audio, and input never import wgpu types` — the module's role is unchanged, but its name in this doc should stop referencing a format.
- Line 223: delete `ericw-tools for BSP, ` from the `**Runtime level compilation**` bullet. Only `prl-build` remains as the compiler the engine consumes.

**`context/lib/development_guide.md`:**

- Line 27: rewrite the `Math | glam 0.30 (pinned — qbsp compatibility...)` dep-table row. New text: `Math | glam`. The pin rationale goes away with Task 3.
- Line 28: delete the entire `| BSP loading | qbsp 0.14 |` dep-table row.
- Line 179: reword the `Option<T>` example from `optional BSPX lump` to any PRL-native optional — e.g., `optional lightmap section` or `optional texture animation data`. BSPX is Quake-ericw terminology and no longer appears in the engine's vocabulary.
- Line 193: delete `qbsp` from the crate stack list in the `unsafe` note. New text: `The crate stack (wgpu, winit, kira, gilrs, glam) provides safe APIs...`.

**Other `context/lib/*.md`:**

- `resource_management.md`, `audio.md`, `entity_model.md`, `testing_guide.md` — grep each for `\.bsp\b`, `qbsp`, `BSPX`, `ericw`, and `BSP loader`. Any reference to the runtime BSP loader, qbsp, or BSPX as a shipping concern gets rewritten or deleted. Any reference to the BSP *tree* inside PRL stays (it's load-bearing architecture). Expect most files to need one or two small edits.

**Acceptance:**

- `grep -rn "\.bsp\b" context/lib/` returns only the BSP-tree references (grep for `BSP` tells you these; the literal `.bsp` extension should appear nowhere after this pass).
- `grep -rn "qbsp\|BSPX\|ericw" context/lib/` returns nothing (or only historical / non-load-bearing mentions with explicit "formerly" framing).
- `CLAUDE.md` has no `.bsp` build example and no glam-pin bullet.
- The index.md router has no BSP-legacy entries.
- No file in `context/plans/done/` is modified — history stays.

---

### Task 5 (optional) — Orphan script cleanup

**Files:** `assets/maps/` scripts and logs, `.gitignore`, any shell helpers.

`assets/maps/` contains `gen_test_map.py`, `gen_test_map_3.py`, `gen_test_map_4.py`, assorted `*.log` / `*.prt` / `*.texinfo.json` files, and the existing `.map`/`.prl` sources. Spot-check each script and log for `.bsp` references:

- If a script runs `qbsp` or `vis` or `light` (ericw-tools), it's legacy — either delete it or rewrite to call `prl-build`. Judgment call: keep if it still produces useful `.prl` output via `prl-build`, delete if it's pure ericw-tools.
- Stale `*.log` files from old BSP compiles can be deleted or left alone — they're not load-bearing.

Optional because none of these affect `cargo build`. Run after Task 4 if time permits; defer indefinitely otherwise.

**Acceptance:** none required; this is a tidiness pass.

---

## Sequencing

```
Task 1: delete BSP loader + visibility path + main.rs dispatch
   │
   ▼
Task 2: remove qbsp dependency (and dead texture.rs extractor)
   │
   ▼
Task 3: unpin glam, update CLAUDE.md glam bullet
   │
   ▼ (independently revertible — keeps T1+T2 if T3 ripples)

Task 4: docs sweep — parallel to any of T1/T2/T3, or after
Task 5: optional orphan script cleanup — anytime after T4
```

Task 1 is the largest diff and the one most likely to surface surprises (the `TexturedVertex`/`TextureSubRange` relocation wasn't in the audit). Tasks 2 and 3 are small and mechanical once Task 1 lands. Task 4 runs against durable docs and shares no files with code tasks; it can run in parallel.

---

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| `TexturedVertex` / `TextureSubRange` relocation touches more files than the audit suggested, cascading into render.rs and the tests. | Task 1 Step 1 is isolated: the type move is a single atomic refactor before any deletion. If the move fails, nothing else has changed. If it succeeds, the rest of Task 1 is pure deletion. |
| `glam` unpin triggers transitive-dep cascades that break other crates. | Task 3 is a separate commit. Revert is one `git revert` — Tasks 1 and 2 stand without it. |
| A developer has a local `.bsp` map (not in the repo) and expects to load it. | This is the point of the plan. Users recompile from the `.map` source via `prl-build`. Document the migration in the commit message for Task 1. |
| Hidden BSP reference the audit (and this plan's spot-check) both missed — a test fixture, an example binary, a script. | The grep-based acceptance criteria on Task 1 (`grep -rn "bsp::" postretro/src/`) and Task 2 (`grep -n qbsp Cargo.lock`) catch any live reference before each task commits. |
| `DEFAULT_MAP_PATH` points at a `.prl` that an individual contributor hasn't generated yet. | The existing no-args launch path already handles "file not found" gracefully (logs a warning, starts without a map). Worst case: `cargo run` launches an empty scene until the contributor runs `prl-build`. |

---

## Acceptance Criteria

1. `cargo build --workspace` succeeds after each task commits.
2. `cargo test --workspace` passes after each task commits.
3. `cargo run -p postretro` launches on the default map (a `.prl`) and renders.
4. `cargo run -p postretro -- <any .prl>` launches and renders.
5. `cargo run -p postretro -- foo.bsp` exits with an error or launches empty (unknown-extension path logs and returns `Ok(None)`); whichever the implementer picked, it is consistent with the "unknown extension" handling for any non-`.prl` file.
6. `cargo tree -p postretro | grep qbsp` returns nothing.
7. `grep -n "qbsp\|BspWorld\|load_bsp" postretro/src/` returns nothing.
8. `Cargo.lock` has no `qbsp` entry.
9. `glam` in workspace `Cargo.toml` has no qbsp-compatibility pin comment.
10. `CLAUDE.md` has no `.bsp` build example and no glam-pin bullet.
11. `context/lib/build_pipeline.md` has no `BSP (Legacy Support)` section and no mention of qbsp in the PNG pipeline table.
12. `context/lib/index.md` has no BSP-legacy router entries.
13. No `unsafe` introduced.

---

## Out of Scope

- Changing the PRL format (section layout, compression, versioning).
- Adding any new visibility feature or renderer capability.
- Revisiting the shape of `VisibilityPath`, `VisibleFaces`, or `VisibilityStats` beyond removing the `BspPvs` variant.
- Rewriting any plan under `context/plans/done/` — they remain as historical record.
- Deleting `qbsp`-format tooling from outside the `postretro` crate (the `postretro-level-compiler` crate does not depend on `qbsp` and is untouched by this plan).
- Updating ericw-tools build scripts, game configurations, or authoring docs — TrenchBroom authoring is not affected, only the engine's runtime loader.
- Replacing the `TexturedVertex` layout or making it `#[derive(bytemuck::Pod)]`-friendly. The `#[repr(C)]` + manual `STRIDE` pattern is preserved across the relocation.

---

## Open Questions

None. The only judgment call — whether to relocate `TexturedVertex`/`TextureSubRange` into a new `geometry.rs` module or fold them into `prl.rs` — is flagged in Task 1 Step 1 with a recommended default (new module). The implementer may deviate if the workspace evolves between draft and implementation.
