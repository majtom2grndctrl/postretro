# Compiler Log Hygiene

> **Dependencies (this backlog):** Independent of the SH and lightmap specs, with one coordination point — it shares the `pack.rs` section-size logging surface with `lighting-scale--sh-delta-footprint-instrumentation`. This spec downgrades the per-section byte breakdown to `debug!`; that spec adds new "SH-total vs non-SH" and indirect `DeltaShVolumes` size summary lines. Coordinate so this downgrade does not suppress those new summary lines (they stay at `info!`).

## Goal

Cut `prl-build` (level-compiler) log noise so a default build is quiet and `-v`/`--verbose` shows curated per-stage stats, not raw per-item spam. Today two classes of clutter leak through: benign per-light `warn!` defaults print on *every* build at the default `warn` verbosity, and unconditional `info!` per-item dumps flood `-v` / `RUST_LOG=info` because they lack the `if args.verbose` gate the `log_stats` dumps have. Logging-only — no change to compiled `.prl` bytes or build behavior.

## Scope

### In scope
- Remove benign per-light default-value warnings (missing `style`, `_color`, `_cone`, `_cone2`, `angles` → documented FGD defaults).
- Downgrade benign ineffective-input hints (intensity-0, `_phase`/`_start_inactive` with style 0) from `warn!` to `info!` — kept for `-v` debugging, off by default.
- Downgrade unconditional `info!` per-item spam (per-section byte breakdown, `[cache]` hit/miss, per-fog-entity dumps, per-PNG, per-animated-light delta, once-per-build stage summaries) to `debug!`, so `-v`/info stays curated and the detail is reachable at `RUST_LOG=debug`.
- Downgrade the always-visible warm-SH approximation notice to `info!`.
- Gate the per-light sub-texel penumbra hint behind `-v` (downgrade to `debug!`).

### Out of scope / explicit KEEP (do NOT touch)
- All genuine authoring-error warns: value clamps (`_phase`/`_curve_phase`/`light_range`/scatter), conflicts (`_cone` > `_cone2`, brightness-curve-vs-`style`, `_bake_only` + animation), leaks (exterior probe in solid leaf, no interior empty leaves), cap overflows (chunk light list, animated-light-chunk, shadowmask overlap/out-of-range drop), cache-corruption recovery warns, texture problems (mip validation), and `Portal generation produced 0 portals`.
- Every `log_stats` body — already double-gated `if args.verbose { … info! }`. Leave as-is.
- The `println!` Build Summary block and the single per-file `Wrote …` / read-back-validation summary lines — these stay visible at default verbosity.
- No `.prl` output change. No new logging added. No re-plumbing of a `verbose` flag through the bakers — prefer a straight `debug!` downgrade.
- No file split. This change only removes or re-levels existing log calls (net-neutral to slightly smaller); it adds no functionality to any file, so the split-before-extend rule does not apply even where a touched file exceeds 800 lines.

## Acceptance criteria

- [ ] A default build (no `-v`, no `RUST_LOG`) of a many-light stress map emits **no** per-light default-value warnings and no per-light ineffective-input hints.
- [ ] A default build emits no per-light sub-texel penumbra hints and no warm-SH approximation notice.
- [ ] Genuine authoring-error warnings — conflicting, clamped, or dropped inputs — still print at default verbosity.
- [ ] Genuine leak, cap-overflow, cache-corruption-recovery, texture-problem, and zero-portal warnings are unchanged (still print at default verbosity).
- [ ] Counted before and after on the same many-light stress map, the default-verbosity warn-line total drops measurably; after the change the remaining warn lines are exclusively genuine-error warns (the per-light default/hint/penumbra lines are gone).
- [ ] A `-v` (or `RUST_LOG=info`) build shows the curated per-stage stats but **no** raw per-section byte breakdown, no per-`[cache]`-entry hit/miss lines, no per-fog-entity dumps, no per-PNG lines, and no per-animated-light-item dumps. Those appear only at `RUST_LOG=debug`.
- [ ] The single per-file `Wrote …` summary, the read-back-validation confirmation, and the per-stage Build Summary still print at default verbosity.
- [ ] A `.prl` compiled from the same inputs and cache state before and after the change is byte-identical.

## Tasks

