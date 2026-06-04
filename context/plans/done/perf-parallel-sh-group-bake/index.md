---
name: Parallelize the per-group SH bake
description: >
  Perf follow-up to incremental-bake-per-element. The warm per-group SH bake
  (`bake_sh_volume_grouped`) runs serially while the monolithic `bake_sh_volume`
  parallelizes across probes with rayon, so a cold-cache (all-miss) warm build is
  ~3x slower than `--no-cache`. Parallelize the per-group bake so an all-miss warm
  build is at least on par with the cold build, without changing output bytes.
type: plan
---

# Parallelize the per-group SH bake

## Goal

Make a fresh / all-miss warm SH build no slower than a `--no-cache` cold build.
`bake_sh_volume_grouped` (`crates/level-compiler/src/sh_group.rs`) bakes groups in
a serial `for group in &groups` loop, and `bake_group` bakes each group's probes
with a serial `.iter().map()`. The monolithic `bake_sh_volume`
(`crates/level-compiler/src/sh_bake.rs`) bakes all probes with
`(0..total).into_par_iter().map(...).collect()`. The warm path therefore loses
the monolithic `into_par_iter` amortization. Measured consequence: on
occlusion-test a cold-cache warm build #1 took **~677 s** vs **~228 s** for
`--no-cache` (~3x); campaign-test warm #1 was **~713 s**. This undercuts the
original plan's headline iteration-speed goal on the first build after a geometry
edit, which re-bakes every group. Tracked today as a `// follow-up:` comment at
the group loop (`sh_group.rs:656`).

The single-light-edit warm rebuild (a few in-reach groups miss, the rest hit) is
already fast — this targets the all-miss case (fresh cache, or a geometry edit,
which invalidates every group via the whole-map geometry content hash).

## Scope

### In scope

- Parallelize the all-miss warm SH bake with rayon, mirroring the monolithic
  bake's order-preserving parallel structure. Group-level parallelism (rayon over
  the group list) is the committed approach (see Approach); per-probe-within-group
  parallelism is a documented fallback only if the wall-clock measurement shows
  group count is too low to saturate cores.
- Preserve byte-identical output: the existing determinism gates
  (`full_light_set_grouped_equals_monolithic`,
  `sh_cold_grouped_equals_monolithic_on_fixtures`,
  `warm_sh_within_tolerance_on_fixtures`) must still pass — byte-identical where
  they were byte-identical, within tolerance where they were within tolerance.
- Keep `StageCache` `get`/`put` correct under concurrency. State the thread-safety
  requirement explicitly and satisfy it (see Cache writes).
