# E20--streaming-telemetry

Brief · compact · Epic 20 · reads: `context/lib/development_guide.md` §6.1, §6.4 · `context/lib/networking.md` §Not netcode: the live introspection channel · `context/lib/boot_sequence.md` §Startup timing vocabulary · `context/lib/rendering_pipeline.md` §Cluster SH residency, §12 · `context/lib/build_pipeline.md` (compiler reporters) · `context/lib/testing_guide.md` §Resource bounds · read at 32ac4217 · evidence: `research.md`

## Problem
Agents diagnosing a run read its diagnostics as text, and an agent attached to a running session cannot read them at all. This is a capability agents requested, and an active agent asked for these first events by name. Load phases, SH and lightmap streaming windows with their miss buckets, CPU stage and GPU pass timing windows, and `prl-build` bake stages each come from a typed snapshot. Each snapshot leaves its process only as one formatted log line on stderr, or as reporter text from the compiler. The observe-live socket carries none of it, and nothing retains recent log records. When this is done, one versioned envelope carries those four families as typed events. An agent that launched the run reads them from a JSONL file. An agent attached to a session someone else started pages them from a bounded in-process ring over observe-live, with recent log records alongside. Every existing log line and every reporter's output is unchanged.

## Decisions
- **One snapshot, two renderings.** Each event is built at its log line's emit point from the same closed snapshot the line formats. No text is parsed back. Lines and reporter output stay byte-identical, because unit tests pin them and humans read them.
- **A new leaf crate owns the envelope.** It holds the envelope, the version constant, the JSONL file sink, the ring and the tee logger, on std, serde and serde_json. postretro, renderer and level-compiler depend on it. Payload structs live with their producers. This is not the deferred `postretro-observability-protocol`, which waits on a second typed consumer (roadmap Epic 20). `postretro-stage-timing` stays engine-blind and serde-free.
- **Envelope.** Each envelope has a schema version, run id, per-process monotonic sequence, monotonic milliseconds since process start, source (`engine` | `prl-build`), level, kind and payload. Level names the running level by its `LevelLoads` generation and map, and is null between levels. Load-family events also carry their own load's generation, because a load beside a running level differs from the running one.
- **The run brackets itself.** `run_start` carries wall-clock epoch, argv, engine version, and every known producer with its state. The states are enabled, disabled (naming the variable that enables it) or unsupported (with a reason, such as no timestamp queries on the adapter). An absent family is therefore explained, never read as zero (the `cpu_timing` availability precedent, `networking.md`). `run_end` carries the exit reason. A file with no `run_end` is a crash or a kill.
- **Telemetry turns on no producer.** CPU and GPU timing keep their own `POSTRETRO_*` gates, and `run_start` reports them. GPU timing has adapter cost and adapter requirements, which a sink switch must not hide.
- **First kinds:** boot, mod-init and level-load phases with their contained marks, unload, geometry install, the SH and lightmap streaming windows, the CPU and GPU timing windows, bake stage transitions and the bake summary, plus `log`, `run_start` and `run_end`.
  - Streaming windows carry the window's deltas, the cumulative totals and the elapsed time, so any two events difference cleanly across a drop or a late attach.
  - A streaming window still open at unload or `run_end` emits a final `partial` event with no log line.
  - CPU and GPU partial windows stay discarded, as `discard_partial` already rules for level changes.
  - The CPU window payload is `CpuWindowReport`, the same rendering the live `cpu_timing` dump section uses, so the two cannot drift.
  - Load phases carry the `.prl` path. The bake summary carries the source map, the output `.prl` and cache skips, so a reducer joins a load to its bake.