### Task 1: Remove benign per-light warns, downgrade ineffective-input hints
In `crates/level-compiler/src/format/quake_map.rs` (all inside `translate_light`, fired once per light entity): **delete** the five per-light default-value `warn!` calls that merely restate FGD-documented defaults — missing `style` → 0 at line 241 (the motivating example), missing `_color` → white at 116, `light_spot` missing `_cone` → 30° at 188, missing `_cone2` → 45° at 195, and `light_sun` missing `angles` → straight-down at 231. Then **downgrade from `warn!` to `info!`** (do not delete) the three ineffective-input hints so they survive for `-v` debugging but are silent by default: intensity-0 at line 104, `_phase` set with `style=0` at 477, `_start_inactive` set with `style=0` at 481 (macro opens at 480). Do not touch the genuine-conflict warns in the same function — `_cone` > `_cone2` (200/201), brightness-curve-vs-`style` (414), `_bake_only` + animation (499) — nor any clamp warn.

### Task 2: Downgrade unconditional per-item `info!` spam to `debug!`
Change these unconditional `info!` calls to `debug!` (no `verbose` token exists in these files, so a straight macro swap is correct; no flag threading). In `crates/level-compiler/src/pack.rs`, the per-section byte-breakdown block spanning lines 789–928 (~23 `info!` lines) → `debug!`; **keep** the single `Wrote …` summary at 947 and the read-back-validation confirmation at 951 as `info!`. In `crates/level-compiler/src/main.rs`, the `[cache]` hit/miss pairs: lightmap 683/687/701/705, navmesh 542/548/565, animated-lm-weight-maps 1135/1138, sdf-atlas 1187/1190 → `debug!` (leave the adjacent corrupt-cache `warn!` at 552/1129/1182 untouched). In the bake modules: `sh_group.rs` 508/519, `direct_sh_bake.rs` 650/658, `shadowmask_bake.rs` 160/164/188/192 (its cache hit/miss — leave the drop/overflow warns at 44/107/503) → `debug!`. Per-fog-entity dumps in `parse.rs` at 1006/1131/1231/1376 and the 8-line stat summary at 776–783 → `debug!`. `texture_validation.rs` per-PNG log at 171 → `debug!`. Per-animated-light delta line in `delta_sh_bake.rs` at 337 → `debug!`. The unconditional bake-path SDF summary in `sdf_bake.rs` at 323 → `debug!` (its verbose-gated `log_stats` duplicate at 356 stays `info!`). The once-per-build stage summaries → `debug!`: `navmesh_bake.rs` 123/435, `chunk_light_list_bake.rs` 333, `animated_light_chunks.rs` 78/195, `animated_light_weight_maps.rs` 148, `visibility/mod.rs` 111. In `animated_light_chunks.rs` leave the cap-overflow `warn!` at 205/278; in `chunk_light_list_bake.rs` leave the overflow `warn!` at 276/347/424; in `visibility/mod.rs` leave the leak `warn!` at 70/124.

### Task 3: Downgrade the two default-visible lighting warns
Two `warn!` calls print at default verbosity on ordinary builds. In `crates/level-compiler/src/main.rs` at line 806, the warm/cached-SH approximation notice (`WARM_SH_APPROX_WARNING`) fires on every warm build → change to `info!`. In `crates/level-compiler/src/lightmap_bake.rs` at line 1462, the per-light sub-texel penumbra hint inside `warn_sub_texel_penumbra_lights` (a `for` loop over static lights, so it fans out per light) → downgrade to `debug!` so it is reachable at `RUST_LOG=debug` but silent at `-v` and default. Leave every other warn in both files untouched.

## Sequencing

**Phase 1 (concurrent):** Task 1 (`quake_map.rs` only), Task 2 (the info→debug sweep across pack/parse/main/bake modules) — disjoint file sets.
**Phase 2 (sequential):** Task 3 — shares `main.rs` with Task 2 (Task 2 edits the cache lines, Task 3 edits the warm-SH line at 806); run after Phase 1 so the two `main.rs` edits don't collide.

## Rough sketch

The logger has no per-call gate. `main.rs:366–367` sets the env_logger default filter to `warn`, raised to `info` by `-v` (`Args.verbose` field at 1276, parsed at 1390–1392). So level *is* the gate: `warn!` always prints, `info!` needs `-v`, `debug!` needs `RUST_LOG=debug`. The `log_stats` dumps are correctly double-gated (`if args.verbose { … info! }`); the offenders are unconditional `info!`/`warn!` that only the level filter suppresses. The fix is entirely re-leveling and deleting existing macro calls — no control-flow, no `.prl`, no new logs.

Note the two verified line corrections: the `pack.rs` breakdown block ends at 928, not 951 (lines 947/951 belong to `write_and_validate_sections`, the file-write summary that stays `info!`); and the `main.rs` portal warn macro opens at 466 with its message on 467 — that one is a KEEP.

Byte-identical `.prl` (final AC) is the safety net proving this was logging-only: verify by compiling a fixture map to two outputs across the change and diffing, holding inputs and cache state constant.