- Remove the `// follow-up:` comment at the group loop once the work lands.
- Record before/after wall-clock on a heavily-lit fixture (the cold-cache warm
  build #1 vs `--no-cache`). To produce the all-miss warm build, delete the cache
  dir (`.build-caches/prl-cache/` at the workspace root) and run `prl-build <map>`
  (warm path, every group a miss); separately run `prl-build --no-cache <map>` for
  the cold baseline; time both.

### Out of scope

- The lightmap path (per-light layers, compositor) — untouched.
- The cache key derivation (`group_cache_key`), the reach cutoff
  (`SH_REACH_CUTOFF_METERS = 16.0`), the group size (`SH_GROUP_DIM = 4`), the
  warm/cold contract, and `encode_group_payload`/`decode_group_payload` (the
  per-group cache payload encode/decode; the byte-stable-payload property the
  determinism argument relies on must not be touched) — unchanged. This is a
  parallelization of existing serial code, not an algorithm change.
- The bake math: `bake_probe`, `pack_octahedral_irradiance_tile`, the per-probe
  global-index seeding, the reaching-light selection and ordering — unchanged.
- Runtime, the PRL format, and the `OctahedralShVolume` section layout — untouched.
- The monolithic `bake_sh_volume` — already parallel; no change.
- `build_shell` (sh_group.rs:684, the shared volume-shell allocation that
  `bake_sh_volume_grouped` calls) is unchanged; its byte-identity is already
  pinned by `full_light_set_grouped_equals_monolithic`.
- The separate `.prl` release-provenance-stamp follow-up (a PRL format change, its
  own plan) and any group-size or cutoff retuning.

## Acceptance criteria

> Verified manually — the project has no CI or headless build. Each criterion is
> checked by running `prl-build` (the `postretro-level-compiler` binary) plus the
> `sh_group`/`sh_bake` test suites.

- [x] `full_light_set_grouped_equals_monolithic` still passes: the full-light-set
      grouped bake is byte-identical to the monolithic `bake_sh_volume`.
- [x] `sh_cold_grouped_equals_monolithic_on_fixtures` still passes on every
      `content/dev/maps/` fixture (the cold/full-reach SH ship-path regression
      guard). (Run with `--ignored`.)
- [x] `warm_sh_within_tolerance_on_fixtures` still passes: warm SH stays within
      `WARM_SH_P999_REL_IRRADIANCE_ERROR` (p99.9 of the floored relative error) on
      every fixture. (Run with `--ignored`.)
- [x] The existing `sh_group` suite still passes: `grouped_bake_is_self_consistent`
      (a warm grouped bake is deterministic across two runs),
      `cache_round_trip_hits_on_second_build`, `corrupt_cache_entry_re_bakes`,
      `partition_covers_every_probe_exactly_once`.
- [~] On a heavily-lit fixture (occlusion-test or campaign-test): the cold-cache
      warm build #1 (every group a miss) passes if warm #1 wall-clock ≤ 1.1× the
      `--no-cache` build on the named fixture (ideally faster); the original ~3x
      regression is gone. Wall-clock for both is recorded. **Result (occlusion-test):
      warm #1 all-miss = 165 s, `--no-cache` cold = 120 s, warm #2 all-hit = 10 s.
      The ~3x regression is gone (warm #1 fell from ~677 s to 165 s). The ≤1.1×
      sub-clause is NOT met (1.375×): the ~45 s residual is `StageCache::put`'s
      per-entry `sync_all()` fsync tax across ~900 group writes, which this plan
      scoped out (see Open questions). Accepted by owner; see fsync follow-up below.**
- [x] A second (all-hit) warm build still serves every group from cache and emits
      a `.prl` byte-identical to the first warm build (no regression to the
      existing round-trip-skip behavior). (warm #1 == warm #2, sha256 verified.)
- [x] `cargo fmt --check`, `cargo clippy -p postretro-level-compiler -- -D warnings`, and `cargo test` are clean
      for the `postretro-level-compiler` crate.
- [x] No `unsafe`.

> **Follow-up (deferred, owner-accepted):** the residual gap to the ≤1.1× target is
> `StageCache::put`'s per-entry `sync_all()` (`cache.rs`), now parallel but still ~45 s
> across ~900 concurrent puts on occlusion-test. Closing it (batched/deferred fsync, or
> dropping per-entry `sync_all`) is out of scope here — file as its own perf task if the
> cold-bake cache-write tax becomes a priority.

## Determinism (hard constraint)

The assembled warm volume must remain byte-identical to today's serial grouped
bake. The output is fed to `StageCache`, which serves stored bytes verbatim and
keys on input hash — any nondeterminism silently poisons the cache. This is the
same invariant the monolithic bake already documents
(`sh_bake.rs` `sh_volume_bake_produces_byte_identical_output_on_repeated_runs`).

Why an order-preserving parallel map is safe here:

- **Per-probe seeds are index-derived, not stateful.** `bake_group` calls
  `bake_probe(..., global_index as u64)` and threads each kept light's global
  `static_lights` index via `Some(&global_indices)` into the soft-visibility seed.
  The seed is a pure function of the probe's global index and the light's global
  index — no RNG, no cross-probe or cross-group state. A probe baked in any thread
  produces identical bytes.
- **The reaching-light set and its order are computed per group from the global
  `static_lights` slice** (`reaching_lights`), independent of bake order. The
  per-hit radiance sum iterates this fixed-order slice, so the float accumulation
  order is unchanged.
- **Assembly is pure placement.** `place_group` writes each probe's metadata to
  `probes[global_index]` and byte-copies its tile to
  `irradiance_tile_origin(global_index, ...)`. Placement is keyed on global index,
  so the order groups are produced in does not affect the assembled bytes —
  provided placement does not race (see below).

Therefore an **order-preserving `into_par_iter().map().collect()`** (over groups,
or over probes within a group) is the only acceptable shape — exactly what the
monolithic bake uses. A `for`-accumulating reduction or a `par_iter().reduce()`
over floats is **NOT** acceptable: it would reorder the float sums and break byte
identity. Mirror the monolithic structure: parallel **map** to a `Vec`, collect in
index order, then place. The result must be invariant to rayon's thread schedule —
`grouped_bake_is_self_consistent` already asserts run-to-run determinism; keep it
passing.

## Cache writes (thread-safety requirement)

`StageCache` holds only a `dir: PathBuf`; `get` and `put` take `&self`, perform no
interior mutation, and `put` writes atomically (temp file + `fs::rename`) to a
filename derived from the entry's unique `CacheKey`. Distinct groups have distinct
keys — `group_cache_key` (sh_group.rs:403) folds in the group's identity, so
distinct groups yield distinct `CacheKey`s, which is the property the
no-collision argument depends on — so concurrent `put`s target distinct paths. `par_iter` over `&groups` visits each group exactly once and keys are 1:1 with
groups (the partition is disjoint — `partition_covers_every_probe_exactly_once`),
so no cache key is read or written by two workers concurrently. `&StageCache` is
`Sync`, so sharing it across rayon workers is sound, and concurrent `put`/`get` on
distinct keys do not race.

**Requirement:** the parallelization must share the existing `&StageCache` across
workers without introducing a lock or serializing cache I/O on the critical path.
No `StageCache` API change. The implementer chooses between:

1. **Concurrent cache I/O inside the parallel map** — each worker calls
   `bake_or_load_group` (its own `get`/`put`) directly. Simplest; relies on the
   distinct-key / atomic-rename property above. Recommended.
2. **Parallel bake, serial writes** — workers return baked groups; the driver
   places and `put`s them serially after the map. Use only if profiling shows
   concurrent `put`s contend (they should not, given distinct paths).

State which option lands in the rough sketch's final form and why. Do not add a
mutex around the whole cache — that reintroduces the serialization this plan
removes.

## Rough sketch

`bake_sh_volume_grouped` (`sh_group.rs:634`):

- Replace the serial `for group in &groups { let baked = bake_or_load_group(...);
  place_group(&mut section, &baked); }` loop with an order-preserving parallel map
  over `&groups` that produces `Vec<BakedGroup>`, then a serial placement pass
  (`place_group` mutates `&mut section`, so placement stays single-threaded; it is
  cheap byte-copy, not the cost center). Concretely: `groups.par_iter().map(|group|
  bake_or_load_group(inputs, &layout, group, &static_lights, config.probe_spacing,
  &geom_hash, cache)).collect::<Vec<_>>()`, then `for baked in &baked_groups {
  place_group(&mut section, baked); }`. `bake_or_load_group` already does its own
  `get`/bake/`put`, so cache I/O parallelizes for free (Cache writes option 1).
- `ShBakeCtx<'_>`, `ProbeGridLayout`, `&[&MapLight]`, `&[u8; 32]`, and
  `Option<&StageCache>` are all shared `&`-borrows with no interior mutability, so
  they cross the rayon closure boundary as `Sync` references. Confirm the BVH /
  primitives borrows are `Sync` (the monolithic `into_par_iter` over probes already
  shares the same `&ShBakeCtx`, so this is established).
- Optionally also parallelize `bake_group`'s inner `.iter().map()` over
  `probe_indices` with `into_par_iter` (mirroring the monolithic per-probe
  parallelism). For the all-miss case, group-level parallelism alone already
  recovers the amortization, and nested rayon parallelism adds scheduling overhead
  for little gain when group count is in the hundreds-to-thousands. Recommend
  group-level only unless the wall-clock measurement shows group count is too low
  to saturate cores on the target fixture — in which case parallelize probes
  within groups instead of (not in addition to) groups, to avoid nested pools.
- Remove the `// follow-up:` comment block (`sh_group.rs:656-662`) once landed;
  replace with a one-line note that the group bake is parallel and why ordering is
  preserved (point at the determinism rationale, do not restate it).

**Approach options weighed:**

| Axis | Pros | Cons | Verdict |
|---|---|---|---|
| **rayon over groups** (`groups.par_iter()`) | each group independent; `bake_or_load_group` already self-contained incl. cache I/O; minimal diff | group count must be high enough to saturate cores (hundreds–thousands per Task 1 spike — fine) | **recommended** |
| rayon over probes within each group | mirrors monolithic exactly; saturates cores even with few groups | nested under a group loop = nested pools, or requires flattening; more churn | fallback if group count too low |
| flat parallel over all probes, route to per-group cache entries | maximal parallelism | re-introduces the routing the per-group cache was built to avoid; complicates cache-entry assembly | rejected — fights the per-group cache design |

The recommended shape keeps the diff small and the cache wiring untouched, and
matches the monolithic bake's proven order-preserving-map pattern.

## Sequencing

Single contained change; no task split needed.

**Phase 1 (sequential):** parallelize `bake_sh_volume_grouped`, confirm cache
thread-safety holds, keep all gates green, record before/after wall-clock.

## Open questions

- **Group count vs. core count — resolved.** Group-level parallelism is the
  committed approach: measured group counts (~3,032 campaign 4³ / ~900 occlusion)
  far exceed any core count, so group-level saturation is guaranteed on these
  fixtures. The per-probe fallback applies only if a smaller fixture proves too few
  groups to saturate cores.
- **Per-entry `sync_all` on the cold-bake write path.** `StageCache::put` calls
  `file.sync_all()` per entry (`cache.rs:150`). At a few thousand concurrent puts
  this is a one-time cold-bake tax, now spread across threads rather than serial.
  Out of scope to change here; note it if the wall-clock shows fsync, not ray
  tracing, dominates the warm #1 build (unlikely — the bake is ray-bound).