- **File sink: `POSTRETRO_TELEMETRY=<path>`, in every build.** Both the engine and `prl-build` read it, open the file for append, write one whole line per event and flush each line. Event rates are per-load, per-window and per-stage, so flushing every line is cheap and a crash keeps everything written before it. postretro-tool needs no change, because the processes it runs inherit the environment. A failed open or write warns once and disables the sink, and never blocks a frame or a bake.
- **Bake events come from a `Reporter` decorator** in `prl-build`. It wraps whichever reporter `ReporterMode` picks. The plain and TUI reporters are untouched.
- **The ring.** One process-wide ring holds typed events and log records, under separate count and byte caps. `run_start` and the latest event of each one-shot load kind are pinned outside the caps, so a late attach still sees what is running and how the level loaded. A log message over its cap truncates and is flagged. An eviction advances a dropped counter, so `seq` gaps are explained.
- **Log capture.** The tee logger forwards to the existing logger with its filter unchanged. With a consumer present (the sink variable, or `--observe-live`), it raises the global max level to info and captures info and above. In `prl-build` it forwards a record to `CollectingLogger` only when that logger's own filter accepts it. With no consumer, logger setup is exactly today's, and a plain run formats no extra records.
- **Live read: a cursor-polled `events` verb on observe-live, served on the transport thread.**
  - A request carries `since`, `max`, an optional `kinds` filter, and an optional `wait_ms` long-poll capped at 2000 ms.
  - A reply returns the events, `next`, `oldest`, `dropped` and `run`. The hello also gains `run`, `oldest` and `next`. Protocol goes from 1 to 2.
  - Polling over the existing request/response transport keeps the one-connection, single-slot contract, and resumes across reconnects. Server push would need a writer not tied to a reply and per-client backpressure, which `done/E20--live-socket-channel` deferred.
  - The transport thread serves `events` straight from the ring. It copies out under the lock and serializes outside it.
  - This amends `networking.md`'s "the main thread builds every reply" for this verb only. That rule exists because registry state is `!Send`. The ring is engine-blind plain data, and logs matter most when the frame loop has stalled. `dump` stays on the main thread.
- **Pollers close between polls.** Connections are served to completion, so an idle open connection locks out other clients. That is today's contract, not this brief's. `wait_ms` bounds each request's latency, not the lockout.
- **First consumer: a reducer in `tools/`.** It is stdlib Python with a unittest, and it summarizes one telemetry file for an agent: load-phase table, per-window miss buckets, CPU and GPU p50 and max across windows, and bake stage durations, joined per run and per load. It follows the reducer shape `E20--memory-visibility` set out. `tools/` ships in SDK bundles, so a bundle holder can run it.
- **Placement.**
  - Recording is instrumentation in every build (§6.4).
  - The file sink is a surface, and it too ships in every build. This departs from §6.4, which puts surfaces behind features. The reason: the SDK engine carries only `dev-tools`, and `prl-build` has no features, so a gated sink would never reach a bundle holder's bake or run.
  - The `events` verb rides the existing dependency-free `observe-live` feature. The SDK engine does not enable that feature, so the live half reaches engine developers only, and bundle holders get the file.
  - Nothing new links in release beyond the leaf crate.
- **Non-goals:**
  - Migrating the measurement scrapers under `measurements/`: they are frozen evidence.
  - Converting other log lines into typed events: other subsystems add a kind later.
  - Building lightmap miss-cause buckets: the lightmap event carries the counters that exist, and gains the buckets when they land.
  - Server push and concurrent clients.
  - Live mutating verbs, the MCP frontend, record/replay, protocol-crate extraction: separate roadmap items.
  - Memory accounting: `E20--memory-visibility` owns it.
  - Dashboards or a TUI: an Epic 20 non-goal.
  - A `docs/` page: the schema is pre-stable, and the version field marks breaks.

## Acceptance
### Automated
**One snapshot, two renderings:**
- [ ] For each engine kind, one closed snapshot yields a payload and a log line that agree field for field. The fixtures are line C sums and contained marks, a miss bucket, a CPU row's average and max, and a GPU pass mean. A GPU pass with no samples reads absent in the payload, never zero.
- [ ] With telemetry on and off, stderr is identical, the `prl-build --no-tui` stderr is identical, and the Build Summary stdout is identical. The existing log-capture and CLI-contract tests pass unchanged.

**Edges:**
- [ ] An idle SH window emits neither a line nor an event.
- [ ] Unloading 2 s into an SH window emits one `partial` event carrying that window's elapsed time and no line. A CPU window discarded at a level change emits nothing.
- [ ] A load beside a running level reports `bundle_held` and `running_release`, and no `commit_checks`. Its envelope level names the running level, and its payload names the new load's generation.
- [ ] A bake fixture's event stream lists the same stages, in order, as the Build Summary, minus the non-stage rows. A cached stage emits a skip, not a finish. A failing bake emits the stages it began, then an unsuccessful summary.

**Run header:**
- [ ] `run_start` lists every producer: enabled with timing on, disabled (naming the variable) with timing off, and unsupported when GPU timing is requested on an adapter without timestamp queries.
- [ ] A clean exit writes `run_end`, and a killed process leaves every line it wrote before the kill parseable.

**File sink:**
- [ ] An unwritable path warns once, and the run completes.
- [ ] An engine run and a bake appending to one path produce two runs with distinct ids, and every line parses.

**Ring bounds:**
- [ ] Overfilling events by count, events by bytes, and log records by count and by bytes each evicts oldest-first. Each stays at or below its caps, advances `dropped`, and keeps `seq` strictly increasing.
- [ ] Pinned events survive a full ring.
- [ ] An over-cap message truncates and is flagged.

