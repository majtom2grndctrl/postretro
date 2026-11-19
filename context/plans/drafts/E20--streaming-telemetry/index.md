# E20--streaming-telemetry

Brief · compact · Epic 20 · reads: `context/lib/development_guide.md` §6.1, §6.4 · `context/lib/networking.md` §Not netcode: the live introspection channel · `context/lib/boot_sequence.md` §Startup timing vocabulary, §4 · `context/lib/rendering_pipeline.md` §Cluster SH residency, §12 · `context/lib/build_pipeline.md` (compiler reporters) · `context/lib/testing_guide.md` §3, §Resource bounds · read at 32ac4217 · evidence: `research.md`

## Problem
Agents diagnosing a run read its diagnostics as text, and an agent attached to a running session cannot read them at all. This is a capability agents requested, and an active agent asked for these first events by name. Load phases, geometry install, SH and lightmap streaming windows, CPU stage and GPU pass timing windows, and `prl-build` bake stages each come from a typed snapshot. Each snapshot leaves its process only as a formatted log line on stderr or as reporter text. The observe-live socket carries none of it, and nothing retains recent log records. When this is done, one versioned envelope carries those diagnostics as typed events. An agent that launched the run reads them from a JSONL file. An agent attached to a session someone else started pages them from a bounded in-process ring over observe-live, with recent log records alongside. Every existing log line and every reporter's output is unchanged.

## Decisions
- **One snapshot, two renderings.** Each event is built at its log line's emit point from the same closed snapshot the line formats. No text is parsed back. Lines and reporter output stay byte-identical, because unit tests pin them and humans read them.
- **A new leaf crate owns the envelope**, its version and its sinks, on std, serde and serde_json only. Payload structs live with their producers. This is not the deferred `postretro-observability-protocol`, which waits on a second typed consumer (roadmap Epic 20). `postretro-stage-timing` stays engine-blind and serde-free.
- **Envelope semantics.**
  - Every envelope carries the run, a per-process sequence, monotonic time since process start, the source process, the running level and a kind.
  - The running level is the generation and map captured at commit, and is null between levels.
  - The sequence is the order in which events enter the ring and the file, so a cursor never skips one.
  - Time can run backward across the sequence between threads.
- **The run brackets itself.**
  - `run_start` comes first. It carries wall-clock epoch, argv, engine version, the absolute sink path, and every known producer with its state: enabled, disabled (naming the variable that enables it), pending, or unsupported (with a reason). GPU timing reads pending until the renderer has its adapter. A `producer_state` event then settles it. An absent family is therefore explained, never read as zero (the `cpu_timing` availability precedent, `networking.md`).
  - `run_end` carries the exit reason. It is written on every exit the process controls: clean, error, panic, and the `process::exit` paths of capture and headless runs. It comes after any final partial windows and is the run's last line; nothing writes after it. Only a kill leaves no `run_end`.
  - The file is the record of a run's end. A socket client may see the connection drop first.
- **Telemetry turns on no producer.** CPU and GPU timing keep their own `POSTRETRO_*` gates, and `run_start` reports them. GPU timing has adapter cost and adapter requirements, which a sink switch must not hide.
- **First kinds:**
  - **Load family.** Boot, mod-init and level-load phases with their contained marks, carrying the load's generation and its `.prl` path. Unload names the level it unloads. Geometry install is emitted by the app when it takes the load's set, under that load's generation. A set from a cancelled load emits nothing, and the renderer's own line is unchanged.
  - **Streaming windows.** SH and lightmap windows carry deltas, cumulative totals, elapsed time and the streaming session they measure. Totals restart with each session. A session that ends with counts since its last close emits a final `partial` window: at unload, on replacement within a level, on a world-less clear, or at `run_end`. A partial has no log line. An idle window emits nothing.
  - **Timing windows.** The CPU window payload is `CpuWindowReport`, the same rendering the live `cpu_timing` dump section uses. CPU windows are cleared at a level change, as today. GPU windows are never reset at a level change and may span one; each GPU window event carries the level current at its close.
  - **Bake.** Bake stage transitions in every reporter mode, and a bake summary. The summary names the source map and the output `.prl`, so a reducer joins a load to its bake. Stage durations come from the summary alone, because transitions overlap or begin late under the TUI. A failed bake's summary carries no durations.
  - **The rest:** `log`, `run_start`, `producer_state` and `run_end`.
