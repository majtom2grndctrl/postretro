# Level Load Performance — Investigation Contract

## Goal

Find where level-load time goes and rank what is worth optimizing. This phase measures and reads code; it changes no shipped behavior. Its findings decide the build tracks that follow, and those tracks amend this file.

## Decisions

- **The metric is wall time from load request to first level frame**, split two ways: the level worker (off the main thread) and main-thread install (which freezes the loading screen while it runs). A main-thread stage that costs the same as a worker stage ranks higher, because it is also a visible hitch.
- **Two load paths are measured:** a first load from boot (CLI map) and a level change from Running, which adds unload. A path that cannot run in this container is reported, not skipped silently.
- **Measurement uses the existing per-stage log (log line C, `StartupTimings` in `level_timings`).** Finer marks may be added inside a stage for the investigation. They stay uncommitted unless the finding recommends keeping them as permanent instrumentation.
- **GPU-side cost is out of reach here.** The container has no Vulkan driver. Texture upload, geometry upload, and first-frame present are measured on the CPU side only, and flagged for measurement on the owner's Mac.
- **Maps:** the `stress-warren*` family in `content/dev/maps` (largest sources) and `campaign-test`. Baking is expensive; bake as few as give a representative spread.

## Invariants

- No change to the PRL format, cache keys, the install order in `boot_sequence.md` §3, or the worker/main-thread split in §2 during this phase. A finding may *recommend* changing any of them, stated as a decision for the owner.
- Timings come from the `--release` profile, the one `development_guide.md` names for perf validation. Dev-profile numbers may guide exploration but are never reported as results.
- Each reported number names its map, profile, path (first load or level change), and run count.

## Deliverable

`context/plans/in-progress/level-load-perf-findings.md`: a stage-by-stage cost table per map, then candidate optimizations ranked by expected savings. Each candidate gives its evidence, the crates it touches, the risk, whether it crosses a contract (format, cache key, install order, thread split), and how its gain would be proven.

## Open questions

- Is the target faster time to first frame, or a loading screen that never freezes? The findings should show both; the owner chooses after reading them.

---

# Build phase

The investigation landed in `level-load-perf-findings.md`. The owner chose to build the CPU-only candidates here and carry the GPU-dependent ones to a Mac research brief. Every track reads the findings' section for its candidate.

## Decisions

- **Built here:** candidate 1 (SH-streaming dense-node maps become arrays), candidate 2 (sub-quadratic cluster-directory validation), the single-parse half of candidate 4, and the instrumentation the findings recommend keeping. The worker/main-thread split, the install order, the clear-on-unload table, the PRL format and cache keys stay as they are; candidates that cross one go to the Mac brief.
- **Candidate 1 stays on the main thread and in two crates.** The renderer's stored-node layout and the planner topology in `postretro` each become arrays in place. Sharing one product between them would need a new seam across the renderer boundary; that is not this phase.
- **Candidate 2 stays exact.** The check remains; only its cost changes. Accept/reject behavior must not change for any input, and a rejected file must fail with the same error it fails with today.
- **Permanent instrumentation is log-only.** New marks extend `level_timings` (log line C) or log their own line; no new crate edges, and the renderer never depends on app-side timing types.

## Invariants

- **Equivalence, not just green tests.** Each change keeps the old code path available to a test as an oracle (or an equivalent golden), and a test asserts the new path's output equals the oracle's on the fixtures plus randomized or synthetic inputs. A wrong-but-fast result is the failure this phase fears most.
- **Release measurements, before and after, same machine and setup.** A track records its own baseline on the current branch head before its change. Each number names map, path (first load / level change), run count, and reports the median.
- **Measurement scaffolding is never committed.** `scratchpad/measure-scaffold.patch` holds the lavapipe storage-binding clamp and the `POSTRETRO_X_CYCLE` reload harness; `scratchpad/run.sh` drives it. Apply to measure, reverse before committing. `git diff origin/main...HEAD -- '*.rs' '*.toml'` must never contain `POSTRETRO_X_CYCLE`, `X_PENDING_RELOAD`, or the clamp.
- **Build environment:** release builds source `scratchpad/env.sh` (release keeps line tables and symbols). Building release without it invalidates the warm target and costs a ~25 min rebuild. Focused tests run on the dev profile with `CARGO_INCREMENTAL=0`, one `-p` crate at a time, to bound disk.
- **Disk floor 15 GB free on `/`.** Below it, clear incremental caches under `target/`; never delete the baked `.prl` files or `baked/materials/`.

