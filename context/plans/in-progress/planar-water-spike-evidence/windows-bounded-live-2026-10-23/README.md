# Bounded planar + probe live experiment

Owner intent: probes are the intended distant layer. Finite D/R handing detail to probes is acceptable. Evaluate the useful near planar image and blend continuity, not parity with unlimited planar.

Question: do D15/D30 remove the live visibility cost measured with unlimited planar, while retaining a useful near-field planar contribution? Compare eight explicit launch settings in forward/reverse order: SSR High + probe, half planar unlimited, half D30/D15, quarter D30/D15, half D30+R15, half R8. All preserve the existing probe composite. Static standing pose, ordinary streaming, recorded actual extents. Release optimization and vsync off are a separate dataset from the earlier development-build capture/vsynced probes.

Completed: [findings and recommendation](findings.md), [tables](tables.md), `summary.json`, `audit.json`, original `runs/` logs/results/manifest/telemetry, a separate accepted `pilot/`, six `handoff-images/` stills plus two independent `handoff-repeat/` stills. Sixteen accepted live runs retain 9,600 tail CPU frames and 5,760 GPU readbacks. All eight accepted stills have complete 120-readback windows and clean counters. Quarter D15 is byte-identical to probes-only here; half/quarter D15 both reproduce exactly in independent launches.

Tools: `run.py` defaults to 25 seconds; this experiment used `--seconds 20`. It terminates its own engine process, validates effective settings/logs, and records five tail 120-frame CPU windows plus three tail 120-readback GPU windows. GPU log values are rounded to 0.01 ms; CPU/GPU windows do not align. `summarize.py` reduces `runs/results.json` into tables; `audit.py` revalidates reductions, capture counters, image sizes/hashes and source hashes. `capture_handoff.py` creates fresh static illustrations separately from the timed matrix. All launch outputs require a fresh directory.

Verification: checks with and without dev-tools, Clippy all targets with warnings denied, formatting, 10 planar tests (including two new launch controls tests) and 100 existing fluid tests passed. See logs and `review.md`. Launch seeds are dev-tools only; no player settings, visibility algorithm, protected composite shader or GPU binding changes.

Build provenance: the first cold release build failed when MSVC `lib.exe` returned 1114 during Luau packing. Successful retry: `cargo build --release -j 2 -p postretro -p postretro-script-compiler --features postretro/dev-tools,postretro/capture`, `CARGO_INCREMENTAL=0`, and worktree-local TEMP/TMP (`target/planar-bake-tmp`), finished in 16m53s. Both build logs are retained. `handoff-images-no-timing/` preserves the first illustration attempt (GPU flag omitted); it is excluded from the accepted eight. No build or bake ran alongside timed experiments. All runners closed their engine/telemetry processes.

Replay from this spike worktree after building:

```powershell
python context/plans/in-progress/planar-water-spike-evidence/windows-bounded-live-2026-10-23/run.py --output target/bounded-live-new --seconds 20
python context/plans/in-progress/planar-water-spike-evidence/windows-bounded-live-2026-10-23/capture_handoff.py --output target/bounded-stills-new
python context/plans/in-progress/planar-water-spike-evidence/windows-bounded-live-2026-10-23/audit.py
```

The audit reads the retained experiment, while replay outputs use fresh locations. No shipping preset or owner motion acceptance is claimed.
