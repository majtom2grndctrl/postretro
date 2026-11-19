# E20--streaming-telemetry — plan of record

mode: compact
status: active
read at: e0ac8b7f

## Corrections
- Decision "a new leaf crate … on std, serde and serde_json only" → the Path puts the tee logger in that crate, and a `log::Log` impl needs the `log` facade; planning around it by adding `log` (the facade every crate already links) as the crate's fourth dependency. No engine crate enters it, so the Decision's meaning holds.
- Brief "the renderer hands back `GeometryInstallMarks` as plain data" → the marks are built inside `build_level_set_core` on the upload thread and never leave the set; planning around it by freezing them into a public plain `GeometryInstallTiming` stored on the resident set at the line, read back by set id at the bundle take or the beside commit.
- Brief "`FrameTiming::accumulate`" as an emit point → it runs inside the renderer, which has no serde; planning around it by keeping each closed GPU window for a take-once renderer accessor that the binary emits from on the same frame, so the window carries the level current at its close.
- Source: `rustc` 1.97 in the container is below the workspace `rust-version` 1.99 → updated the stable toolchain to 1.100 before building. Environment only.
- Boundary inventory "Live verb | `ObserveRequest` variant" and Path "reshape the response vocabulary" → the transport peeks the verb and answers `events` from `observe_live/events.rs` with its own request type and reply body; `ObserveRequest`/`ObserveResponse` stay the main thread's vocabulary and their docs say so. Same wire shape; the request ignores unknown fields so a later wait field needs no protocol bump.
- Wire format "absent optional payload values serialize as `null`" → `cpu_timing_window` reuses `CpuWindowReport` as the Decision requires, and that type omits absent keys rather than writing `null`. Absent still never reads as zero; the reducer reads both forms.
- Brief silent on platform suspend → a suspend stops the running level without an unload: its streaming sessions close their partial windows naming it, then the hub's level clears so later events name none until the resumed load commits. Whether suspend should also write an `unload` is an owner call.
- Brief silent on capture runs' timing → capture writes its timing to its measurement report, not timing windows, so a capture run lists requested timing producers unsupported with that reason, as headless does.

