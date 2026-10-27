# Review Panel Summary — round 2

**Target:** coloured-sh-bounce, `3275d1480..fea054c18` (round-1 fixes included).
**Panel:** 3 slices + seam pass + acceptance-proof audit. Correctness tracers ×3, adversarial testers ×2, contract verifier ×1, AC auditor ×1 (Opus); hygiene/drift ×3 (Sonnet). Independent of round 1 (review.md not read).
**Verdict:** request changes. Production lighting flow, keys, gate order, fluid tail and indirect scale verified correct by every depth lens. Blocking: stale tests that fail `cargo test`, clippy errors, one unproven AC row. Should-fix: one seam three reviewers flagged independently, a sidecar-repair hole, test-proof durability, decomposition.

## 🔴 Must fix

- **R1 Stale stage-order tests** (seam tracer, hygiene A). `ORDERED_STAGES` is now 29 with TextureValidation + Materials after NavMesh. Stale: `pipeline/tests.rs:434-470` `planned_stage_contract_pins_order_labels_and_sdf_prediction` (asserts 28, TexValidation second); `tui/tui_steps.rs:398` `p1_overlapping_actives_open_lighting_and_show_both` and `:424-426` `collapsed_and_expanded_headers_always_show_summaries` ("Parse 0/2", "Lighting 0/7"); `tests/compiler_cli_contract.rs:14-45` `SUMMARY_LABELS` (ignored). TUI grouping: TextureValidation sits under SH_LIGHTING_STAGES only because it moved — name or comment why.
- **R2 Clippy `-D warnings`**: `texture_mips/world.rs:47` type_complexity; `surface_materials/stats.rs:113` chunks_exact→as_chunks; `sh_group.rs:1896-1906` `bounce_material_hash` placed after `mod tests` (items_after_test_module). Clippy stops at first failing crate; rerun until clean.
- **R3 AC28 unproven** (AC auditor). No test for "no level-compiler code writes stats to disk" and none listing build-cache entries after cold + warm builds. Grep gates (research.md §Grep gates) for AC30 (one mean computation: `build_srgb_to_linear_lut` callers only in texture_mips/ and surface_materials/) and AC33 (no `bounce`/`albedo`/`saturation` knob in worldspawn keys, FGD, prl-build args — FGD needs an allowlist for existing fog `saturation(float)` fields) are claimed, not tested.

## 🟡 Should fix

- **Y1 Saturation has two owners** (estimator tracer, materials tracer, keys adversary — 3×). Albedo pre-blended in `MaterialStats::from_slots(…, saturation)` (`stats.rs:50`) via `bake_materials_into(Some(s))`; key folds `BounceMaterials.saturation` (`stats.rs:166`) passed separately (`texture_stage.rs:26,31`). A mismatch = key/byte disagreement. Fix: `MaterialStats` keeps `snapped_mean` (+ whatever glow-independent data future consumers need) only; `BounceMaterials::new(stats, s)` derives albedo with its own saturation; baker takes a collect-stats flag. This is the seam surface-radiance-cache patch albedo builds on.
- **Y2 `None` material window and inconsistent `None` keying** (materials tracer, estimator tracer, keys adversary, seam tracer). `pipeline.rs:727` builds `ShBakeCtx{bounce_materials: None}`, shadowed at `:831`; a future stage in the window silently bakes grey. Group key folds `BOUNCE_SATURATION` when `None` (`sh_group.rs:440-445`), delta key folds nothing (`delta_sh_cache.rs:81-83`). Fix: build the SH ctx once after Materials (or make the empty table the production grey); make both keys fold the same presence-tagged encoding.
- **Y3 Sidecar repair misses dropped slots** (adversary A). A parser-valid, hash-matching sidecar whose DIFFUSE or EMISSIVE slot is absent while the source exists passes `cache_entry_has_valid_declared_slots`; `from_slots` returns Ok with the wrong mean (strip included, or grey). Fix on the stats hit path: DIFFUSE/EMISSIVE presence must match source presence, else rebake (not NORMAL — legitimately dropped below 4×4, `world.rs:349-354`). Log (debug) the reason whenever a cached entry is rejected for stats.
- **Y4 Stats/index alignment by convention** (2×). Add `ensure!(stats.len() == names.len())`; `debug_assert!` that `texture_names` only grew before the fluid tail slice (`texture_stage.rs:46`).
- **Y5 Decompose `texture_mips/world.rs` (995 lines)**: inline tests ~570 lines → `texture_mips/world_tests.rs`; ~10 interleaved `#[cfg(test)]` statements in the production loop → a zero-cost lifetime/metrics helper (no-op in non-test). `texture_mips/metrics.rs` naming vs `*_tests.rs`/`*_test_fixtures.rs` precedent.
- **Y6 Test proofs that die at landing** (hygiene B, AC auditor). Empty-case and oracle CLI tests hard-code `../../work/ab/grey/prl-build` (untracked; gone after landing) and `/usr/bin/shasum` (macOS-only). Old-compiler comparisons are one-shot pre-change oracles: resolve the old compiler only from an env var, hash in Rust (blake3/sha2 already available?), skip with a clear message when unset rather than panic, and say so in the test doc. All CLI proofs are `#[ignore]`: preflight must run them with `--ignored`.
- **Y7 Duplicated CLI harness** across `tests/coloured_bounce_{oracle,empty,cache}.rs` (args, tempdir, section reader, Fixture/Bake structs) and `box_brush` duplicated in `coloured_bounce_test_fixtures.rs:89` / `coloured_bounce_empty.rs:335`. Move into the shared fixtures module.
- **Y8 Weakened AC proofs** (AC auditor): AC10 one coloured texture only; AC12 restates Vec indexing — bake two different name lists through the material stage and compare the shared texture's mean; AC14 no `neon_` texture without `_e`; AC21 group stage legacy miss rests on a constant assert — prove version reaches the key; AC23 no CLI case for mover decode failure, `_e` dimension mismatch ordering, fluid-only `_e` mismatch; AC24 gate case does not assert texture validation absent; AC25 no pipeline fixture with a name shared by world and fluid faces; AC29 all 32 fixture textures share content — exercise distinct rebakes.
- **Y9 Renderer indirect-scale proof** (contract verifier, hygiene C). GPU readback `uploads/renderer_tests/indirect_scale.rs:61` is `#[ignore]` + `expect` on adapter, unlike peers (`let Some(..) = renderer() else { return }`); and `read_receiver_scales` calls `write_light_params` itself, so it can't catch a call site passing 1.0. Drop the ignore, use the skip pattern, drive the real frame path if feasible. The source-text test in `pipeline_budget_tests.rs:626-653` pins the setter's exact text (`value.clamp(0.0, 1.0)`) — keep only the call-site check, move it beside the receiver tests.
- **Y10 Per-entry info logging** `delta_sh_cache.rs:196,202` — one line per (cell, light): floods large maps. Lower to debug; tests that parse it set RUST_LOG.
- **Y11 docs/level_design.md:112** — define "renders as glow" for authors (`_e` non-zero RGB on a material whose prefix gives emissive strength, e.g. `neon_`), state the grey fallback plainly, say movers/fluids/models are not bounce surfaces.

