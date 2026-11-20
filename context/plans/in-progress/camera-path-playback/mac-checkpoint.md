# camera-path-playback — Mac playback trace runbook

A runbook for an agent on the owner's Mac. It records a camera-route playback run with a Metal System Trace and a GPU clock poll attached, then reduces them to per-frame drain GPU time. The first run (159f47d64 on `mac/camera-path-checkpoint`) settled the frame-mark and clock-source questions; `plan.md` §Delegated answers records the answers.

Follow `context/lib/rendering_pipeline.md` §12: attach, never `--launch`. Keep the window in front for the whole run. Nothing else runs beside it. **Unlock the screen first:** a locked Mac renders nothing, and the engine stalls at `[Engine] Window ready`. Keep it awake for the whole run with `caffeinate` (step 5): after the first frame, a lock or a sleeping display pauses the run with no deadline.

## Steps

Run from the repo root on branch `camera-path-playback`, pulled to the commit the owner names.

1. Build the profiling engine, the sidecar and the tool:
   - `cargo build --profile profiling -p postretro --features dev-tools`
   - `cargo build --profile profiling -p postretro-script-compiler --bin scripts-build` builds the sidecar beside the engine. This is what `xtask run` does before it launches.
   - `cargo build --release -p postretro-tool`
2. Compile campaign-test if `content/dev/maps/campaign-test.prl` is missing or stale. A stale bake fails to load with `ClusterShPayloadsVersionMismatch`: `cargo run --release -p postretro-level-compiler -- content/dev/maps/campaign-test.map -o content/dev/maps/campaign-test.prl`.
3. Generate a short far-point route: `target/release/postretro-tool generate-camera-route content/dev/maps/campaign-test.prl --kind far-point --steps 40 -o /tmp/cp-far.txt`. Note the pose count, which is the line count minus the header.
4. Start the clock poll in its own shell. It runs before, during and after the trace:
   `while true; do python3 -c 'import time; print(f"{time.time():.3f}")'; ioreg -r -c IOAccelerator -d 1 | grep -o '"Core Clock(MHz)"=[0-9]*\|"Temperature(C)"=[0-9]*\|"Device Utilization %"=[0-9]*'; sleep 0.5; done > /tmp/cp-ioreg.log`
5. Launch the engine windowed with a long hold, so there is time to attach:
   `RUST_LOG=info target/profiling/postretro --windowed --camera-route /tmp/cp-far.txt --camera-route-hold 900 --camera-route-report /tmp/cp-far.playback.json content/dev/maps/campaign-test.prl > /tmp/cp-engine.log 2>&1 &` and note the pid.
   Then hold the display and the machine awake until the engine exits: `caffeinate -di -w <pid> &`.
6. Once the level reveals (the log line `[Settling] entry`), attach within the hold:
   `xcrun xctrace record --template 'Metal System Trace' --attach <pid> --time-limit 20s --no-prompt --output /tmp/cp-far.trace < /dev/null`
   Keep the trace at 20 s or less: a 40 s trace wrote about 4.7 GB of ktrace and took over 10 minutes to save, while 20 s saved 436 MB in about 11 minutes. Give the watchdog at least 30 minutes after the engine exits. A watchdog that kills `xctrace` mid-save loses the export (`Document Missing Template Error`).
7. The engine exits 0 by itself after the last pose. Confirm the exit code and that `/tmp/cp-far.playback.json` exists. Stop the `ioreg` loop.
8. Export the table of contents and both interval tables into one directory. The reducer needs both interval tables there. It finds the TOC there too, or takes it from `--toc`. Without a TOC it still runs, but cannot line the clock poll up with the trace, so it reports the clock over the whole poll log:
   - `mkdir /tmp/cp-far-export`
   - `xcrun xctrace export --input /tmp/cp-far.trace --toc > /tmp/cp-far-export/toc.xml`
   - `xcrun xctrace export --input /tmp/cp-far.trace --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-application-intervals"]' > /tmp/cp-far-export/metal-application-intervals.xml`
   - `xcrun xctrace export --input /tmp/cp-far.trace --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' > /tmp/cp-far-export/metal-gpu-intervals.xml`
9. Run the reducer: `python3 tools/playback_trace_reduce.py --record /tmp/cp-far.playback.json --trace-dir /tmp/cp-far-export --ioreg /tmp/cp-ioreg.log > /tmp/cp-far-reduce.txt; echo "reducer exit $?"`. It exits 2, naming the cause on stderr, on a malformed record, an unreadable table, or a record with no frame matching a `pr-frame` mark. The redirect captures stdout only, so an error leaves the output file empty: check the exit code, and keep the stderr text, before returning.

## Return to the integrating session

- The engine exit code, the last 40 lines of `/tmp/cp-engine.log`, and `/tmp/cp-far.playback.json`.
- `/tmp/cp-far-reduce.txt`, the reducer's output, and its exit code (with its stderr if non-zero).
- `/tmp/cp-far-export/` and `/tmp/cp-ioreg.log`. Compress the tables with `xz` if they are large; the reducer reads `.xml.xz` directly.
- Delete the `.trace` bundle and the `instruments*.ktrace` file in `$TMPDIR` after the exports are copied.