**Log capture:**
- [ ] With a consumer present, an info record is captured while stderr's default filter still hides it, and a debug record outside the filter is not captured.
- [ ] With no consumer, the global max level is unchanged.
- [ ] In `prl-build`, an info record does not reach the TUI warnings list.

**Live `events` verb:**
- [ ] `since` older than the oldest retained `seq` returns from the oldest, and `oldest` and `dropped` show the gap.
- [ ] `since` equal to `next` returns empty.
- [ ] `max` 0 returns nothing and leaves `next` unchanged.
- [ ] A `kinds` filter returns only those kinds and still advances `next` past the events it filtered out.
- [ ] Two polls with no events between them return disjoint, contiguous results.
- [ ] `wait_ms` returns as soon as an event arrives, or at the cap when none does. A request over the cap is clamped.
- [ ] With the main thread blocked, `events` still answers within its `wait_ms`, while a `dump` times out as it does today.
- [ ] A protocol-1 client reads the hello, and `events` against a malformed body returns an error frame.

**Allocation:**
- [ ] With the sink and the ring enabled, a frame that closes no window allocates nothing (`alloc_probe`).

**Reducer:**
- [ ] The reducer's unittest summarizes a checked-in two-run sample (one bake, then an engine run) into the expected per-load and per-window tables.

### Manual
- [ ] A stress-warren run through `postretro-tool run` with `POSTRETRO_TELEMETRY` and both timing variables set produces one file that holds the bake and the engine run. The reducer's tables match the stderr lines. Use Windows or Vulkan for GPU rows, because Metal reports GPU timing unsupported.
- [ ] An agent polling `events` on a windowed session across a level change sees unload, load phases, then streaming windows, with no unexplained `seq` gap at default caps.

## Path
- Precedents: `CollectingLogger` / `LogSink` in level-compiler for a collecting `log` backend with a bounded queue. The `cpu_timing` live-only section for the shape of an availability field. `serve_connection` and `read_request_frame` for where the transport peeks the verb before the single-slot queue.
- Emit points:
  - `StartupTimings::log_level_line` and the boot and mod `summary` sites.
  - `unload_level`.
  - `GeometryInstallMarks::log` on the upload thread.
  - `ShStreamingLogWindow::line_if_due` and `LightmapStreamingLogWindow`.
  - `FrameTimer::log_last_window`.
  - `FrameTiming::accumulate`.
  - The `Reporter` calls in level-compiler `pipeline.rs`.
- `StartupTimings` keeps its contained marks private. Expose a typed view; do not reparse `summary()`.
- Rival shapes:
  - A socket-only stream with no file. Rejected: the agent that launched the run is the common consumer, and bake events have no socket.
  - A file only, with the hello naming the file's path. Rejected for two reasons:
    - An active agent consumer reads the stream live.
    - The MCP frontend (roadmap Epic 20) exposes live verbs as tools, and a path in the hello gives it no tool.
- First slice: the leaf crate, `run_start`/`run_end`, line C as an event, and the file sink in the engine. This proves the snapshot-to-envelope path and append-mode framing before fanning out to the other emit points.
- `startup/mod.rs` and level-compiler `main.rs` are past 800 lines. Keep new logic in new modules and add only calls there.
- `ObserveResponse::Ok` carries `dump` today. Reshape the response vocabulary so `events` has its own body without breaking the dump path.

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
| Kinds | enum, snake_case | `run_start`, `run_end`, `log`, `boot_phases`, `mod_phases`, `load_phases`, `unload`, `geometry_install`, `sh_streaming_window`, `lightmap_streaming_window`, `cpu_timing_window`, `gpu_timing_window`, `bake_stage`, `bake_summary` |
| Producer state | enum | `"enabled"`, `"disabled"`, `"unsupported"` |
| Env var | — | `POSTRETRO_TELEMETRY` |
| Live verb | `ObserveRequest` variant | `{"verb":"events","since":u64,"max":u32,"kinds":[string]?,"wait_ms":u32?}` |
| Live reply | response body | `events`, `next`, `oldest`, `dropped`, `run` |
| Hello additions | `ServerHello` | `run`, `oldest`, `next`; `protocol` = 2 |

## Wire format
- **File:** UTF-8 JSONL, one envelope per line ending `\n`, written as one append per line and flushed.
- **Socket:** the existing observe-live framing (little-endian u32 length prefix, JSON body, 64 KiB request cap).
- **Fields:**
  - `seq` and `t_ms` are unsigned. `seq` starts at 0 at each `run_start`. Absent optional payload values serialize as `null`, never `0`.
  - An empty `events` reply is `[]`. `oldest` equals `next` when nothing is retained.
