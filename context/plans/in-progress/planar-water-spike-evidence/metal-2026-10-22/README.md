# Metal measurement evidence

Collected 2026-10-22. See `../../planar-water-spike-findings.md` for interpretation and caveats.

- `*-summary.json`: 240-frame GPU interval statistics and source capture CPU windows.
- `planar.json`, `ssr-high.json`, `ssr-medium.json`: original capture reports.
- `repeat-summary.json`: six untraced run medians; individual reports and logs retained.
- `manifest.json`: binary/map identity, features and raw-data location.
- Scene JSONs preserve the actual paths used in the run. Output paths can be changed before replay.
- `extract.py`: extraction logic. Run the copy in `target/planar-spike-measurements/`, where the raw XML resides: `python3 target/planar-spike-measurements/extract.py planar ssr-high ssr-medium`.

Raw bundles: `target/planar-spike-measurements/{planar-valid,ssr-high,ssr-medium}.trace`. Preserve them before deleting target. Open them in Instruments for the complete timeline. The original GPU and application interval XML exports are beside the bundles.

## Reproduction

Build `cargo build -p postretro --features dev-tools,capture`. From the workspace root, record each mode separately. Use `POSTRETRO_PLANAR_REFLECTION=1` for planar and `0` for SSR, with the corresponding scene file. Use a new trace path each time:

```sh
xcrun xctrace record --template 'Metal System Trace' --output /tmp/new-planar.trace --time-limit 30s --no-prompt --env POSTRETRO_PLANAR_REFLECTION=1 --env POSTRETRO_CPU_TIMING=1 --env POSTRETRO_SH_STREAMING=sync-proof --env RUST_LOG=info --target-stdout /tmp/new-planar.log --launch -- "$PWD/target/debug/postretro" --capture context/plans/in-progress/planar-water-spike-evidence/metal-2026-10-22/planar-scene.json
xcrun xctrace export --input /tmp/new-planar.trace --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' --output /tmp/new-planar-gpu.xml
```

Host access is required; sandbox Instruments may crash and sandbox Metal may expose no adapters. Wait for capture and trace finalization/export to finish before starting the next measurement. The trace launch time limit does not bound offline trace finalization.

`repeat.py` records the untraced repetition order. It uses the capture binary relative to the workspace and its own folder for scenes/outputs when replayed. Keep the original evidence intact by copying the script and three base scenes to a fresh directory before rerunning.
