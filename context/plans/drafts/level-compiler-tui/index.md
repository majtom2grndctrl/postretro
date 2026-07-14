# Level Compiler TUI

## Goal

Replace the offline level compiler's uncoordinated progress output with a coordinated
terminal UI. `prl-build` (crate `postretro-level-compiler`) gains a ratatui TUI showing the
per-map compile-step list with per-step progress and ETA, an end-of-run warning tally that never
gets clobbered, and live pause/resume + core-throttle controls that take effect mid-stage. Fall back
cleanly to plain output when not attached to a terminal. Compiled `.prl` output stays byte-identical
regardless of pause/throttle.

## Scope

### In scope

- Custom `log` backend that collects/tallies warnings and forwards them to the active reporter, so
  warnings no longer race the progress spinner on stderr.
- `Reporter` abstraction with a plain (non-TTY) impl and a ratatui TUI impl, replacing `BuildProgress`.
- End-of-run warning tally (total count plus the list of formatted warning records) in both reporters.
- `Governor`: one cooperative gate polled by parallel and serial bake work-items, unifying pause and
  core-throttle. Pause parks work-items mid-stage; throttle caps concurrent active work-items to N
  live permits without resizing the rayon pool.
- Per-step progress % and projected completion for the quantifiable stages (lightmap, SH, direct SH,
  delta SH, direct-SH-delta, animated weight maps).
- ratatui layout: left column of the map's planned compile steps with an active-step animation,
  per-step %/ETA at the column foot, bottom panel with a live core-count control and a pause/resume
  toggle, plus a warning/log region. Bake runs on a worker thread; main thread owns render + input.
- Behavior-preserving extraction of the linear `fn main` stage sequence into a stage-orchestration
  module.
- CLI: initial core count flag and TUI force/disable flag; TTY-gated reporter selection.
- Add `ratatui` + `crossterm` deps to the crate.

### Out of scope

- Any change to bake math, PRL sections, cache keys, or stage versions — output bytes are unchanged.
- Parallelizing the serial lightmap stage (it stays single-threaded; throttle documented as no-op there).
- Throttling non-work-item stages (parse, BSP, geometry, packing) — they run to completion untouched.
- A new PRL section or wire format. No persisted ETA-priors file; whole-build ETA is best-effort
  in-memory extrapolation only.
- GPU / renderer changes (this crate has no GPU dependency).
- Progress/ETA for the fast serial stages beyond a running/done indicator.

## Acceptance criteria

- [ ] During a bake, warnings appear in a dedicated region and do not corrupt or overwrite the live
      progress display (both TUI and plain modes).
- [ ] Every build ends with a summary reporting the total warning count and listing the warnings that
      fired, even ones that scrolled out of view during the bake.
- [ ] While a quantifiable stage runs (lightmap, SH, direct SH, delta SH, direct-SH-delta, animated
      weight maps), the stage's percent complete and a projected completion time are shown — in the
      TUI's per-step column foot, and in plain mode as a discrete printed line.
- [ ] The step column lists the steps planned for the map being compiled — content-dependent, e.g. no
      SDF row when the map has no SDF lights — with a visible in-progress animation on the active step,
      and steps that turn out to be no-ops render as skipped.
- [ ] A bottom panel exposes a live core-count control and a pause/resume toggle.
- [ ] Changing the core-count control during a parallel stage changes how many work-items run
      concurrently within that same stage, without restarting the stage.
- [ ] Pause halts forward progress mid-stage — including partway through the lightmap stage and the SH
      stage — and resume continues from the same point.
- [ ] Reducing the core count during the serial lightmap stage does not change its throughput
      (documented single-threaded), while pause still halts it mid-stage.
- [ ] For a fixed map and flags, the compiled `.prl` is byte-identical whether the build ran with
      pausing/throttling/TUI or ran straight through with none of them.
- [ ] With stdout/stderr not a TTY (CI, pipe, `xtask`, redirected stdin), the compiler runs without the
      TUI, prints plain per-stage progress — including percent and ETA as discrete lines, not a
      redrawing spinner — plus the warning tally, and emits no terminal control sequences.
- [ ] A CLI flag sets the initial core count and a CLI flag forces or disables the TUI; both appear in
      `--help`.
- [ ] With no `-j` flag, the default core count is fewer than all logical cores when more than one is
      available (leaves CPU headroom), so an unattended default build does not saturate every core —
      on a single-core machine the degenerate default is 1.
- [ ] On normal exit, error, bail, and panic, the terminal is left in cooked mode on the main screen
      with no residual control state.