- **File sink: `POSTRETRO_TELEMETRY=<path>`, in every build.**
  - The engine and `prl-build` both read it, and a relative path resolves against each process's working directory.
  - Each process appends one whole line per event, flushed, so a crash keeps everything before it. Event and info-log rates are per load, per window, per stage and per actionable message (§6.1), so flushing every line is cheap.
  - Captured log records go to the file as well as the ring.
  - A failed open or write warns once and disables the sink, without deadlocking against its own warning. It never blocks a frame or a bake.
- **The ring.**
  - One process-wide ring holds typed events and log records under separate count and byte caps. A log message over its cap truncates and is flagged.
  - Pinned outside the caps: `run_start`, the latest `producer_state` per producer, and the latest of each load-family kind. A superseded pin falls under the caps at once.
  - An eviction advances a dropped counter. The oldest retained sequence ignores pins.
- **Log capture.** The tee logger forwards to the existing logger with its filter unchanged. With a consumer present (the sink variable, or `--observe-live`), it raises the global max level to info and captures info and above. In `prl-build` it forwards a record to `CollectingLogger` only when that logger's own filter accepts it. With no consumer, logger setup is exactly today's, and a plain run formats no extra records.
- **Live read: a cursor-polled `events` verb on observe-live, served on the transport thread.**
  - A reply returns the pinned events at or after `since`, then the retained events from the oldest, in sequence order, with the next cursor, the oldest, the dropped count and the run.
  - A `since` beyond the next cursor, or an unknown kind in the filter, returns an error frame naming the current run and cursor.
  - The hello gains the run and protocol 2. A client finds the cursor by polling with `max` 0.
  - Polling keeps the one-connection, single-slot contract and resumes across reconnects. Server push would need a writer not tied to a reply and per-client backpressure, which `done/E20--live-socket-channel` deferred.
  - Serving off the main thread amends `networking.md`'s "the main thread builds every reply" for this verb only. That rule exists because registry state is `!Send`. The ring is engine-blind plain data, and logs matter most when the frame loop has stalled. `dump` stays on the main thread.
- **First consumer: a reducer in `tools/`.** It is stdlib Python with a unittest. It summarizes one telemetry file: a load-phase table, per-window miss buckets, CPU and GPU p50 and max across windows, and bake stage durations, joined per run and per load. It follows the reducer shape `E20--memory-visibility` set out. `tools/` ships in SDK bundles.
- **Placement.**
  - Recording is instrumentation in every build (§6.4).
  - The file sink is a surface, and it too ships in every build. This departs from §6.4, which puts surfaces behind features. The reason: the SDK engine carries only `dev-tools`, and `prl-build` has no features, so a gated sink would never reach a bundle holder's bake or run.
  - The `events` verb rides the dependency-free `observe-live` feature. The SDK engine does not enable that feature, so the live half reaches engine developers only.
  - Nothing new links in release beyond the leaf crate.
- **Non-goals:**
  - Migrating the measurement scrapers under `measurements/`: they are frozen evidence.
  - Converting other log lines into typed events: other subsystems add a kind later.
  - Building lightmap miss-cause buckets or per-stage bake cache-hit reporting: neither exists in source. The events carry what does.
  - Resetting GPU windows at a level change.
  - A size cap or rotation for the file: it is opt-in, and its growth is per load, per window and per stage.
  - A long-poll, server push and concurrent clients. Connections are served to completion, so pollers close between polls. A wait field can be added later without a protocol bump.
  - Fixing the hello's map, which is always empty.
  - Live mutating verbs, the MCP frontend, record/replay, protocol-crate extraction: separate roadmap items.
  - Memory accounting: `E20--memory-visibility` owns it.
  - Dashboards or a TUI: an Epic 20 non-goal.
  - A `docs/` page: the schema is pre-stable, and the version field marks breaks.

## Acceptance
### Automated
**One snapshot, two renderings:**
- [ ] For each engine kind, one closed snapshot yields a payload and a log line that agree field for field. The fixtures are line C sums and contained marks, a miss bucket, a CPU row's average and max, and a GPU pass mean. A GPU pass with no samples reads absent, never zero.
- [ ] With telemetry on and off, stderr is identical, the `prl-build --no-tui` stderr is identical, and the Build Summary stdout is identical. The existing log-capture and CLI-contract tests pass unchanged.
- [ ] The engine built with no optional features writes the file sink, and so does `prl-build`.

