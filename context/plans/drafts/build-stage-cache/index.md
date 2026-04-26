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

- Disk-backed cache, keyed by `blake3(stage_version || input_hash)`, storing serialized stage outputs.
- Cache participation for the two expensive stages: **lightmap bake** and **SH volume bake**.
- Cache substrate (key derivation, atomic write, load, eviction-by-nuke) usable by additional stages later without rework.
- Determinism audit and fixes for any cached stage's output. Two builds with identical input must produce byte-identical cache entries.
- CLI surface: `--cache-dir <path>` (default: `<workspace-root>/.prl-cache/`), `--no-cache` (bypass read and write). Workspace root is located by walking parent directories for `Cargo.toml`; falls back to map-adjacent if no workspace root is found.
- Cache invalidation tests: edit-property-only round trips skip the bakes; edit-light round trips re-bake; bumping a stage version constant invalidates that stage's entries.
- Documentation update in `context/lib/build_pipeline.md` describing the cache, its invariants, and the stage-version bump discipline.

### Out of scope

- **Per-element / sub-region incremental bake.** Caching a partial lightmap atlas or a subset of SH probes. Tracked in the sibling stub plan `incremental-bake-per-element/`.
- **Caching the cheap stages.** Parse, BSP, portals, exterior cull, geometry, BVH. They are not iteration-loop bottlenecks; adding cache participation costs determinism work without measurable speedup. The substrate makes adding them later trivial if profiling shows it's worth it.
- **Cache eviction policy.** No LRU, no size cap. The user nukes `.prl-cache/` if it grows. Reconsider once anyone complains.
- **Distributed / shared cache** (e.g., across CI machines or developers). Local on-disk only.
- **Runtime PRL changes.** PRL format and engine loader untouched. Cache lives next to the build, not inside the artifact.
- **Parallel builds writing to the same cache dir.** Single-builder assumption. Concurrent writers may race; behavior is "last writer wins, no corruption" via atomic rename, but cache hits are not guaranteed across racing builds.

## Acceptance criteria

- [ ] Building a `.map` twice with no changes between builds: second build skips the lightmap and SH volume stages (verified via build progress log) and produces a `.prl` byte-identical to the first.
- [ ] Editing an entity property that no cached stage's input includes (e.g., `worldspawn.fog_pixel_scale`): second build skips both lightmap and SH volume; output PRL differs only in the entity-derived sections.
- [ ] Moving one light: second build re-bakes both lightmap and SH volume (cache miss); output differs in lighting sections.
- [ ] `--no-cache` flag: cache is neither read nor written; build behaves as if cache directory is empty; no `.prl-cache/` is created or modified.
- [ ] `--cache-dir <path>` flag: cache reads and writes are confined to the supplied path.
- [ ] Bumping a stage's version constant in source invalidates only that stage's entries on the next build (verified by inspecting which entries the next build writes).
- [ ] Two clean builds of every fixture in `assets/maps/` produce byte-identical cache entries for every cached stage. (Determinism gate.)
- [ ] Cache directory is safe to delete at any time; the next build succeeds and rebuilds all entries.
- [ ] A corrupted or truncated cache entry is detected (length or hash mismatch), discarded with a warning, and the stage re-runs.
- [ ] `context/lib/build_pipeline.md` documents the cache: where it lives, how keys are derived, when to bump a stage version, the determinism invariant.

## Tasks

### Task 1: Cache substrate

Build a `cache` module in `postretro-level-compiler` exposing a `StageCache` with `get(key) -> Option<Bytes>` and `put(key, bytes)`. Storage: one file per entry under the cache directory, name is the hex key, contents are `[length: u32 | blake3-of-payload: 32 bytes | payload]`. Atomic write via temp file + rename. Length+hash check on load; mismatch is a soft failure (warn, return `None`). Provide a `CacheKey` builder that hashes `(stage_id_str, stage_version_u32, input_hash)` with blake3.

### Task 2: Stage I/O serialization