## 🟢 Nits

- G1 fold set includes zero-index-count faces: harmless over-keying, note it in the doc.
- `BounceMaterials::albedo` only used by a test — `#[cfg(test)]` or move.
- Production `bounce_materials: None` sites (direct, animated-direct, scatter) — one comment saying they're intentionally colour-blind.
- Three test-mounting styles (`#[path] mod` child, `include!`, plain mod) — prefer plain `#[cfg(test)] mod`.
- `bake_world_texture_mips` is pub but only test callers — doc it as the single-call test oracle (or gate).
- `derive_material` prefix warning now emitted from three callers.
- Renderer: Rust `_pad` vs WGSL `_pad2`; `mesh_pass.rs:2211` `to_ne_bytes` vs neighbours' `to_le_bytes`; kinematic `write_light_params` lacks the mesh one-line scale doc; redundant `*_follow_indirect_scale_changes` serializer tests; shader guard string-match looseness.
- `-0.0` saturation folds differently from `0.0` (spurious miss only).
- Fluid tail re-walks texture root and rebuilds the LUT (negligible).
- Pre-existing `texture_mips.rs:64-65` "polynomial" wording.

## Comment / doc drift

- 🟡 Movable plan-dir headers → `context/lib/build_pipeline.md §Compiler pipeline`: `surface_materials/stats.rs:2`, `coloured_bounce_test_fixtures.rs:2`, `sh_bake/colour_tests.rs:2`, `sh_bake/colour_key_tests.rs:2`, `sh_coarsen/colour_reference_tests.rs:2`, `tests/coloured_bounce_cache.rs:2`, `tests/coloured_bounce_oracle.rs:2`. `sh_bake/empty_material_tests.rs:1-3` breaks the two-line header format.
- 🟡 `texture_mips/world.rs:7-21` doc on `bake_world_texture_mips` describes the pre-split TextureMips stage.
- 🟡 `pipeline/texture_stage.rs` final `log_texture_byte_summary`: restore the deleted "why" (flush once after world, fluid, model, sprite are all accounted).
- 🟡 `surface_materials.rs` still probe-specific: `[ReflectionProbes]` warn tags (~197-211), "first time a capture ray reaches it" (31-34). Re-scope to shared, or state only probes read footprints here.
- 🟡 Indirect-scale docs now stale: `debug_ui/mod.rs:157` ("controls the static-surface indirect"), `forward.wgsl:17-19`, `billboard.wgsl:11-13` (group-0 fields list omits indirect_scale), `rendering_pipeline.md:990` (b4 params fields), `level-compiler/src/reflection_probes/shading.rs:16-18` ("forward and fluid passes multiply"), `mesh_pass.rs:173-187` struct doc.
- 🟢 `sh_bake.rs:52-53` BOUNCE_ALBEDO doc → "fallback when a hit has no resolved diffuse mean"; `bounce.rs` grey arm: say it keeps pre-colour bytes; `sh_group.rs:440-445` None/Some why; `stats.rs` all-glow fallback why; `lib.rs:24-27` shim reason; `texture_mips.rs:22-25` allow/comment order.
- 🟢 `build_pipeline.md` §Baked texture mips: one sentence that means derive from the full-size level on reuse or rebake, and a rejected sidecar is rebaked before any mean is taken.
- 🟢 `context/lib/index.md` router has no entry for bounce albedo / material stage or indirect-scale consumers — owner decision (library additions).

## Not findings (verified)

Grey path bit-identical; estimator shared by base and delta; key fold encoding unambiguous, sorted, presence-tagged per index, re-keys on texture swaps; version bumps reach filenames; partial warm builds can't mix colours; reflections re-key from SH bytes; id41 coarsener uncached; gate order; fluid tail indices; byte report dedup; renderer layouts byte-for-byte, same-frame snapshot, fog unscaled, reflection mirror tied by test. Main oracle trustworthy (pre-colour source).

## Deferred to follow-up (not this brief)

- `SurfaceMaterials::sample` hard-codes magenta; surface-radiance-cache footprint consumer will need a fallback parameter. Note in the next brief rather than build now.