- [ ] After the pipeline extraction, `fn main` no longer holds the inline stage sequence and a bake of
      the same map produces byte-identical `.prl` output and a Build Summary table with identical row
      set, order, labels, and format (per-stage elapsed timing values naturally vary) to before
      the refactor.

## Tasks

### Task 1: Extract the stage pipeline from `fn main`

`crates/level-compiler/src/main.rs` is ~2653 lines; `fn main` is one linear function that inlines 21
compile stages (parse, data-script, texture validation, BSP partitioning, visibility, geometry, BVH,
navmesh, lightmap, SH, delta SH, direct SH, entity-shadow selection, direct-SH-delta, shadowmask,
chunk-light-list, animated-light-chunks, animated weight maps, SDF atlas, texture mips, packing).
Behavior-preservingly extract this sequence into a new stage-orchestration module (e.g.
`src/pipeline.rs`) that owns the ordered stage execution, the `timings` vector, and the Build Summary
table, while `fn main` shrinks to arg parsing, cache construction, output-dir precheck, and a single
call into the orchestrator — invoked directly from `fn main` in plain mode; Task 4 moves this call onto
a spawned worker thread in TUI mode. Keep the current `BuildProgress` spinner and `env_logger` init exactly as
they are — this task adds NO new behavior, changes NO bake logic, and must produce byte-identical
`.prl` output and a Build Summary with identical row set, order, labels, and format (per-stage elapsed timings
naturally vary) for the same input. The extraction must expose a seam
where each stage has a stable identifier and a human label and where the orchestrator (not each inline
block) decides ordering, so later tasks can attach a reporter and gate. The `label` is the Build Summary
label (the `timings` string, e.g. `"SH Bake"`) that the byte-identity AC keys row identity to; the
existing spinner message (e.g. `"SH volume bake..."`) is a separate string and need not equal the label.
The orchestrator must also expose, computable before stage execution, an ordered predicted-present
descriptor list of `(stage_id, label, predicted_present)` via a named accessor (e.g.
`Pipeline::planned_stages() -> Vec<StageDescriptor>`) that Task 4 calls by name — SDF predicted via
`map_needs_sdf_atlas(&map_data.lights)`, which needs only the parsed lights — so Task 4 can render the
up-front step column. Preserve the exact conditional
structure: the SDF stage row is emitted only when `map_needs_sdf_atlas` holds, and stages that compute
`None`/placeholder output today keep doing so. Verify by compiling a map before and after and diffing
the output bytes and the summary row structure (labels, order, format — not the per-stage
elapsed timings).

### Task 2: Reporter abstraction, Governor gate, collecting logger

Introduce three foundations in new modules under `crates/level-compiler/src/`, then wire the Task 1
orchestrator to them, retiring `BuildProgress` (and removing the now-unused `indicatif` dep and import,
since `BuildProgress` is its only user). (1) A `Reporter` trait with methods the orchestrator
and stages call: begin a stage (by the stable id + label from Task 1), declare a stage's total work
units plus a shared progress-counter handle (the orchestrator allocates one `Arc<AtomicUsize>` per
quantifiable stage and clones it into both the stage `Ctx` and the reporter), mark a stage finished or
skipped, record a warning, and finalize the build with the timing summary (the orchestrator calls skip
when a stage's guard or `is_empty` check yields placeholder/`None` output, so AC4 renders it skipped).
For stages whose total is known up front the orchestrator declares it before the stage call; for delta SH
and direct-SH-delta the total (`affinity_lights.len()`) is only known mid-bake, after the affinity CSR is
built, so thread a settable total handle the bake publishes from inside (a second `Arc<AtomicUsize>` the
bake sets once, or a `set_total` call), and the reporter shows those two stages indeterminate until their
total is published.
Progress advance is via that shared `Arc<AtomicUsize>` — incremented inside the Task 3 work-item
closures, read by the reporter for %/ETA — not a per-item trait method; plus a `PlainReporter` impl reproducing today's non-TTY behavior (timestamped
stage lines or a simple progress line) plus per-stage percent and ETA emitted as discrete printed
lines (driven by the Task 3 counters, never a redrawing spinner, so CI/`xtask` logs stay clean), and
printing an end-of-run warning tally: the total warning
count plus the list of formatted warning records. (2) A `Governor`
struct: a cooperative gate holding a paused flag and a live permit count, exposing `checkpoint()` (fast
poll that parks the caller while paused, for serial loops) and an RAII `enter()` guard (parks while
paused, then acquires one of N permits, parking callers beyond N, releasing on drop, for parallel
work-items). Back it with a `Mutex` + `Condvar` (or equivalent std primitives — no `unsafe`); a
setter to change the permit count must wake parked threads, and clear-pause must wake all. Expose getters for the current permit count and paused flag
(`permits()`/`is_paused()`) for the TUI's live readout. `Governor::new` takes the starting permit count
(and an initially-unpaused flag) as parameters so Task 5 can seed it from `-j`. The rayon
global pool is left at all cores; the Governor caps concurrent active work-items, so excess parked
worker threads are the accepted oversubscription cost. (3) A `log::Log` backend installed via
`log::set_boxed_logger` that replaces the `env_logger::Builder...init()` call, preserves
`RUST_LOG`/verbose filtering, tallies `warn`-and-above counts, and forwards each formatted record to a
shared sink the active reporter drains (the existing `log::warn!`/`warn!` sites are NOT edited — the
installed logger captures them automatically). The orchestrator owns the `Reporter` and `Governor` (behind shared handles — `Arc` for the
Governor so worker threads and the input thread share it) and threads them toward the stages. Warnings
currently written with `eprintln!` in compiler code should route through `log` so the tally captures
them; converting those specific `eprintln!` sites to `log::warn!` is in scope only where the site is a
genuine warning the tally should include (not pure debug-diagnostic prints). This task settles the `Reporter` + `Governor` API that Tasks 3 and 4 depend on.

