# Glossy world reflections, Task 12b: M8 probe-stage compile time

Row M8 of `context/plans/done/glossy-world-reflections/`: probe-stage compile time on the stress fixture plan.md names (checkpoint 1: the private project's largest map by source size, compiled cold), at a stated worker count, against the build before on the same fixture.

- **Fixture:** the private project's largest map, 5 038 791 bytes of `.map` source, compiled from a scratch copy of that project's mod root (the map file copied, every other entry of the mod root symlinked read-only) with `--baked-root` pointing at a scratch copy of the project's own baked material root, so neither compiler wrote into the project. Same map bytes for both binaries.
- **Before:** `prl-build` at `8dda8a34a` (pre-branch; no `--glossy-reflections` flag exists), as the pre-branch build compiles the map.
- **After:** `prl-build` at `ca71993bf` (HEAD's production source is unchanged since), with `--glossy-reflections`.
- **Command:** `nice -n 10 prl-build <map> -o <scratch> --no-cache --no-tui -j 6 --baked-root <scratch copy>` (the project's own build script runs at nice 10). 6 workers. Cold: `--no-cache`, so no stage reads or writes the cache.
- **Machine:** the 5300M MacBook Pro (16 logical CPUs, 32 GB), nothing else building or rendering; runs in order before, after, after, before.
- **Figure:** the build summary's own `Reflection Probes` stage time.

| run | Reflection Probes stage s | whole compile s |
|---|---|---|
| before r1 | 1.54 | 540.0 |
| after r1 | 14.30 | 579.2 |
| after r2 | 14.23 | 582.9 |
| before r2 | 1.54 | 562.1 |
| **before, mean** | **1.54** | 551.0 |
| **after, mean** | **14.27** | 581.0 |

4/4 runs completed (exit 0). The probe stage takes 12.7 s more with glossy placement (×9.3). With the flag, glossy placement fills the 256-probe cap and drops the rest: the build warns that 1908 glossy world probes past the cap were dropped (smallest bound area first), as Task 1's census predicted for this content (the Block section of `plan.md`). The prefilter's estimate in `plan.md` (Corrections, Task 7: about 40 ms per glossy probe, about 10 s CPU at 256 probes) accounts for most of the added stage time. The whole compile is about 30 s longer (+5%); the 17 s outside the probe stage is within the two before runs' own 22 s spread.
