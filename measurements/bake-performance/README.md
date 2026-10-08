# Bake performance measurement

Scripts from `context/plans/done/bake-parallelism-large-maps` for timing `prl-build` bakes and checking that a compiler change leaves output bytes unchanged. Results and the pinned measurement conditions are in that brief's `research.md`. Run outputs (`target/bake-measure/…`) stay local: commit transcribed numbers, not raw logs.

| Script | Purpose |
|---|---|
| `measure-hallway.ps1` | One timed bake under pinned conditions: builds release `prl-build`, bakes a map (default the hallway; `-Map <name>` for another), samples CPU and RSS every 10 s into `cpu-samples.tsv`, and writes `summary.txt`. `-Cold` runs the uncached `--release` bake; `-ReuseCacheDir <dir>` runs a second warm build on an existing cache; `-RepoRoot` bakes another checkout. Windows PowerShell 5.1 or pwsh 7. |
| `stats.sh` | Per-stage busy-core mean and percentiles and peak RSS from a `cpu-samples.tsv`. Git Bash or WSL. |
| `fixture-bytes.ps1` | Bakes a fixture set in four modes (cold `-j 1`, cold default `-j`, warm all-miss, warm all-hit) and prints one digest per `.prl`. Run it on a baseline compiler, then after a change, and diff the two outputs. |
| `prl-pathdiff.py` | Compares two `.prl` bakes of one map from different checkouts. The `.prl` embeds the absolute data-script path, so it reports where the path differs and whether everything else is identical. Assumes a `C:\Users` checkout path. |