**Ordering (pins in `research.md`):**
- [ ] A loading-screen level change writes, in order: the old level's partial windows, its unload naming it, the new load's geometry install, then the new load's phases naming its generation. No unload carries a null level (P3).
- [ ] A load beside a running level reports `bundle_held`, `commit_checks` and `running_release` as phases, and its commit-frame group holds no `commit_checks`. Its geometry install carries the new load's generation while the envelope names the running level (P4).
- [ ] A load cancelled after its set build emits neither geometry install nor load phases. Its generation shows as a gap (P13).
- [ ] Events racing in from the main, upload, loader and logging threads land in strictly increasing sequence with no gap except evictions. A client polling during the race misses none (P5).
- [ ] With GPU timing requested on an adapter without timestamp queries, `run_start` reads pending, and a later `producer_state` reads unsupported with its reason, before any GPU window could close (P1).

**Streaming windows:**
- [ ] An idle SH window emits neither a line nor an event, including at the session's end.
- [ ] Unloading 2 s into an SH window that has counted something emits one partial carrying that elapsed time, and no line.
- [ ] A session replaced within a level emits one partial, and the next window's totals restart (P9).
- [ ] A CPU window cleared at a level change emits nothing. A GPU window spanning a level change names the level current at its close.

**Bake:**
- [ ] A bake fixture's events list the same stages as the Build Summary, minus the non-stage rows, in summary order. Its summary durations equal the Build Summary's in both the plain and TUI reporter paths (P7).
- [ ] A stage that produced no output emits a skip, not a finish.
- [ ] A failing bake emits the stages it began, then an unsuccessful summary with no durations.
- [ ] A bake that ends before its first stage writes `run_start` and `run_end` with its reason, and no stage events.

**Run bracketing:**
- [ ] `run_start` lists every producer: enabled with timing on, and disabled with timing off, naming the variable.
- [ ] A clean exit writes every open window's partial, then `run_end` as the last line. A record logged after `run_end` from any thread writes nothing (P6).
- [ ] An error exit in the engine and in `prl-build`, a panic, and a capture or headless exit each write `run_end` naming its reason.
- [ ] A killed process leaves every line written before the kill parseable.

**File sink:**
- [ ] An unwritable path warns once, and the run completes.
- [ ] A write that fails mid-run warns once and disables the sink without deadlocking. Later events still reach the ring and stderr (P10).
- [ ] An engine run and a bake appending to one path at the same time produce two runs with distinct ids. Every line parses and is whole, including a log record at its message cap (P11).
- [ ] A bake run from one working directory and an engine run of its output from another name the same absolute `.prl`, and the reducer joins them.

**Ring and live `events`:**
- [ ] Overfilling events by count, events by bytes, and log records by count and by bytes each evicts oldest-first. Each stays at or below its caps, advances `dropped`, and keeps the sequence strictly increasing.
- [ ] A client attaching after eviction past the latest load reads `run_start`, the latest producer states and that load's phases first, then the retained run from the oldest. A newer load arriving mid-pagination drops the superseded load's phases with `dropped` advanced (P2).
- [ ] An over-cap message truncates and is flagged.
- [ ] A `since` older than the oldest retained returns from the oldest, and the reply shows the gap.
- [ ] A `since` equal to the next cursor returns empty.
- [ ] A `since` beyond the next cursor returns an error naming the run and cursor.
- [ ] `max` 0 returns nothing and leaves the cursor unchanged (P8).
- [ ] A `kinds` filter returns only those kinds and still advances the cursor past the events it filtered out. An unknown kind returns an error.
- [ ] Two polls, the second from the first's cursor, with events written between them and no eviction, return disjoint results whose sequences run contiguous from the first `since`.
- [ ] With the main thread blocked, `events` answers at once, both before any `dump` and after a `dump` has timed out. The `dump` times out as it does today (P12).
- [ ] A protocol-1 client reads the hello, and a malformed `events` body returns an error frame.

**Log capture:**
- [ ] With a consumer present, an info record is captured while stderr's default filter still hides it, and a debug record outside the filter is not captured.
- [ ] With no consumer, the global max level is unchanged. This is checked in a test process with no other logger installed.
- [ ] In `prl-build`, an info record does not reach the TUI warnings list.

**Allocation and the reducer:**
- [ ] With the sink and the ring enabled, the streaming windows' and the CPU timer's per-frame paths allocate nothing on a frame that closes no window and logs nothing at info (`alloc_probe`).
- [ ] The reducer's unittest summarizes a checked-in two-run sample (a bake, then an engine run) into the expected per-load and per-window tables.
- [ ] Every line of that sample parses as a current envelope in the leaf crate's tests.

