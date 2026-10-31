# Surface radiance cache measurements

Task 8 of `context/plans/in-progress/surface-radiance-cache`: the scripted runs behind the Measurement rows (AC 34-37) and the prep for the owner A/B (AC 38). Report-only, no gates. Numbers: `results.md`, `results/results.json`, raw excerpts in `results/raw/`. Run outputs (PRLs, caches, full stderr logs) stay in a scratch directory and are not committed.

## Machine and binaries

- Intel(R) Core(TM) i9-9980HK @ 2.40 GHz, 16 logical cores, macOS; `prl-build` default workers 14.
- Before: a cargo `--release` `prl-build` built from base commit d2bda6032 (the one-sample estimator). After: `cargo build --release -p postretro-level-compiler` at HEAD 194113525 -> `target/release/prl-build`. Both get identical flags. Put a current `scripts-build` beside the base binary (data-script maps need it).
- Perf discipline for this Mac (see `feedback` notes: slowdown regime, screen saver): `caffeinate -dims` on every timed run, one bake at a time, `pgrep -fl 'cargo|prl-build|rustc'` clean first (the scripts refuse to start otherwise), pilot one config before a batch, abort on first failure.

## Scripts

| Script | Purpose |
|---|---|
| `run-bake.sh <before\|after> <repo-relative .map> <label> [outdir]` | One cold `--release --no-tui` bake; writes `<label>.{out,err}.log`, `.summary.txt` (Build Summary), `.time.txt`; deletes the PRL unless `KEEP=1`. `SCRATCH` env overrides the scratch dir that holds the base binary (`$SCRATCH/bin/prl-build-base`) and the scratch `baked/`. |
| `run-bake-ts.sh` | Same, with each stderr line prefixed by seconds since start (`ts.py`), needed for stage and progress timing. |
| `ac36-batch.sh` | AC 36: campaign-test, stress-warren-mini, hallway-mini, before then after, plus a second campaign-test pair. |
| `gen_light_sweep.py` | Sealed hall + pillars + an a x b grid of static lights (`--fill` adds worldspawn `_bounce_fill`, `--tex` picks the texture). |
| `ac35-batch.sh`, `ac35-extend.sh` | AC 35: 16 / 32 / 64 lights, then 128 / 256. After binary first, then before. |
| `summarize.py`, `phases.py` | Build Summary rows to JSON; SH Bake split from a timestamped log (see limits below). |
| `warm-edit.sh`, `cache-window.py` | AC 34 / 37 procedure (owner section below). |
| `ab/` | A/B maps, `build.sh`, `AB.md`. |

## How to re-run

```bash
# repo root, nothing else building
cargo build --release -p postretro-level-compiler
export SCRATCH=/some/scratch/dir          # holds bin/prl-build-base (+ scripts-build) and receives runs/
measurements/surface-radiance-cache/ac36-batch.sh    # about 40 min
measurements/surface-radiance-cache/ac35-batch.sh    # about 15 min
```

hallway-mini cold runs write a 1.5 GB PRL (deleted after the Build Summary is captured); keep 5 GB free.

## Limits of the phase split

`surface_radiance` logs no timed line per phase. The log offers: stage-begin lines (`SH volume bake...`, `Delta SH volume bake...`), the `SH Bake` progress bar, `[cache] sh_patch_layer hit|miss`, `[cache] sh_base_volume hit|miss` and the SH base working-set gate line (predicted bytes: layers, sum, later bounces, probe gather, id-34 entry). `phases.py` splits SH Bake into a plateau (patch shading plus setup) and a ramp (later bounces plus gather). Patch shading vs setup, and later bounces vs gather, are not separable without timed log lines; that is the one missing piece for AC 34's "timed apart".

## Owner-launched rows

Both use `warm-edit.sh`, which works on a scratch copy of the map (the working tree stays clean), fills a private empty cache with a first warm build (A), edits one static point light (+10% intensity, first plain `light` entity), and rebuilds warm (B). It records timestamped stderr, Build Summary, `[cache]` hit/miss counts, the SH phase split and the cache-window bytes. Needs about 25 GB free for the full hallway (cache plus PRLs), a fresh `--cache-dir`, nothing else running, `caffeinate`. Sleep disabled; the full hallway cold SH bake is the long pole (Lightmap Bake dominated the mini at ~340 s; the full map ran about 9 h in `bake-performance`).

### AC 34 — warm one-light edit on the full hallway, against today's warm build

```bash
cd <repo root>
cargo build --release -p postretro-level-compiler
S=<scratch dir>        # holds bin/prl-build-base (+ scripts-build beside it); do NOT reuse a cache dir
HW=$PWD/content/dev/maps/stress-warren-hallway-inspection.map
measurements/surface-radiance-cache/warm-edit.sh $S/bin/prl-build-base   $HW $S/hw-warm-before   # today's warm build
measurements/surface-radiance-cache/warm-edit.sh $PWD/target/release/prl-build $HW $S/hw-warm-after
```

Report from each `<workdir>/B.*` (and `A.*` as the empty-cache baseline): `B.out.log` Build Summary (SH Bake, Delta SH Bake, Total), the printed `[cache]` counts (expect after: all patch layers hit but one, `sh_base_volume` miss; before: `sh_group` mostly hit), and `B.phases.json` (`patch_plateau_s` = patch shading plus setup, `rest_s` = later bounces + gather). Run the binaries one after the other, never together. Compare B(after) against B(before) for SH Bake; the plan expects the warm after-build near its cold SH cost (the whole-volume gather reruns on any light edit) and the before-build much lower. Windows owner: the equivalent is `measure-hallway.ps1 -ReuseCacheDir <first run's prl-cache>` after editing one light in a copy of the map; PowerShell does not timestamp stage progress, so use the Mac script for the phase split or pipe `prl-build` stderr through a timestamper.

### AC 37 — hallway cache peak bytes, layer + id-34 entry bytes vs `--cache-max-size`

Use the same A build (`hw-warm-after/A.*`), whose cache dir `hw-warm-after/cache` holds one full map's entries:

```bash
du -sk $S/hw-warm-after/cache                         # total cache bytes after one full build (peak: no prune ran mid-build)
cat $S/hw-warm-after/A.cache.json                     # id-34 entry bytes and layer bytes (heuristic, below)
grep -a 'SH base working-set gate' $S/hw-warm-after/A.err.log   # predicted peak, layers, sum, later bounces, id-34 entry
grep -a 'maximum resident' $S/hw-warm-after/A.err.log           # process peak RSS (/usr/bin/time -l)
```

- Cache peak bytes: the directory size above; the budget `--cache-max-size` defaults to 2 GiB and is applied by the next build's start-of-build prune, so also run a third build with `--cache-max-size 2GiB` (default) and read the end-of-build cache budget report in `prl-build`'s log to see whether the map's kept set fits.
- Patch-layer plus id-34 entry bytes: `cache-window.py` sums entries written during the SH Bake stage window and attributes by size: the largest is the id-34 entry, entries in a size class shared by more than 50 files are other stages and are excluded, the rest are layers. Entries are hash-named, so this is a heuristic; cross-check the layer count against the `sh_patch_layer miss` count (should equal the static light count that has patches in reach) and compare the sum with the gate line's predicted layers + id-34 entry. Divide by `--cache-max-size` (2 GiB default) for the fraction.
- In-memory peak against the gate prediction: A's `maximum resident set size` against `estimated peak` in the gate line.

Pilot of the whole procedure (both scripts, both binaries) on campaign-test is in `results.md` and `results/raw/warm-*`.
