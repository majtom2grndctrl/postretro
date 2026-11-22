# Camera routes: repeatable flights for streaming measurement

A camera route is a file of camera poses that the SDK engine flies through, one pose per frame. The same route makes the same streaming demand every time it plays. Use it to see how your level streams lightmaps and probe lighting as the camera moves, to compare two settings on the same path, or to let an agent replay a flight you made by hand.

Routes are a measurement tool. Player builds made with `dist` do not include them, and no player setting names them.

Every command below runs from an SDK bundle root. On Windows, use `bin\postretro-tool.exe`.

The commands below keep routes in a `routes` folder. Make it first with `mkdir routes`. Neither the tool nor the engine creates a missing folder. Each stops with an error naming it before doing anything else, so a recording or a report never fails to save at the end of a run.

## Making a route

### Generate one from a level

`postretro-tool generate-camera-route` reads a compiled level and writes a route. It needs no GPU and doesn't start the engine.

```bash
bin/postretro-tool generate-camera-route content/<mod>/maps/<name>.prl --kind far-point -o routes/far-point.txt
```

`-o` must name a file in an existing folder. If that file exists, it must be an earlier route: the tool refuses to overwrite the level it reads or any other file.

`<mod>` is your mod's folder under `content/`. This command takes the level's path from the folder you run it in, so it needs the `content/<mod>/` part. `postretro-tool run`, below, takes a path inside the mod folder, `maps/<name>.prl`.

The level needs at least two rooms joined by a portal. On a level without one, the command stops and says so.

| `--kind` | What it flies |
|---|---|
| `far-point` | Repeatedly heads for the farthest unvisited room. It reaches new areas quickly and makes the heaviest streaming demand. |
| `random` | Wanders from room to room at random. |
| `smoothed` | The same rooms in the same order as a `far-point` or `random` walk (`--walk`), flown at run speed with a capped turn rate. Use it to watch a route on screen or to measure stutter. |

`far-point` and `random` jump to a new spot inside the room and turn a quarter turn every frame. That gives the heaviest streaming load, but it flickers on screen. To watch a route, generate `smoothed`.

| Option | Default | Meaning |
|---|---|---|
| `--steps <n>` | 2000 | Rooms the walk visits |
| `--seed <n>` | the built-in seed | Picks a different walk; decimal or `0x` hex |
| `--pitch <degrees>` | 0 | Camera pitch, between −89 and 89 |
| `--hfov <degrees>` | 100 | Horizontal field of view, 60–130 |
| `--walk far-point\|random` | `far-point` | `smoothed` only: the walk whose rooms it follows |
| `--speed <m/s>` | 8 | `smoothed` only: flight speed |
| `--turn-cap <deg/s>` | 270 | `smoothed` only: the fastest the camera turns |

The same level and options always write the same route. The file records the options it was made with and which bake of the level it belongs to.

### Record one by flying it

Launch the engine with `--record-camera-route` and play normally:

```bash
bin/postretro-tool run maps/<name>.prl --record-camera-route routes/my-flight.txt
```

The recording starts on the first frame of the first level you play. It does not start on a menu backdrop, and it skips the loading hold. It stops when that level ends or you quit, and it is written once, then. It keeps where you stood and looked, not head bob, camera roll or field-of-view kicks. It is saved at 60 poses per second, whatever frame rate you played at.

A recording follows the camera only. When it plays back, the world starts as the level starts: doors you opened while recording are closed again.

A recording takes the same stall timeout as playback, `--camera-route-stall-timeout <seconds>` (300 by default). If the level you record never shows on screen in time, the engine exits with an error and writes no file. A menu or a loading screen does not count.

## Playing a route

```bash
bin/postretro-tool run maps/<name>.prl --camera-route routes/far-point.txt
```

The engine loads the level, waits until what the first pose sees is ready, holds that first pose, flies the route, writes a report, and exits. Your player stays at the spawn, and movement keys do nothing during the run. The diagnostic chords and the Diagnostics panel still work.

| Option | Default | Meaning |
|---|---|---|
| `--camera-route <file>` | — | The route to play |
| `--camera-route-pacing frame-locked\|wall-clock` | `frame-locked` | `frame-locked` draws pose N on frame N, so every run makes the same demand at any frame rate. `wall-clock` keeps the route's own speed and skips poses when frames are slow, as a player would see it. |
| `--camera-route-hold <frames>` | 600 | Frames to hold the first pose before flying, for example to attach a profiler; at most 100000 |
| `--camera-route-report <file>` | `<route name>.playback.json` in the bundle root | Where the report goes. `postretro-tool run` starts the engine in the bundle root, so a relative path is relative to it. |
| `--camera-route-stall-timeout <seconds>` | 300 | Give up if no frame of the run (a hold or route frame) has shown in time, even while the window shows a menu or a loading screen; at most 86400 (a day) |
| `--present-mode vsync\|no-vsync` | `vsync` | `vsync` shows the stutter a player sees. `no-vsync` shows how long each frame takes to make. |