### Manual
- [ ] Bake a stress-warren map with `prl-build`, then run its output through `postretro-tool run`. Set the same absolute `POSTRETRO_TELEMETRY` path and both timing variables for both. The result is one file holding the bake and the engine run, and the reducer's tables match the stderr lines. Use Windows or Vulkan for GPU rows, because Metal reports GPU timing unsupported.
- [ ] An agent polling `events` on a windowed session across a level change sees the old level's unload before the new level's load phases, every streaming window tagged with the level it measured, and no unexplained sequence gap at default caps.

## Path
- **Precedents:**
  - `CollectingLogger` / `LogSink` in level-compiler, for a collecting `log` backend.
  - The live `cpu_timing` section, for an availability shape.
  - `serve_connection` / `read_request_frame`, for where the transport peeks the verb before the single-slot queue.
  - test-log-capture's sequence-then-insert, for one-lock ordering.
- **Emit points:**
  - `StartupTimings::log_level_line` and the boot and mod summary sites.
  - `unload_level`: capture the level before it is cleared.
  - The app's take of the set: the renderer hands back `GeometryInstallMarks` as plain data.
  - `ShStreamingLogWindow::line_if_due` and `LightmapStreamingLogWindow`, plus each session-ending path.
  - `FrameTimer::log_last_window`.
  - `FrameTiming::accumulate`.
  - The `Reporter` calls in level-compiler `pipeline.rs`, through a decorator around whichever reporter `ReporterMode` picks.
- **Typed views.** `StartupTimings` keeps its contained marks private. Expose a typed view; do not reparse `summary()`. Nothing keeps the running level's generation after commit, so capture it there.
- **Sink mechanics.**
  - The leaf crate holds the envelope, the JSONL sink, the ring and the tee logger.
  - Take the sequence, insert into the ring and write the line under one lock.
  - Format any warning outside that lock.
  - Serve `events` by copying out under the lock and serializing outside it.
- **Process wiring.** postretro-tool needs no change, because child processes inherit the environment. `ObserveResponse::Ok` carries `dump` today, so reshape the response vocabulary to give `events` its own body.
- **Rival shapes** (a socket-only stream; a file only, with the hello naming its path) are weighed in `research.md`.
- **First slice:** the leaf crate, `run_start`/`run_end`, line C as an event, and the file sink in the engine. This proves snapshot-to-envelope and append framing before fanning out.
- **Large files.** `startup/mod.rs` and level-compiler `main.rs` are past 800 lines. Put new logic in new modules and add only calls there.

## Open questions
- Ring cap values (counts, bytes, message cap) — **delegated**: pick from a stress-warren session's observed rates and report them with that measurement.
- Run id form (random or pid plus epoch) — **delegated**.
- Whether preflight already runs `tools/` Python tests, and wiring the reducer's unittest in if not — **delegated**.

## Boundary inventory
| Name | Rust | Wire (JSON) |
|---|---|---|
| Envelope fields | serde snake_case | `v`, `run`, `seq`, `t_ms`, `source`, `level`, `kind`, `payload` |
| Level | `Option<…>` | `{"gen": u64, "map": string}` or `null` |
| Source | enum | `"engine"`, `"prl-build"` |
| Kinds | enum, snake_case | `run_start`, `producer_state`, `run_end`, `log`, `boot_phases`, `mod_phases`, `load_phases`, `unload`, `geometry_install`, `sh_streaming_window`, `lightmap_streaming_window`, `cpu_timing_window`, `gpu_timing_window`, `bake_stage`, `bake_summary` |
| Producer state | enum | `"enabled"`, `"disabled"`, `"pending"`, `"unsupported"` |
| Env var | — | `POSTRETRO_TELEMETRY` |
| Live verb | `ObserveRequest` variant | `{"verb":"events","since":u64,"max":u32,"kinds":[string]?}` |
| Live reply | response body | `events`, `next`, `oldest`, `dropped`, `run` |
| Hello addition | `ServerHello` | `run`; `protocol` = 2 |

## Wire format
- **File:** UTF-8 JSONL, one envelope per line ending `\n`, each line written as a single append and flushed.
- **Socket:** the existing observe-live framing (little-endian u32 length prefix, JSON body, 64 KiB request cap).
- **Fields:**
  - `seq` and `t_ms` are unsigned, and `seq` starts at 0 with each process's `run_start`.
  - Absent optional payload values serialize as `null`, never `0`.
  - An empty `events` reply is `[]`, and `oldest` equals `next` when nothing unpinned is retained.
