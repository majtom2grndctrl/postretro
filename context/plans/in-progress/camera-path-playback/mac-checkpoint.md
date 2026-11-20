# camera-path-playback — Mac checkpoint (Task 4)

A runbook for the agent on the owner's Mac. It answers two delegated questions with a real export (`plan.md` §Delegated answers):

1. Does a Metal System Trace export carry the playback frame marks (`pr-frame <k>` debug groups) on the "Streamed Lightmap Drain" and "Streamed SH Upload Copies" encoders, so drain GPU time attributes to a frame index?
2. Does `ioreg` report a usable GPU core clock during a windowed run, polled beside `xctrace`?

Follow `context/lib/rendering_pipeline.md` §12: attach, never `--launch`. Keep the window in front for the whole run. Nothing else runs beside it. **Unlock the screen first:** a locked Mac renders nothing, and the engine stalls at `[Engine] Window ready`.

The first run (159f47d64 on `mac/camera-path-checkpoint`) answered both questions; `plan.md` §Delegated answers records them. Keep this runbook for later Mac runs; the fixes below come from that run.

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
6. Once the level reveals (the log line `[Settling] entry`), attach within the hold:
   `xcrun xctrace record --template 'Metal System Trace' --attach <pid> --time-limit 20s --no-prompt --output /tmp/cp-far.trace < /dev/null`
   Keep the trace at 20 s or less: a 40 s trace wrote about 4.7 GB of ktrace and took over 10 minutes to save, while 20 s saved 436 MB in about 11 minutes. Give the watchdog at least 30 minutes after the engine exits. A watchdog that kills `xctrace` mid-save loses the export (`Document Missing Template Error`).
7. The engine exits 0 by itself after the last pose. Confirm the exit code and that `/tmp/cp-far.playback.json` exists. Stop the `ioreg` loop.
8. Export: `xcrun xctrace export --input /tmp/cp-far.trace --toc > /tmp/cp-far-toc.xml`. Then export every table whose schema mentions Metal GPU intervals, encoders or command buffers, plus any signpost or debug-group table (for example `metal-gpu-intervals`, `metal-application-encoders-list`, `metal-command-buffer-completed`, `os-signpost`). Use `--xpath '/trace-toc/run[@number="1"]/data/table[@schema="<schema>"]'` and write one file per schema.
9. Look for the marks: `grep -l 'pr-frame' /tmp/cp-far-*.xml`. For one table that has them, note how a `pr-frame <k>` row relates to a "Streamed Lightmap Drain" row: the same row, a parent or child row, or only overlapping time.

## Return to the integrating session

- The engine exit code, the last 40 lines of `/tmp/cp-engine.log`, and `/tmp/cp-far.playback.json`.
- `/tmp/cp-far-toc.xml` and each exported table. Compress them if they are large; keep the raw XML.
- `/tmp/cp-ioreg.log`.
- The answers: are `pr-frame` marks present (in which tables and how they nest with encoders), and did `Core Clock(MHz)` read non-zero while the route ran?
- Delete the `.trace` bundle and the `instruments*.ktrace` file in `$TMPDIR` after the exports are copied.