### Task 3: Instrument stages with checkpoints and progress counters

Thread the `Governor` and a per-stage progress counter (an `AtomicUsize` the reporter reads) into every
parallel bake and the serial lightmap, so pause and throttle take effect mid-stage and the reporter can
show %/ETA. The counter is incremented inside the work-item closures; it feeds display ONLY and must
not alter the order-preserving `.map()/.flat_map().collect()` results — the pre-BC6H byte-identity
determinism invariant (the Determinism invariant in build_pipeline.md) must hold, so do not introduce
float reductions, `HashMap`-ordered output, or reordering. Instrument these parallel work-item sites by
inserting a `governor.enter()` guard at the top of each closure and a counter increment per completed
item, and set the stage total before the loop: SH probe bake in `sh_bake.rs` — place the `governor.enter()` guard in the `into_par_iter` closure
inside `bake_sh_volume`, NOT in `fn bake_probe` (the warm path also calls `bake_probe`, so a guard
there would nest inside the group guard and deadlock at `-j 1`), total `layout.total_probes()`; its
warm grouped path in `sh_group.rs` gates only at `bake_or_load_group`, and since its work-item is a
whole group, pick one consistent pairing: either total `layout.total_probes()` and advance by that
group's `probe_indices.len()` per group, or total `groups.len()` and advance by 1 per group; direct SH
probe tiles in `direct_sh_bake.rs` (`bake_probe_tile`, total `layout.total_probes()`); the
direct-SH-delta and delta-SH CSR sub-block bakes in `direct_sh_bake.rs` and `delta_sh_bake.rs`
(per-`affinity_lights` entry); and the animated weight-map chunk bake in
`animated_light_weight_maps.rs` (`bake_one_chunk`, total `chunks.len()`). For the serial lightmap,
thread the gate into `bake_monolithic_atlas` (cold) and `bake_light_layer` (warm) in
`lightmap_bake.rs`/`lightmap_layer.rs` and call `governor.checkpoint()` inside the per-face `for` loop
so pause halts it mid-stage — but do NOT add `enter()` permits there, since
the stage is single-threaded and the throttle is a documented no-op for it. The cold single sweep in
`bake_monolithic_atlas` counts per face with total `placements.len()`; the warm path calls
`bake_light_layer` once per baked light-layer, each sweeping all `placements`, so a bare `placements.len()`
total overshoots across the multi-layer loop — for the warm path either count per face with total
`placements.len() × baked-layer-count` or advance once per completed layer with total = baked-layer-count. Pause only takes effect
within these instrumented stages (lightmap, SH, direct SH, delta SH, direct-SH-delta, animated weight
maps); during the fast non-instrumented stages (parse, BSP, geometry, packing) a pause request is a
deferred no-op until the next instrumented stage begins, consistent with those stages being out of
pause/throttle scope. The gate/counter reach
these functions by adding a field to each stage's existing `Ctx`/`Inputs` struct (`ShBakeCtx`,
`DirectBakeInputs`, `DeltaBakeInputs`, `WeightMapInputs`, `LightmapBakeCtx`, and the warm lightmap layer
call) or an added parameter, populated by the Task 1 orchestrator from the Task 2 handles; enumerate
and update every call site in the orchestrator. Note `ShBakeCtx` is constructed once and shared by the
cold and warm SH bakes AND borrowed by both direct stages via `DirectBakeInputs`, so the counter on
`ShBakeCtx` is the indirect-SH counter ONLY; direct SH and direct-SH-delta each allocate their own
counter (and total handle) on their freshly-built `DirectBakeInputs`, and their closures must never
increment `sh_ctx`'s counter. A gate with all cores permitted and never paused must
reproduce today's behavior and timing within noise. `governor.enter()` is acquired only at the
outermost work-item boundary; a gated closure must not block on other gated work, which would deadlock
at low permit counts (e.g. `-j 1`).