## Delegated answers
- Ring caps — events 4096 records / 4 MiB, log records 2048 / 1 MiB, log message cap 4 KiB. Reason: at the File sink's rates (one line per load, per 5 s streaming window, per 120-frame timing window, per actionable message) a stress-warren session closes about one SH, one lightmap and two timing windows every 5 s; 4096 events hold over an hour of windows, and each window line is under 2 KiB. The stress-warren measurement the brief asks for needs a windowed GPU session this container cannot run; it stays an owner manual row below.
- Run id — `<epoch ms hex>-<pid hex>`. Reason: two processes alive at once have distinct pids, and no random-number dependency enters the leaf crate.
- Preflight and `tools/` tests — preflight runs no Python today; the reducer's unittest joins it as its own check (`python3 -m unittest tools.tests.test_telemetry_reduce`). Reason: stdlib only, so it needs no environment and stays in the gate.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| Snapshot → payload and line agree (line C sums and marks, miss bucket, CPU avg/max, GPU mean, absent GPU pass) | unit tests beside each emit point (`*_event_matches_line`) | achievable as stated |
| stderr unchanged with sink on/off; fixed snapshot byte-identical; `prl-build --no-tui` stderr and Build Summary stdout; existing log-capture and CLI-contract tests unchanged | engine line-builder tests on/off; `compiler_cli_contract` + new `telemetry_cli` integration test diffing on/off output | achievable as stated |
| No-feature engine and `prl-build` write the sink | `cargo test -p postretro --no-default-features` sink test; `telemetry_cli` | achievable as stated |
| P3 loading-screen level change order | `lifecycle_load` telemetry test with scoped hub | achievable as stated |
| P4 beside load phases and commit-frame group | `lifecycle_beside` telemetry test | achievable as stated |
| Beside commit: partials and unload between geometry install and phases | `lifecycle_beside` telemetry test | achievable as stated |
| P13 cancelled after set build emits nothing | `lifecycle_load` telemetry test | achievable as stated |
| Unloaded before reveal: geometry install + unload, no phases; reducer lists it | lifecycle test + reducer unittest | achievable as stated |
| P5 concurrent emitters strictly increasing, poller misses none | leaf crate concurrency test | achievable as stated |
| P1 GPU pending → unsupported before any window | renderer support state + app settle test | achievable as stated |
| Idle SH window emits nothing | `sh_streaming_diagnostics` test | achievable as stated |
| Unload 2 s into counted SH window: one partial with elapsed, no line | `sh_streaming_diagnostics` test | achievable as stated |
| Replacement within a level: one partial, totals restart (P9); idle nothing | `sh_streaming_diagnostics` test | achievable as stated |
| Lightmap partial at session end; world-less clear ends both | `lightmap_streaming_diagnostics` + session test | achievable as stated |
| CPU window cleared at level change emits nothing; GPU window names level at close | `cpu_timing` test; GPU emit test | achievable as stated |
| Bake events list Build Summary stages in order; durations equal in plain and TUI (P7) | level-compiler reporter decorator tests | achievable as stated |
| Stage with no output emits skip | decorator test | achievable as stated |
| Failing bake: begun stages, unsuccessful summary without durations | decorator/frontend test | achievable as stated |
| Bake ending before first stage: `run_start` + `run_end` only | `telemetry_cli` | achievable as stated |
| `--help` and rejected argument write nothing (P14) | `telemetry_cli` | achievable as stated |
| `run_start` first at seq 0 before logs; relative path resolved | leaf test + `telemetry_cli` + engine binary test | achievable as stated |
| `run_start` producers: CPU enabled/disabled with variable; GPU disabled/pending; settle to enabled | engine producer tests | achievable as stated (enabled settle needs a timestamp adapter: GPU-gated) |
| Headless with both variables: unsupported, no windows, nothing pending at `run_end` | headless binary test (`observability`) | achievable as stated |
| Clean exit: partials then `run_end` last; post-`run_end` record writes nothing (P6) | leaf test + engine exit test | achievable as stated |
| Error, panic, capture/headless exits write `run_end` with reason | leaf guard test + `telemetry_cli` + headless test | achievable as stated |
| Caught worker panic writes no `run_end`; process-ending panic does, engine and both `prl-build` reporters (P6) | leaf guard test; `telemetry_cli` panic hook seam | achievable as stated |
| Killed process leaves every prior line parseable | leaf test (child process killed) | achievable as stated |
| Unwritable path warns once, run completes | leaf test | achievable as stated |
| Mid-run write failure warns once, no deadlock, ring and stderr continue (P10) | leaf test | achievable as stated |
| Engine and bake appending at once: two runs, whole lines incl. message at cap (P11) | leaf multi-process test | achievable as stated |
| Bake and engine from different cwds name same absolute `.prl`; reducer joins | `telemetry_cli` + reducer unittest | achievable as stated |
| Ring eviction per class and caps | leaf ring tests | achievable as stated |
| Late attach after eviction (P2) | leaf ring test | achievable as stated |
| Paging from 0 with small `max` reads every event once (P2) | leaf ring test | achievable as stated |
| Over-cap message truncates and is flagged | leaf test | achievable as stated |
| `since` older than oldest | leaf ring test | achievable as stated |
| `since` = `head` empty | leaf ring test | achievable as stated |
| `since` > `head` error names run and head | leaf + observe-live test | achievable as stated |
| `max` 0 (P8) | leaf ring test | achievable as stated |
| `kinds` filter and unknown kind | leaf + observe-live test | achievable as stated |
| Two polls disjoint and contiguous | leaf ring test | achievable as stated |
| Main thread blocked: `events` answers at once, before and after a timed-out `dump` (P12) | observe-live transport test | achievable as stated |
| Protocol-1 client reads hello; malformed `events` body errors | observe-live test | achievable as stated |
| Consumer present: info captured while stderr filter hides it; debug not captured | leaf tee test | achievable as stated |
| No consumer: max level unchanged (own process) | leaf integration test binary | achievable as stated |
| `prl-build`: info record does not reach TUI warnings list | level-compiler tee test | achievable as stated |
| `alloc_probe`: streaming windows and CPU timer per-frame paths allocate nothing with sink/ring | engine alloc test | achievable as stated |
| Reducer unittest over checked-in two-run sample | `tools/tests/test_telemetry_reduce.py` | achievable as stated |
| Every sample line parses as a current envelope | leaf crate test | achievable as stated |
| Manual: stress-warren bake + run into one file, reducer matches stderr | owner, in-engine (Windows/Vulkan for GPU rows) | manual |
| Manual: agent polls `events` across a level change | owner, windowed session | manual |

