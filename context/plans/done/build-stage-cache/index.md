---
name: Build Stage Cache
description: Disk-backed content-hash cache for prl-build stages. Skips lightmap and SH volume bake when their inputs are unchanged.
type: plan
---

# Build Stage Cache

## Goal

Tighten the `prl-build` iteration loop by caching the output of expensive stages on disk, keyed by content hash of their inputs. When a level author edits something that doesn't affect a stage's inputs (an entity property, an unrelated brush far from any light, a CLI flag the stage doesn't read), that stage is skipped and its prior output is loaded from cache. Lightmap and SH volume bake are the primary targets — they dominate compile time today.

This plan also lays the substrate (deterministic stage outputs, serializable intermediates, an addressable cache directory) that a future per-element incremental bake plan will build on.

## Scope

### In scope

- Disk-backed cache, keyed by `blake3(stage_id || stage_version || input_hash)`, storing serialized stage outputs.
- Cache participation for the two expensive stages: **lightmap bake** and **SH volume bake**.
- Cache substrate (key derivation, atomic write, load, eviction-by-nuke) usable by additional stages later without rework.
- Determinism audit and fixes for any cached stage's output. Two builds with identical input must produce byte-identical cache entries.
- CLI surface: `--cache-dir <path>` (default: `<workspace-root>/.prl-cache/`), `--no-cache` (bypass read and write). Workspace root is located by walking parent directories for `Cargo.toml`.
- `.prl-cache/` and compiled `.prl` artifacts added to the workspace `.gitignore`.
- Cache invalidation tests: edit-property-only round trips skip the bakes; edit-light round trips re-bake; bumping a stage version constant invalidates that stage's entries.
- Documentation update in `context/lib/build_pipeline.md` describing the cache, its invariants, and the stage-version bump discipline.

### Out of scope

- **Per-element / sub-region incremental bake.** Caching a partial lightmap atlas or a subset of SH probes. Tracked in the sibling stub plan `incremental-bake-per-element/`.
- **Caching the cheap stages.** Parse, BSP, portals, exterior cull, geometry, BVH. They are not iteration-loop bottlenecks; adding cache participation costs determinism work without measurable speedup. The substrate makes adding them later trivial if profiling shows it's worth it.
- **Cache eviction policy.** No LRU, no size cap. No policy-driven eviction. The user nukes `.prl-cache/` if it grows. Corruption triggers per-entry discard, not a full nuke.
- **Distributed / shared cache** (e.g., across CI machines or developers). Local on-disk only.
- **Runtime PRL changes.** PRL format and engine loader untouched. Cache lives next to the build, not inside the artifact.
- **Parallel builds writing to the same cache dir.** Single-builder assumption. Concurrent writers may race; behavior is "last writer wins, no corruption" via atomic rename, but cache hits are not guaranteed across racing builds.

## Acceptance criteria

- [x] Building a `.map` twice with no changes between builds: second build skips the lightmap and SH volume stages (verified via build progress log) and produces a `.prl` byte-identical to the first.
- [x] Editing an entity property that no cached stage's input includes (e.g., `worldspawn.fog_pixel_scale`): second build skips both lightmap and SH volume; output PRL differs only in the entity-derived sections.
- [x] Moving one light: second build re-bakes both lightmap and SH volume (cache miss); output differs in lighting sections.
- [x] `--no-cache` flag: cache is neither read nor written; build behaves as if cache directory is empty; no `.prl-cache/` is created or modified.
- [x] `--cache-dir <path>` flag: cache reads and writes are confined to the supplied path.
- [x] Bumping a stage's version constant in source invalidates only that stage's entries on the next build (verified by inspecting which entries the next build writes).
- [x] Two clean builds of every fixture in `content/dev/maps/` produce byte-identical cache entries for every cached stage. (Determinism gate.)
- [x] Cache directory is safe to delete at any time; the next build succeeds and rebuilds all entries.
- [x] A corrupted or truncated cache entry is detected (length or hash mismatch), discarded with a warning, and the stage re-runs.
- [x] `context/lib/build_pipeline.md` documents the cache: where it lives, how keys are derived, when to bump a stage version, the determinism invariant.
- [x] `.prl-cache/` and compiled `.prl` artifacts are listed in the workspace `.gitignore`.

## Tasks

### Task 1: Cache substrate

Build a `cache` module in `postretro-level-compiler` exposing a `StageCache` with `get(key) -> Option<Bytes>` and `put(key, bytes)`. Storage: one file per entry under the cache directory, name is the hex key, contents are `[length: u32 le | blake3-of-payload: 32 bytes | payload]`. Atomic write via temp file + rename. Length+hash check on load; mismatch is a soft failure (warn, return `None`). `StageCache::new(path)` creates the directory if it does not exist. Provide a `CacheKey` builder that hashes `(stage_id_str, stage_version_u32, input_hash)` with blake3. Each participating stage module owns a `const STAGE_VERSION: u32`, manually bumped by the engineer when the algorithm changes. Stage ID in the key means entries are per-stage by construction — no cross-stage collision risk.

### Task 2: Stage I/O serialization and config structs

Add `serde` derives to the intermediate types consumed and produced by the cached stages: at minimum `GeometryResult`, `LightmapInputs`, and `ShInputs`. Avoid serializing the live `bvh::Bvh` — derive its inputs from cached `GeometryResult` and rebuild on cache hit. Use postcard throughout: more compact than bincode (varint vs. fixed-width encoding) and designed for deterministic Rust-to-Rust byte serialization.

Define `LightmapInputs` and `ShInputs` structs containing the exact fields each bake reads from `MapData`. Refactor the bake entry points to take these structs instead of `&MapData`. This serves two goals: the hash covers exactly the inputs (no more, no less), and the future per-element incremental bake plan gets precisely-typed input slices without a second refactor. Update all call sites; no compat shims.

Define `LightmapConfig` and `ShConfig` structs (postcard-serialized) populated from `parse_args()` output. Thread them into the lightmap and SH bake entry points respectively. Adding a CLI flag means adding a field to the appropriate struct — the hash picks it up automatically, eliminating silent stale-cache risk from forgotten flag coverage.

### Task 3: Determinism audit + fixes

Audit lightmap bake and SH volume bake for non-determinism that would defeat byte-identical caching. Known suspects from initial survey: any `HashMap` whose iteration order leaks into output, rayon reductions that aren't order-preserving, floating-point accumulation order across threads. Fix by replacing `HashMap` with `BTreeMap` (or sorting before iteration), and by ensuring `par_iter().collect()` is the only parallel pattern (it preserves index order). Scope is the two cached stages only; wider non-determinism in other stages is deferred to the per-element sibling plan. Add a determinism test that runs each cached stage twice and asserts byte-identical output.

### Task 4: Wire cache into lightmap stage

Compute the lightmap-bake input hash from: postcard-serialized `LightmapInputs` and `LightmapConfig`. Look up; on hit, deserialize and skip the bake; on miss, run the bake and write. Preserve the existing retry-on-overflow behavior — only the *successful* output is cached, not failed attempts. Surface cache hit/miss in build progress logs. Log line format: `cache: lightmap hit` or `cache: lightmap miss`.

### Task 5: Wire cache into SH volume stage

Same shape as Task 4. Input hash covers postcard-serialized `ShInputs` and `ShConfig`. BVH is a pure function of geometry; geometry in the hash pins it transitively — no BVH bytes needed. Cache the `ShVolumeSection`. Surface hit/miss in logs. Log line format: `cache: sh_volume hit` or `cache: sh_volume miss`.

### Task 6: CLI flags

Extend `parse_args()` with `--cache-dir <path>` (default `<workspace-root>/.prl-cache/`, workspace root located by walking parent dirs from the input `.map` for `Cargo.toml`) and `--no-cache`. Thread into the pipeline. Update `--help` text. `--no-cache` overrides `--cache-dir` when both are supplied.

### Task 7: Tests

- Determinism: each cached stage produces byte-identical output across two clean runs (one test per stage).
- Round-trip skip: build twice, assert second build's progress log records cache hits for the two cached stages.
- Targeted invalidation: build, edit a single light entity in the `.map`, build again; assert both bake stages report cache miss while other stages are unaffected. Repeat with an entity property that no cached stage reads; assert both bakes hit.
- Stage version bump: change a stage's version constant in source, build; assert that stage misses and others hit.
- Corruption recovery: write garbage into a cache entry, build; assert warning is logged and the stage re-runs.
- `--no-cache`: assert no files are read from or written to the cache directory.

### Task 8: Documentation

Update `context/lib/build_pipeline.md` with a new Build Cache section: where the cache lives, what stages participate, key composition, the stage-version bump rule (when an algorithm changes, bump its version; the substrate handles invalidation). Note the determinism invariant as a maintenance constraint on the cached stages. Add `--cache-dir` and `--no-cache` to the CLI reference. Add `.prl-cache/` and compiled `.prl` artifacts to the workspace `.gitignore`.

## Sequencing

**Phase 1 (sequential):** Task 1 — cache substrate is the foundation; everything else imports it.
**Phase 2 (sequential):** Task 2 — stage I/O serialization, `LightmapInputs`/`ShInputs` extraction, and `LightmapConfig`/`ShConfig` structs. All three are required before any stage can be wired in.
**Phase 3 (sequential):** Task 3 — determinism fixes must land before stage wiring, otherwise cache entries are unstable from day one and tests can't pin down regressions.
**Phase 4 (concurrent):** Task 4, Task 5 — lightmap and SH wiring are independent.
**Phase 5 (sequential):** Task 6 — CLI flags depend on the wired stages reading them. Note: the `--cache-dir` default resolution must be in place before Tasks 4/5; implement the default path logic in Task 1 alongside `StageCache` construction.
**Phase 6 (concurrent):** Task 7, Task 8 — tests and docs are independent and finish the plan.