### Task 4: ratatui TUI reporter and render loop

Add `ratatui` + `crossterm` to `crates/level-compiler/Cargo.toml` and implement a `TuiReporter` that
satisfies the Task 2 `Reporter` trait. Layout: a left column listing the map's planned compile steps
(the ordered predicted-present descriptor list the Task 1 orchestrator exposes before execution — omit stages known absent up
front such as SDF when the map has no SDF lights, and render stages that resolve to no-ops as skipped)
with an in-progress animation on the active step; the active step's percent complete and a per-stage
completion ETA shown at the foot of that column, computed from the Task 3 progress counter and the
stage total (reliable — derived directly from a known total and counter — except delta SH and
direct-SH-delta, whose total is published mid-bake per Task 2, so their foot shows an indeterminate
indicator until then); a whole-build ETA, if also
shown, is a separately-labeled best-effort readout (elapsed/done extrapolation only), not the
column-foot value;
a bottom panel with a live core-count control (slider/±, range `1..=available_parallelism()` via
`std::thread::available_parallelism()` — permits above the core count are inert) bound to the
`Governor` permit count and a pause/resume toggle bound to the `Governor` paused flag; and a region
that renders warnings/log records drained from the Task 2 logger sink without disturbing the step
column. Thread model: in TUI mode the bake (the Task 1 orchestrator call) runs on a spawned worker
thread while the main thread owns the ratatui render + input loop at a fixed tick — reading key events
(quit, pause toggle, core up/down) via crossterm and mutating the shared `Arc<Governor>` (increasing
permits and clearing pause wake parked workers through the Task 2 `set_permits`/`set_paused` setters) — with reporter and
progress-counter state `Arc`-shared between the render/main thread and the bake worker thread;
rendering never blocks the bake. In plain (non-TTY) mode the bake runs directly on the main thread as
today, with no render loop. The TUI must restore the terminal (leave raw mode / alternate screen) on
normal exit, on error, on a build that bails, and on panic — via an RAII guard whose `Drop` restores
the terminal on unwind (optionally paired with a panic hook that restores then re-raises), so a panic
mid-bake cannot leave the terminal in raw mode / alternate screen. On finalize, after leaving the
alternate screen / raw mode, the `TuiReporter` drains the logger's warn-and-above records and prints
the total warning count plus that warn+ listing to the normal screen (the live log region during the
bake showed all levels; the end-of-run listing is the warn+ subset only), so scrolled-out warnings
still appear (satisfying AC 2 in TUI mode). Task 4 exposes a named entry point (e.g. `run_tui(...)`)
that spawns the bake worker thread and owns the render loop end-to-end; Task 5's gated reporter
selection only calls it, so Task 4 lands and compiles without depending on Task 5's gating logic. This
task consumes the `Reporter` trait and `Governor` from Task 2 and displays counters populated by Task
3, but edits disjoint files (tui module, Cargo.toml) from Task 3.

### Task 5: CLI flags, TTY gating, integration verification