- Brief "a bake summary … from each frontend's `Err`" → `prl-build`'s `main` now wraps the old body as `bake_main` under the unwind guard, and writes the failed summary from the body's `Err` once any stage began; planning around `main.rs` size by keeping the logic in a new `telemetry.rs`.
- Brief "partial at every session-ending path" → each streaming window closes its partial when its session drops (`ShStreamingSession`'s existing `Drop`, the lightmap window's own), which covers unload, replacement, decline, world-less clear and the App's drop before `run_end` without naming each path.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Leaf crate `postretro-telemetry`: envelope, hub (one lock: seq, ring, file), ring with pins and caps, `events` page, tee logger, scoped test hub; engine first slice: `run_start`/`run_end`, line C event, file sink | integrating executor | — | done: `cargo test -p postretro-telemetry` 28 passed |
| 2 | Engine load family: boot/mod/load phases, unload, geometry install hand-back, running level at commit; ordering tests P3, P4, P13 | integrating executor | 1 | done; P3/P4/P13/before-reveal tests are GPU-gated and skip here (no adapter; lavapipe's 128 MiB binding limit is below the renderer's floor) |
| 3 | Engine streaming windows and partials; CPU/GPU timing windows; GPU `producer_state` | integrating executor | 1 | done: `cargo test -p postretro --bin postretro -- telemetry streaming_diagnostics cpu_timing::tests timing_events unload_writes` 54 passed |
| 4 | Engine exits: partials then `run_end` on clean/error, capture/headless `process::exit`, panic guard | integrating executor | 2, 3 | done: partials close as each streaming session drops (the App drops before `run_end`); `tests/telemetry_run.rs` 3 passed with `observability` |
| 5 | `prl-build`: sink after argument parsing, tee over `CollectingLogger`, reporter decorator, bake summary from each frontend, exits | integrating executor | 1 | done: `--bin prl-build -- telemetry behind_the_tee` 6 passed; `--test telemetry_cli` 4 passed |
| 6 | observe-live `events` verb on the transport thread; hello protocol 2 with run | integrating executor | 1 | done: `-- observe_live` 34 passed |
| 7 | Reducer in `tools/`, checked-in two-run sample, unittest, preflight wiring | delegated worker; integrated by executor | 1 | done: `python3 -m unittest tools.tests.test_telemetry_reduce` 20 passed; sample parses (`--test sample`) |
| 8 | Allocation probe, preflight, review loop, context updates | integrating executor | 2–7 | review round 1 fixed (see Review); `/preflight` is user-invoked only |

## Review

Round 1: 6 depth lenses (opus) + 2 hygiene passes (sonnet), no 🔴. Fixed:
- `prl-build`: one summary per run (a TUI terminal error after success no longer adds a failed one); the TUI's Parsing begins before `prepare`, so a TUI parse failure writes its stage and failed summary like the plain reporter; a background stage's finish keeps its flag.
- Leaf: the tee's capture flag clears on unwind; `oldest` rises past evictions in either class; in-crate tests compile with `cargo test -p postretro-telemetry` alone; `end_level` emits the unload and clears the level under one lock; the sink warning is formatted outside the lock; `Kind::ALL` drift guard; `ScopedHub` is `!Send`.
- Engine: `exiting()` closes streaming windows and ends the run (macOS app-menu Quit never returns from the loop); suspend clears the running level; capture lists timing producers unsupported; settled producers keep their variable; SH latency gauges are null before a session's first read; capture and headless error exits carry the error text.
- Reducer: a payload missing what its kind needs is counted malformed, never fatal; the sample matches real producer output (boot and mod phase names, commit-frame marks summing, the `Lightmap bound` row) and its leaf-owned payloads parse as the leaf's types.
- Comment drift across renderer, engine and leaf; clippy `type_complexity`; temp-dir cleanup in CPU tests; bake tests moved to a lit fixture that runs every stage and a background one.

Round 2 (one correctness tracer on the fix commit), fixed:
- `exiting()` writes `run_end` only on Apple platforms, where the loop never returns; elsewhere main's `run_end` still records a loop error or a teardown panic. Streaming windows close in `exiting()` everywhere.
- Suspend closes the level's streaming sessions before clearing the running level, so their partials name it.
- The TUI's early Parsing begin is written only when `prepare` fails, so a config cancel or a later TUI error carries no dangling stage.
- `end_level` reads, writes and clears under one lock.
- The reducer validates each payload before applying any of it (no half-applied loads, windows or bakes) and rejects a bool generation.
- The sample's leaf-owned payloads now really parse as the leaf's types (the earlier edit had not landed); the crate graph snapshot names the new leaf.

Left, with reasons:
- Invalid-JSON bodies naming `events` reach the main thread's queue: the peek needs a parseable body, and the main thread still answers an error.
- One entry larger than a class's byte cap empties that class first: unreachable at the default caps.
- A pipe or FIFO sink can tear lines over `PIPE_BUF` and block on a full pipe: the sink is a file by contract.
- Duplicated scoped-hub test helpers across modules: test-only.
- `observability` feature test code in `observability/driver.rs` fails to compile on `main` (missing `PlayerMovementDescriptor` fields): pre-existing, untouched by this branch.