Add `serde` derives (or implement `to_bytes`/`from_bytes` consistent with the format crate's pattern) to the intermediate types consumed and produced by the cached stages: at minimum `GeometryResult` and the relevant slices of `MapData` that feed lightmap and SH bake. Avoid serializing the live `bvh::Bvh` — derive its inputs from cached `GeometryResult` and rebuild on cache hit. Pick one serialization (postcard or bincode) and use it consistently.

### Task 3: Determinism audit + fixes

Audit lightmap bake and SH volume bake for non-determinism that would defeat byte-identical caching. Known suspects from initial survey: any `HashMap` whose iteration order leaks into output, rayon reductions that aren't order-preserving, floating-point accumulation order across threads. Fix by replacing `HashMap` with `BTreeMap` (or sorting before iteration), and by ensuring `par_iter().collect()` is the only parallel pattern (it preserves index order). Add a determinism test that runs each cached stage twice and asserts byte-identical output.

### Task 4: Wire cache into lightmap stage

Compute the lightmap-bake input hash from: serialized geometry input, serialized static-light entity list, lightmap-density CLI flag, and any other flag the bake reads. Look up; on hit, deserialize and skip the bake; on miss, run the bake and write. Preserve the existing retry-on-overflow behavior — only the *successful* output is cached, not failed attempts. Surface cache hit/miss in build progress logs.

### Task 5: Wire cache into SH volume stage

Same shape as Task 4. Input hash includes serialized geometry, BVH section bytes (rebuilt from cached geometry on the fast path), static-light entity list, and the probe-spacing flag. Cache the `ShVolumeSection`. Surface hit/miss in logs.

### Task 6: CLI flags

Extend `parse_args()` with `--cache-dir <path>` (default `<input.map dir>/.prl-cache/`) and `--no-cache`. Thread into the pipeline. Update `--help` text. Reject the combination `--no-cache --cache-dir` with a clear error (or silently ignore `--cache-dir` and warn — pick one in the spec moment).

### Task 7: Tests

- Determinism: each cached stage produces byte-identical output across two clean runs (one test per stage).
- Round-trip skip: build twice, assert second build's progress log records cache hits for the two cached stages.
- Targeted invalidation: build, edit a single light entity in the `.map`, build again; assert both bake stages report cache miss while other stages are unaffected. Repeat with an entity property that no cached stage reads; assert both bakes hit.
- Stage version bump: change a stage's version constant in source, build; assert that stage misses and others hit.
- Corruption recovery: write garbage into a cache entry, build; assert warning is logged and the stage re-runs.
- `--no-cache`: assert no files are read from or written to the cache directory.

### Task 8: Documentation

Update `context/lib/build_pipeline.md` with a new Build Cache section: where the cache lives, what stages participate, key composition, the stage-version bump rule (when an algorithm changes, bump its version; the substrate handles invalidation). Note the determinism invariant as a maintenance constraint on the cached stages. Add `--cache-dir` and `--no-cache` to the CLI reference.

## Sequencing

**Phase 1 (sequential):** Task 1 — cache substrate is the foundation; everything else imports it.
**Phase 2 (sequential):** Task 2 — stage I/O serialization is required before any stage can be wired in.
**Phase 3 (sequential):** Task 3 — determinism fixes must land before stage wiring, otherwise cache entries are unstable from day one and tests can't pin down regressions.
**Phase 4 (concurrent):** Task 4, Task 5 — lightmap and SH wiring are independent.
**Phase 5 (sequential):** Task 6 — CLI flags depend on the wired stages reading them.
**Phase 6 (concurrent):** Task 7, Task 8 — tests and docs are independent and finish the plan.

## Open questions

- **Should the cache be content-addressable across stages?** Storing entries as `<key>.bin` means two stages with the same input+output (unlikely but possible) share an entry. Cleaner but trivially solvable either way; no impact on correctness.
- **Determinism scope of Task 3:** are there latent non-determinism issues outside the cached stages that should be fixed opportunistically (e.g., for the future per-element plan), or held strictly to the cached stages to keep this plan tight? Decided: hold to cached stages; capture the rest as research notes for the sibling plan.

## Decisions

- **Cache directory default:** workspace-adjacent (`.prl-cache/` next to the workspace `Cargo.toml`). One directory to nuke when clearing; shared across all maps in the workspace. Workspace root located by walking parent dirs from the input `.map`; falls back to map-adjacent if no workspace root found.
- **Serialization format:** postcard. More compact than bincode (varint encoding vs. fixed-width), no false-positive invalidation risk from upstream API churn (bincode v1→v2 break history), and designed for deterministic Rust-to-Rust byte serialization — which is exactly this use case.
- **Stage version representation:** manual `const STAGE_VERSION: u32` per stage module, bumped by the engineer changing the algorithm. Source-file-hash auto-bump was rejected: too many false positives (editing a doc comment re-runs a 30-second SH bake). Optionally enforce via CI lint: warn if a stage source file changed but its version constant did not.
- **Determinism scope:** audit and fix only the two cached stages (lightmap, SH volume). Wider cleanup deferred to the per-element sibling plan.
