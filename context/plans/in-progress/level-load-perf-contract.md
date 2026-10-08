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