Wire reporter selection and controls end-to-end. Add two CLI flags to `parse_args_from` / the `Args`
struct and `help_text()` in `main.rs`: one setting the initial core count (e.g. `-j`/`--jobs <N>`,
default leaves CPU headroom: `max(1, available_parallelism - 1)`, or `- 2` when
`available_parallelism > 8`, validated `>= 1`) which seeds the `Governor` permit count, and one
forcing or disabling the TUI (e.g. `--tui`/`--no-tui`). Select the reporter at startup: use the
`TuiReporter` only when TUI is not disabled AND stdout, stderr, and stdin are all TTYs
(`std::io::stdout().is_terminal() && std::io::stderr().is_terminal() && std::io::stdin().is_terminal()`);
otherwise use the `PlainReporter` (CI, pipes, `xtask`, redirected streams). `--tui` overrides only the
default reporter-selection heuristic, never the TTY requirement: on a non-TTY, `--tui` errors out with
a clear message rather than emitting terminal control sequences (so AC 10 always holds); `--no-tui`
forces plain regardless of TTY. In plain/non-TTY mode the core count comes
only from `-j` (no live slider) and pause is unavailable, but the Governor still applies the `-j`
permit cap; emit no terminal control sequences. Confirm the whole feature on a real bake: run
`cargo run -p postretro-level-compiler -- <map>` interactively to exercise the TUI, pause/resume, and
the live core slider mid-bake (including mid-lightmap pause); run it piped/non-TTY to confirm plain
fallback and the warning tally; and, holding cache mode constant, diff `.prl` output of a
paused/throttled `--no-cache` build against a straight (unthrottled, no-pause) `--no-cache` build of
the same map to confirm byte-identity — never compare a warm-TUI build against a cold-`--no-cache`
build, since that conflates warm-vs-cold caching with pause-vs-throttle. The scripted diff proves throttle-invariance
(`-j 1` vs `-j N`, both `--no-cache`, plain mode); pause-invariance is by-design (counters are
display-only, no reordering) and spot-checked in the interactive run. TUI-mode byte-identity is
likewise by-design — the reporter never touches bake output — and is not separately scripted (TUI
needs a TTY and can't be piped through the diff); AC 9's "...or TUI" clause is satisfied by this
by-design guarantee, not by an un-runnable scripted test.

## Sequencing

**Phase 1 (sequential):** Task 1 — pipeline extraction. Creates the stage seam every later task drives;
must land first because it restructures the file all others edit.

**Phase 2 (sequential):** Task 2 — Reporter + Governor + collecting logger. Depends on Task 1's seam
and settles the API that Tasks 3 and 4 both consume.

**Phase 3 (concurrent):** Task 3 (stage instrumentation) and Task 4 (ratatui TUI). Both consume the
Task 2 `Reporter`/`Governor` contract but edit disjoint files — Task 3 edits the bake modules, Task 4
edits the TUI module and `Cargo.toml`. They meet only at the shared trait/gate.

**Phase 4 (sequential):** Task 5 — CLI flags, TTY gating, and real-bake verification. Consumes the
reporters from Tasks 2/4, the gate wiring from Task 3, and cannot verify byte-identity until both land.

## Rough sketch

- **Modules:** `pipeline.rs` (Task 1 orchestrator), `reporter.rs` (`Reporter` trait, `PlainReporter`,
  `TuiReporter`), `governor.rs` (`Governor`, `Permit` guard), `logger.rs` (`CollectingLogger:
  log::Log`). Keep `BuildProgress` deleted once `PlainReporter` covers its behavior.
- **Governor:** `struct Governor { inner: Mutex<GateState>, cvar: Condvar }` with
  `GateState { paused: bool, permits: usize, active: usize }`. `checkpoint()` waits while `paused`.
  `enter()` waits while `paused || active >= permits`, then `active += 1`; drop decrements and notifies.
  `set_permits`/`set_paused` mutate state and `notify_all`. Shared as `Arc<Governor>`. Reducing `permits`
  below the current `active` count does not preempt in-flight work-items — they keep their permits and
  concurrency converges downward as items complete; new admissions are throttled immediately. The
  core-count readout shows the target permit count (the set value), not the transient `active` count.
- **Progress:** per-stage `Arc<AtomicUsize>` + a known total; reporter computes `%` and
  `ETA = elapsed * (total - done) / done`. Counters are display-only — never fed back into bake output.
- **Stage ids:** an enum or `&'static str` id + label per stage, produced by the orchestrator; the TUI
  left column is the predicted-present subset for the map (drop SDF when `!map_needs_sdf_atlas`, mark
  runtime no-ops skipped).
- **Logger sink:** `CollectingLogger` holds `Arc<Mutex<Vec<CapturedRecord>>>` + an `AtomicUsize` warn
  counter, where `CapturedRecord` is an owned type (`level`, `target`, `message: String`) — `log::Record<'a>`
  borrows and cannot be collected across threads. Reporters drain/format it. Startup creates the shared sink first, then clones it into both the
  installed `log` logger and the selected reporter. Installed with `log::set_boxed_logger`, wrapping an
  env-filter so `RUST_LOG`/verbose still work.
- **TTY gate:** `std::io::stdout().is_terminal() && std::io::stderr().is_terminal() &&
  std::io::stdin().is_terminal()`, mirroring the EOF-tolerant pattern already in
  `precheck_output_dir`.