`./postretro --help` (`postretro.exe --help` on Windows), run from the bundle root, lists these options with every other engine launch flag. It opens no window.

The engine refuses to start, and names the file and the reason, when:
- the route is empty, malformed or unreadable
- its field of view is outside 60–130°, or a pose's pitch is past ±89° (the error names the pose's line)
- the report's folder doesn't exist, the report path ends in `/`, `\`, `.` or `..`, the report would overwrite the route itself, or it names an existing file that isn't an earlier report
- the launch also has `--connect` or `--record-camera-route`
- an option is given twice, or a value is outside its range

`--record-camera-route` refuses to start the same way when its file's folder doesn't exist, its path ends in `/`, `\`, `.` or `..`, or it names an existing file that isn't an earlier route.

A route made for a different bake of the level is caught when the level loads, not before launch: the window opens and the level loads, then the engine exits with an error naming the route and the level. No report is written.

Rebuilding a level that changes its rooms invalidates the routes made for it. Generate them again.

Only the bundle's authoring engine, the `postretro` at the bundle root that `postretro-tool run` starts, plays and records routes. The release engine in `bin/` is a player build: it logs a warning for `--camera-route`, `--record-camera-route` and `--present-mode` and runs as if they were not there.

A run that can't finish exits with an error and still writes its report, marked aborted with the reason. That happens when the level changes, unloads or reloads mid-run, when you join a game from the menu before or during the run, or when no frame of the run shows before the stall timeout. Closing the window or quitting before the route ends also writes the report marked aborted, and the engine exits with an error. If the engine stops on an error of its own mid-run, the report names that error.

### Keep the display awake

A locked screen or a sleeping display stops the window from drawing. Until the run's first hold or route frame shows (for a recording, its first recorded frame), the run gives up when the stall timeout runs out, even if the window is showing a menu or a loading screen. After that frame, the run waits until the window shows again, with no deadline, so an unattended run can sit there with no report. Keep the window in front, and keep the display awake for the whole run:

- **macOS:** start the run under `caffeinate`, which holds the display and the machine awake until the command exits:

  ```bash
  caffeinate -di bin/postretro-tool run maps/<name>.prl --camera-route routes/far-point.txt
  ```

- **Windows:** turn on PowerToys Awake with "Keep screen on", or set **Settings → System → Power → Screen and sleep** to *Never* for the run.

Neither stops a lock you make yourself.

The first time the bundle's engine starts, it shows the accessibility panel before any level loads. A route or a recording waits behind it until the stall timeout runs out and then fails, naming the panel. Start the engine once and close the panel before you hand routes to an agent. It stays closed after that.

### Changing the streaming budget for a run

`POSTRETRO_PLAY_DRAIN_CAP_MIB` sets how many MiB of streamed lighting the engine may install per frame during play. The default is 8. Use a value from 1 to 32 in steps of 0.5 (1, 1.5, 2, and so on up to 32), the slider's own range. It sets the starting value of the Play drain cap slider in the Diagnostics panel's Streaming tab:

```bash
POSTRETRO_PLAY_DRAIN_CAP_MIB=4 bin/postretro-tool run maps/<name>.prl --camera-route routes/far-point.txt
```

On Windows PowerShell, set the variable first with `$env:POSTRETRO_PLAY_DRAIN_CAP_MIB = "4"`, then run the command without the prefix. It stays set for the rest of that PowerShell window; clear it with `Remove-Item Env:POSTRETRO_PLAY_DRAIN_CAP_MIB`. The same goes for the other variables on this page.

A value the engine can't read, or one outside that range or between its steps, logs a warning and keeps the default. The loading hold uses its own budget of 32 MiB, larger than the default, either way.

## The report

The report is a JSON file. Its header records:
- how the run was set up: route, pacing, hold, present mode, drain cap
- the machine: GPU, window and render sizes
- whether the level finished loading or timed out
- whether CPU and GPU timing were on

It then holds one row per held or flown frame. Each row has:
- the frame time
- the bytes and items of lightmap and probe lighting installed that frame
- whether that install had to create a new upload buffer (`cold`) or grow a lightmap pool
- how many lightmap blocks and probe clusters the frame drew before they were ready

Some rows are marked, and they stay in the report:
- **Hold rows** are the frames before the route starts flying.
- **No-draw rows** streamed but did not reach the screen.
- **Change marks** start on the frame where you toggled vsync; resized the window, moved it to a display with a different scaling or changed the render resolution; changed window mode; moved the drain cap slider or pressed its production button; or hot-reloaded scripts. The mark stays on every row after it.
- **Gap marks** sit on the rows whose frame time spans a stretch when the window was hidden, a frame that didn't reach the screen, or the engine making room for more report rows.

Leave marked rows out of frame-time figures.

To add per-stage CPU times to each row, set `POSTRETRO_CPU_TIMING=1`. On GPUs that support timestamp queries, `POSTRETRO_GPU_TIMING=1` adds each frame's GPU time and the time its streaming installs took on the GPU.

With `POSTRETRO_TELEMETRY=<file>`, the run also writes one `playback_window` event per 120 rows to the telemetry file, and a final partial one for the rows left when the run ends: frame-time p99, frames over 33 ms, the streaming stages' CPU maximum, and late block and cluster counts.

### GPU time on a Mac

Some Mac GPUs have no timestamp queries. There, GPU time comes from an Instruments Metal System Trace instead. During playback, each frame's GPU work is labelled `pr-frame <N> <site>`. N is the `frame` of that frame's report row, and the site says which work it is: `lightmap` and `sh` for the two streaming installs, `frame` for the frame's rendering. The `sh` time also includes the updates to movers' lighting depth tiles, and any growth of its pools, made during that install.

When you trace, start `caffeinate -di` in its own terminal before the first run and stop it after the last export. Don't wrap each run in `caffeinate` the way *Keep the display awake* shows: a trace keeps saving for many minutes after the engine exits, and if the screen locks then, the next run stalls.

1. In another terminal, start logging the GPU clock, and leave it running until the trace is saved:

   ```bash
   while true; do python3 -c 'import time; print(f"{time.time():.3f}")'; ioreg -r -c IOAccelerator -d 1 | grep -o '"Core Clock(MHz)"=[0-9]*\|"Device Utilization %"=[0-9]*'; sleep 0.5; done > ioreg.log
   ```

2. Play the route with a long hold, for example `--camera-route-hold 900`. Find the engine's process id with `pgrep -x postretro`.
3. Once the level shows, attach a trace during the hold, for 20 seconds or less. Longer traces write gigabytes, and even a 20-second trace takes many minutes to save.

   ```bash
   xcrun xctrace record --template 'Metal System Trace' --attach <pid> --time-limit 20s --no-prompt --output run.trace
   ```

4. When the trace has saved, stop the clock log and export the trace's table of contents and its two interval tables into one folder:

   ```bash
   mkdir trace
   xcrun xctrace export --input run.trace --toc > trace/toc.xml
   xcrun xctrace export --input run.trace --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-application-intervals"]' > trace/app-intervals.xml
   xcrun xctrace export --input run.trace --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' > trace/gpu-intervals.xml
   ```

5. Run the reducer:

   ```bash
   python3 tools/playback_trace_reduce.py --record far-point.playback.json --trace-dir trace --ioreg ioreg.log
   ```

It reports each frame's streaming GPU time and the run's frame-time figures, and names the GPU clock the figures ran at. Without `--ioreg`, the clock reads unrecorded. The table of contents lines the clock log up with the trace. If it is somewhere else, name it with `--toc <file>`. Without it, the clock is reported over the whole log, and the reducer says so. A GPU running at a low idle clock makes every figure an upper bound.

A frame counts as over budget when its frame time is above 16.667 ms, one frame at 60 Hz. Change that with `--budget-ms <ms>`. The telemetry `playback_window` event counts frames over 33 ms instead, so the two counts differ. By default the per-frame tables list only frames that installed something, or that ran over budget while an earlier install may still have been on the GPU. Add `--all-frames` to list every frame, or `--json` for everything the reducer computed.

A figure the trace can't settle reads `missing`, never a guess. A trace from a build older than the site labels has `pr-frame <N>` alone, and some of its streaming work can't be assigned to either install. Those frames' install times read `missing`, and the reducer reports that work on its own line. A label naming a site this reducer doesn't know is counted on its own line and never added to either install.

## Timing figures need a measurement build

The SDK bundle's engine is its authoring engine, a debug build. Use it to check that a route plays and what it streams, not how long frames take. Frame and GPU times from it run far slower than a shipped game.