(`scratchpad/` is `/tmp/claude-0/-home-user-postretro/a33b82f9-2733-5cc5-a1a5-45fd6a0bdb31/scratchpad`.)

## Tracks (sequential, on this branch)

| Track | Owns | Acceptance |
|---|---|---|
| A — dense-node arrays (candidate 1) | `crates/renderer/src/render/sh_streaming/`, `crates/postretro/src/sh_streaming/`; the `geometry_upload` split marks and the unload mark | Equivalence tests green with counts reported; stress-warren-lit `geometry_upload` + `streaming_preload` fall ≥ 0.8 s combined (release, n ≥ 4 first loads, n ≥ 8 changes) |
| B — cluster-directory validation (candidate 2) | `crates/level-format/src/cluster_directory*` | Old-vs-new equivalence test over fixtures and randomized directories; every existing malformed-section test green; stress-warren-lit `prl_parse` keeps ≥ 80 % of the saving a skip-the-comparison prototype shows on this machine, both measured in one interleaved A/B session (release, n ≥ 5) |
| C — single glTF parse + install marks (candidate 4, first half) | model sweep in `crates/postretro/src/startup/`, hit-zone store in `postretro-sim`, `postretro-model` as needed; the model, texture, sprite/fog/host marks; line C stops double-counting the parse | Hit-zone mark ≈ 0 on both maps; hit-zone and clip tables equal before/after on every model the dev mod loads |

## Track A outcome

Built: `NodeMap` (renderer) and `DenseNodeOwners` (`postretro`) replace the per-probe `BTreeMap<StoredNode, _>` and `BTreeMap<DenseNode, u32>`. Each is a flat array over the affinity-brick grid plus an ordered overflow map, so any key answers as the tree did. The old derivations stay in tests as oracles; an uncommitted check also held both crates to the oracles on the baked `campaign-test` and `stress-warren-lit`. Marks added: `[Renderer] Geometry install timing:` (phases of `geometry_upload`) and `[Startup] unload_level=`.

Measured, release, interleaved A/B on one machine, medians, stress-warren-lit, n = 4 first loads and 8 changes (ms):

| Stage | First load before → after | Level change before → after |
|---|---|---|
| `geometry_upload` | 612 → 361 | 601 → 320 |
| `streaming_preload` | 570 → 364 | 451 → 248 |
| Combined | 1182 → 735 (−447) | 1070 → 561 (−509) |

campaign-test moves by under 25 ms either way.

**The −0.8 s target was not reached.** On this machine the two tree maps cost about 0.55 s per load in total (perf: 0.43 s renderer, 0.27 s planner, each inflated by sampling), not the 1.0–1.1 s the investigation's numbers implied; the investigation's `geometry_upload` baseline was 926 ms against 612 ms here. What remains of the code Track A owns is per-probe array work over 2.7 M probes (about 130 ms renderer, 50 ms planner). It cannot fall further without sharing one product across the renderer boundary or moving the build to the worker, both of which this phase excludes. The rest of `geometry_upload` is compute-pipeline creation (candidate 3) and `sparse_compose_capacity` (about 60 ms, not node-keyed).

## Amendment after Track A

- **Targets are relative to this machine.** The investigation's absolute baselines do not reproduce here (this run of the machine is about 1.5× faster). A track's target is a share of the cost it measures itself, in one interleaved A/B session against a saved pre-change binary — never an absolute number carried from the findings.
- **Measurement practice:** apply `measure-scaffold.patch` alone (the instrumentation patch already contains it). Reverse it before `cargo fmt`, re-apply after (`scratchpad/fmt.sh`). Interleave base and new binaries (`scratchpad/ab.sh`, `run2.sh`); the first load after a pause can hit a cold page cache. `perf report` hangs here — use `perf script --no-inline -F comm,tid,time,ip,sym | rustfilt` and `scratchpad/incl.py` / `under.py`. Delete `target/debug` after a test round if `/` nears the 15 GB floor. `postretro` is a bin crate: `cargo test -p postretro --bin postretro <filter>`.
