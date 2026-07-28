# Test Log Capture

## Goal

One log-capture harness for the workspace, so tests can assert on `log` records — level,
target, message body, in emission order.

Four ad-hoc capture loggers exist today, each calling `log::set_logger`. That call is
process-global and settable once, so two of them in one test binary is a correctness bug,
not duplication. `crates/ui` has two, and the loser skips silently on every run.

E15 (`context/plans/drafts/E15--session-lifecycle`) has log-keyed criteria that survived
several review rounds. This plan is its prerequisite and merges to `main` first; E15 is
code-anchored against the merged crate afterward. AC-SURFACE-1 pins the surface that
re-anchor pass will cite by name — E15 names nothing from this crate yet, and a rename
once it does costs a second re-anchor.

## Scope

### In scope

- A workspace crate, dev-dependency only: one process-global `log::Log` routing records
  into per-thread buffers, behind an RAII guard.
- Positive, exactly-once, and negative assertions on level plus body substring, target
  prefix as an added filter, plus the ordered record list.
- Panic — never a silent skip — when the harness is not the active logger.
- Retiring all four ad-hoc loggers.
- Log assertions on three shipped degradation paths in `crates/net`,
  `crates/scripting-core`, and `crates/postretro`.

### Out of scope

- **Records emitted on threads with no guard.** `run_worker`, called from the
  `thread::spawn` closure in `spawn_level_worker` (`crates/postretro/src/startup/worker.rs`),
  logs `[Loader]` warns off-thread. Counted, not captured.
- **Migrating off the `log` facade.** The engine stays on `log` + `env_logger`.
- **Reusing `CapturedRecord` from `crates/level-compiler`.** Its `logger` module is
  declared in `main.rs`, not `lib.rs` — not library surface. Layering blocks it too:
  `crate-graph.md` places `net` below `level-compiler`.
- **`RUST_LOG` / `env_filter` directives.** Capture everything, filter at assertion time.
- **E15's negative-existence criteria** — "no digest moves", "demotes nobody", "no table
  retains an entry". Value assertions on state; capture adds nothing.
- **E15's AC-DIGEST-8** (a field addition must fail to compile). Review gate on the
  exhaustive destructure. No `trybuild`.

## Direction

**Problem.** Four private `log::Log` impls across three crates, each with its own store —
counters in `crates/ui`, record buffers in `crates/postretro` and `crates/renderer` — and
three of the four probing to see whether they lost the `set_logger` race, then skipping if
so. Two share the `postretro-ui` lib test binary, so one always loses. `local_bind_with_no_enclosing_scope_degrades_to_literal_and_warns_at_build`
(`crates/ui/src/tree/tests/local_state.rs`) is the loser on every run: both its warn
assertions are dead and it reports `ok` on its fallback-text assertion alone. The three
`theme_gate_test.rs` cases win the race today, so their skip arms are latent — adding any
third logger to that binary, including this harness, kills them too.

**Prior commitments.**
- `development_guide.md` §6.1 requires a `[Subsystem]` tag in the **message body**
  (`[Net]`, `[Loader]`, `[UI]`). Body substring is the primary assertion key; target is a
  secondary filter.
- `crates/level-compiler/src/logger.rs` defines `CapturedRecord { level, target, message }`.
  The new crate mirrors that field set. It cannot share the code — see Scope.
- `crate_graph.rs` builds its graph from normal edges only (`collect_graph`), so a
  dev-only member adds a node with no edges. `layering_invariants_hold` is unaffected;
  `layers` ranks every member, so the committed `crate-graph.md` goes stale and
  `check_committed_doc` fails until regenerated.

**Divergences.**
- `context/plans/done/M13--fonts-theming/index.md` shipped the `theme_gate_test` cases and
  recorded the gap as practice, not limitation: the warn-once behavior is "a
  code-review/manual gate — the harness has no log capture, matching existing
  fallback-path practice." `context/plans/done/E18--trap-pools-seeded-arming/index.md`
  says the same. This overturns that practice. The practice was priced against a
  per-crate logger; four of those now exist and one is silently dead.
- `testing_guide.md` §3 lists no log-assertion pattern. This adds one. §1 names
  degradation paths a priority target, and Task 4's three produce no other observable
  that discriminates the failure.
- `testing_guide.md` §2 excludes "Crate internals." AC-CAP-1 through AC-CAP-9 do not
  qualify: each asserts on code this plan writes, not on whether the `log` facade
  dispatches. The rule throughout — keep a test that exercises our logger; replace one
  that only exercises someone else's.

**Alternatives rejected.**
- **Diagnostic-as-value at the emission site** — return a typed diagnostic, log at the
  boundary, assert the value. `development_guide.md` §6.1 points this way ("prefer
  structured errors for failures that could surface to the user or to diagnostic
  tooling"), and the repo does it in `StagedManifestDiagnostic` and E15's
  `DivergenceReason`. E15 already drew this line: its gating diagnostics are typed values
  and the log-keyed criteria are the residue. This plan takes that split as given. What
  the rival cannot reach: values E15 decided must not gate, whose only permitted effect is
  the log line; properties of the logging itself, like warn-once-per-build; and which of
  three sibling paths refused a write when all three return `Ok(())`. It also would not
  have touched the four-logger collision.
- **`testing_logger`** (crates.io, latest 0.1.1, published 2018-08-07) — same thread-local
  shape. No exactly-once helper, and no signal when it loses the `set_logger` race, which
  is the defect being removed.
- **Per-crate loggers** — the shipped state. Rejected: it is the bug.
- **`--test-threads=1` or `#[serial]`** — makes correctness a runner flag.

**Placement.** Workspace crate, dev-dependency only. The axis is workspace crate vs.
per-crate module, not engine vs. mod. Five crates need it, and the per-crate alternative
is what is being retired.

**Foreclosures.**
- One test-only logger per binary, workspace-wide. Intended, and loud when violated.
- **Asserted log bodies become API.** A test keying on `[Net] rejecting client` breaks when
  that line is reworded. This opposes the `compiler-log-hygiene` draft, which treats log
  records as freely mutable. No file overlap — that draft is entirely under
  `crates/level-compiler` — and the boundary that keeps it that way: a log line an
  acceptance criterion names is contract; every other log line stays noise.
- **Cross-thread capture** is not built. The per-thread slot holds `Arc<Mutex<Vec<_>>>` so
  a `Send` handle is additive, not a rewrite.
- **Orphaned records are counted, not lost.** `set_max_level(Trace)` is process-global, so
  this logger becomes the sink for every call site in the binary. Silently dropping
  unbuffered records would foreclose a future chaining sink. The counter is unconditional
  and feeds every assertion failure (AC-CAP-8); a full stderr echo is available behind
  `POSTRETRO_LOG_CAPTURE_ORPHANS`, opt-in because `crates/ui` alone runs 237 tests with
  several deliberately emitting `[UI]` warns. When the echo is on, same-thread orphans are
  captured by libtest and attributed to whichever test owns that thread, and cross-thread
  orphans bypass capture and always print.

Reversal costs one crate deletion, five manifest lines, and restoring four loggers from
git history. Not a one-way door. What decays is the assertion style: cheap now, expensive
once E15's criteria are written against it.

## Acceptance criteria

- [ ] **AC-CAP-1** — Assertions match on level **and** body substring together: the same
      substring at a different level does not satisfy one, nor a different substring at
      the same level. Target prefix filters further, so an assertion scoped to one crate's
      module path is not satisfied by an identical body from another's.
- [ ] **AC-CAP-2** — The record list is retained in emission order; each record's level,
      target, and body are readable.
- [ ] **AC-CAP-3** — Exactly-once passes against one match and fails against two; negative
      passes against zero and fails against one. Failure is a panic carrying the message
      AC-CAP-8 describes, so the failing direction is exercised through `catch_unwind` with
      the payload downcast and inspected — not `#[should_panic]` alone. The deliberate
      panics run under a suppressed panic hook, restored after, so a passing suite does not
      print backtraces. The hook is process-global, so the in-crate tests that touch it
      serialize among themselves — permitted, since AC-CAP-5's no-mutex gate covers the
      converted consumer tests, not the harness's own.
- [ ] **AC-CAP-4** — `trace` and `debug` records are captured, not only `warn` and above.
- [ ] **AC-CAP-5** — **Parallel isolation.** Two threads, each holding a guard, each
      emitting distinct records, see only their own. Runnable test. The companion claim —
      no suite-serializing mutex in any converted test — is a grep gate, since the buffer
      itself is a `Mutex`.
- [ ] **AC-CAP-6** — **Panic hygiene.** After an unwind through a held guard, the next
      guard on that thread constructs **without panicking** and observes zero records. The
      non-panic half is the load-bearing one: a fresh buffer makes "zero records" true
      even if `Drop` did nothing.
- [ ] **AC-CAP-7** — **Loud on collision.** With a foreign logger already installed,
      constructing a guard panics naming the conflict. That half needs a dedicated
      integration target, since the condition can only be produced once per process. The
      second half — a second guard on a thread already holding one also panics — is a unit
      test.
- [ ] **AC-CAP-8** — **Legible failures.** A failed assertion names the expectation, the
      match count, and every captured record with level, target, and body. It also reports
      records logged on bufferless threads since the guard was constructed, on a line
      beginning `orphan records (process-wide):` — the literal label is the contract the
      test matches on. Verified on presence and format, not on an exact count: the counter
      is global and concurrent tests move it.
- [ ] **AC-CAP-9** — Clearing empties the calling thread's buffer only and leaves other
      threads' buffers untouched.
- [ ] **AC-LOGGER-1** — **Grep gate**, not a runnable test. No test-only logger install
      remains outside `crates/test-log-capture/`. Grep
      `set_logger|set_boxed_logger|env_logger::` with that directory excluded — it must
      match exactly the three production installers: `env_logger` in `postretro`'s
      `main.rs` and `gen_script_types.rs`, and `set_boxed_logger` in `level-compiler`'s
      `logger::install`. The exclusion matters: the capture crate and AC-CAP-7's
      integration target both install loggers by design. Grepping `set_logger` alone
      misses the boxed variant; today's unfiltered baseline is seven hits.
- [ ] **AC-UI-1** — Every `crates/ui` warning-count assertion runs unconditionally: no skip
      arm, no `Option` return, no test-serializing mutex anywhere in the crate. The orphan
      local-bind case asserts both halves it currently skips — one warn at build, none on
      the retained draw path.
- [ ] **AC-ADAPT-1** — Compile gate, with one deliberate exception. Every existing call
      site of the `crates/postretro` and `crates/renderer` capture helpers compiles
      unchanged. Each helper keeps its own parameter shape — they differ (`postretro` takes
      a generic `F`, `renderer` an `impl FnOnce()`) and neither moves. The exception: the
      renderer helper's return type drops from `Option<Vec<_>>` to `Vec<_>`, and its single
      caller is updated with it.
- [ ] **AC-REACH-1** — Three shipped degradation paths gain a log assertion. **Netcode:**
      the accept path logs its accept and no reject, the divergent path the reverse —
      extending tests that today assert only the `HandshakeOutcome`. The negative half must
      name a reject substring specific enough not to be satisfied by the adjacent
      handshake-decode-failure warning. **Scripting:** each of the three read-only slot
      write paths logs a refusal naming the slot while returning success; the log is what
      says which path refused. These three tests are new — no existing test covers the
      refusal. **Engine:** the unregistered-class and meshless-descriptor branches return
      the same value and are told apart by level and body alone.
- [ ] **AC-BUILD-1** — A non-test build links none of this. Mechanical half: the crate
      appears in no workspace member's normal-edge dependencies — visible in the
      regenerated `crate-graph.md`, where a normal edge would move it off `Layer 0` or into
      the dependents ranking. Review half: the `env_logger` runtime path is unchanged.
- [ ] **AC-BUILD-2** — `crate-graph --check` passes with the new member, and
      `layering_invariants_hold` still passes.
- [ ] **AC-SURFACE-1** — **The pinned surface.** E15's post-merge re-anchor pass will cite
      these names, so a rename, reordered argument, or changed argument type is a contract
      change from that point. Bodies and internals are free.

      ```rust
      // crates/test-log-capture/src/lib.rs
      pub struct CapturedRecord { pub level: log::Level, pub target: String, pub message: String }

      pub struct LogCapture { /* private */ }

      impl LogCapture {
          pub fn start() -> Self;                                            // install + attach fresh buffer
          pub fn records(&self) -> Vec<CapturedRecord>;                      // ordered
          pub fn clear(&self);
          pub fn assert_logged(&self, level: log::Level, body: &str);        // >= 1
          pub fn assert_logged_once(&self, level: log::Level, body: &str);   // == 1
          pub fn assert_not_logged(&self, level: log::Level, body: &str);    // == 0
          pub fn assert_logged_from(&self, level: log::Level, target_prefix: &str, body: &str);
      }
      impl Drop for LogCapture { /* detach */ }
      ```

      `body` is last on every assertion, so the target-scoped variant reads as the plain
      one with a filter inserted. `level` is `log::Level` — exact, never a threshold, so a
      `Warn` assertion is not satisfied by an `Error`. Target matching is a prefix test on
      `record.target()`, which defaults to the emitting module path: `"postretro_net"`
      scopes to the crate, `"postretro_net::transport"` to one module. `LogCapture` is
      `!Send` (it detaches the *thread's* slot) and both `UnwindSafe` and `RefUnwindSafe`
      — AC-CAP-3 and AC-CAP-6 pass `&LogCapture` into `catch_unwind`, which needs the
      latter. `PhantomData<*const ()>` gives `!Send` while preserving both; an interior
      `RefCell` or `Cell` in the struct would silently lose `RefUnwindSafe` and stop those
      tests compiling.

      **Review gate.** Each assertion method has a caller outside
      `crates/test-log-capture` — the crate's own tests do not count, since downstream use
      is what the pin exists to protect. The target-scoped variant is the one with no
      natural consumer, so the netcode tests use it deliberately. Derives on
      `CapturedRecord` (`Debug`, `Clone`, and whatever `records()` and the failure messages
      need) are exempt from "nothing beyond the pinned set".

## Tasks

### Task 1: The capture crate

Add `crates/test-log-capture` (package `postretro-test-log-capture`), `log` its only
dependency, registered in the root `Cargo.toml` under `[workspace] members` and
`[workspace.dependencies]`.

One `src/lib.rs`. A zero-sized `log::Log` impl in a `static`. `enabled` returns `true`
unconditionally — filtering is an assertion-time concern, per the `RUST_LOG` scope
decision, and a level-gated `enabled` would defeat AC-CAP-4. A `Once`-guarded installer
calls `set_logger` and `set_max_level(LevelFilter::Trace)`, storing the `set_logger`
result in an `AtomicBool`; `Once::call_once` establishes the happens-before that lets a
concurrent `start()` read it safely.

The thread-local is `RefCell<Option<Arc<Mutex<Vec<_>>>>>`. Its element type is private:
it may retain sequence metadata, while `records()` exposes only the pinned
`Vec<CapturedRecord>` surface. Reserve a monotonically increasing sequence on entry to
`Log::log`, then order the public record list by that sequence. A `Display` impl that logs
while the outer record formats therefore cannot reverse logger-entry order.

Two disciplines are load-bearing and both were violated by the loggers this replaces.
First, `log` formats `record.args()` into a `String` before touching the capture slot, then
clones the `Arc` out and lets the `Ref` drop **before** locking — no `RefCell` borrow may
be live across a `Mutex` acquisition, or a caller `Display` impl that itself logs
deadlocks. The shipped helpers all hold a `borrow_mut` across the push; do not copy them.
Second, access the thread-local through `try_with`: the slot holds a destructor, so a
`log` call during TLS teardown would otherwise panic inside an unwind and abort. Treat
`Err(AccessError)` as an orphan.

`start()` runs the installer, panics if the stored flag says another logger owns the
process, panics if the thread's slot is occupied, then installs a fresh empty buffer —
which is what makes "cleared on construction" true without a drain. `Drop` `take`s the
slot, so an unwind clears it. Records arriving with no buffer increment a global orphan
counter. They also go to stderr, but **only when `POSTRETRO_LOG_CAPTURE_ORPHANS` is set**:
`set_max_level(Trace)` makes this logger the sink for every call site in the binary, and
`crates/ui` alone runs 237 tests with several deliberately emitting `[UI]` warns, so an
unconditional echo would bury real output. The counter is unconditional; AC-CAP-8 needs
only the count.

Assertions panic on failure with the full message. Unit-test AC-CAP-1 through AC-CAP-6,
AC-CAP-8, AC-CAP-9, and AC-CAP-7's double-guard half in-crate, including two-thread
isolation and a `catch_unwind` unwind case. AC-CAP-7's foreign-logger half needs its own
`tests/` integration target — a single `#[test]` that installs a stub `log::Log` and then
calls `start()` — because the condition can only be produced once per process. No `unsafe`
(`development_guide.md` §3.5).

Regenerate `context/lib/crate-graph.md` with `cargo run -p xtask -- crate-graph --write` in
the same change: `layers` ranks every workspace member, so the zero-edge member lands in
`Layer 0 (leaves)` and `check_committed_doc` fails until the doc is rewritten. That is the
only `context/lib` edit this task owns — §Workspace no longer carries a hand-maintained
crate roster, so adding a member goes stale nowhere else.

### Task 2: Retire the two `crates/ui` loggers

Add a `[dev-dependencies]` section to `crates/ui/Cargo.toml` — it has none — and convert
both test modules. They share one test binary, so converting either alone leaves a
collision that `start()` now turns into a hard panic.

`theme_gate_test.rs`: delete `CountingLogger`, `WARN_COUNT`, `LOGGER_INIT`,
`WARN_TEST_LOCK`, and `install_logger`. Drop `warns_for_build` entirely rather than
reshaping it to return `usize` — its three callers each construct a guard, build the tree,
and use the exactly-once assertion directly, which is what the new surface is for. The
probe-and-skip existed only because the old logger could lose the race silently.

`tree/tests/local_state.rs`: delete its own `CountingLogger`, `WARN_COUNT`, `LOGGER_INIT`,
`WARN_LOCK`, and `logger_active` probe. Both branches this file currently guards behind
that probe become unconditional assertions — one `[UI]` warn at build for an orphan local
bind, and none on the retained draw path. These are the assertions that are dead today.

Both files: drop the now-unused `AtomicUsize`/`Ordering`/`Mutex`/`Once` imports, and
update the header and doc comments that describe the counting logger and the skip path.

### Task 3: Adapt the two remaining loggers

`crates/postretro/src/scripting/reactions/log_capture.rs` and the `#[cfg(test)]` helper in
`crates/renderer/src/lighting/lightmap.rs` both wrap a closure and return its records.
Reimplement each body over `LogCapture` and keep the signature, so no call site moves —
40 references across 16 files in `postretro`, one in `renderer`.

`capture<F: FnOnce()>(f: F) -> Vec<(Level, String)>` becomes: construct a guard, run `f`,
map `records()` to the `(level, message)` pairs its callers expect. Delete `CaptureLogger`,
`install`, the `OnceLock`, and the thread-local. The module keeps its name, its
`pub(crate)` visibility, **and its `#[cfg(test)]` gate** — dropping that gate turns a
dev-dependency into a normal one and breaks AC-BUILD-1. Same for the renderer helper,
which lives inside a `#[cfg(test)] mod tests`.

`capture_logs` drops its `Option`: the `None` arm existed only to signal a lost race,
which now panics, so it returns `Vec<(Level, String)>`. Delete the skip arm in its one
caller, `oversize_section_logs_renderer_prefixed_error`. The two helpers' parameter shapes
differ — `postretro` takes a generic `F`, `renderer` an `impl FnOnce()`; leave each as it
is.

Two things a grep for `log_capture::capture(` will miss. `crates/postretro/src/netcode/state_slots.rs`
imports the function directly (`use crate::scripting::reactions::log_capture::capture;`),
so its call sites are bare `capture(...)`. And no existing site nests `capture` inside
another — verified — which matters because the new `start()` panics on a nested guard
where the old helper silently re-armed.

Add the `[dev-dependencies]` edge to both `crates/renderer/Cargo.toml` and
`crates/postretro/Cargo.toml`; both have a section already.

### Task 4: Assert three log-only degradation paths

Add `postretro-test-log-capture` to `[dev-dependencies]` in `crates/net/Cargo.toml` (no
section yet) and `crates/scripting-core/Cargo.toml` (has one). `crates/postretro`'s edge
already exists from the preceding phase — do not re-add it.

The netcode and engine sites already have behavioral coverage; extend those tests rather
than writing parallel ones. The scripting site has none — those three tests are new.

*`crates/net`.* `process_control_messages` (`transport.rs`) logs the accept at `info` and
the reject at `warn`, both `[Net]`-tagged. `loopback_matching_version_is_accepted` and
`loopback_diverged_app_version_is_rejected_with_typed_reason` already drive both arms and
assert the `HandshakeOutcome`; the operator-facing diagnostic has never been asserted.
`renet` is polled, not threaded, so both records land on the test thread. Assert the
accept path logs the accept and no reject, and the divergent path the reverse. Use the
target-scoped assertion here — `"postretro_net"` as the prefix — rather than the plain
level-plus-body form. These are the only tests that exercise that variant, and
AC-SURFACE-1's review gate requires it to have a caller outside the capture crate.

*`crates/scripting-core`.* Three functions in `store_bridge.rs` refuse a write to a
read-only slot, log `[Scripting] … rejected write to readonly slot`, and return `Ok(())`:
`write_script_store_slot`, `write_state_slot_json`, and `apply_text_edit`. Note
`write_store_slot` also labels its errors `storeWrite` but has no readonly guard — not a
target. The slot value discriminates a refusal from a differing write; only the log says
*which* of the three refused. Nothing tests any of them today: `store_bridge.rs`'s test
module covers only `store_declaration`, and the write-path tests in
`crates/postretro/src/scripting/state_store.rs` never touch a read-only slot. Add the
three tests to `store_bridge.rs`'s `mod tests`. Fixture route: `ScriptCtx::new()`
(`crates/entities/src/ctx.rs`) is a plain `Rc<RefCell<_>>` struct needing no rquickjs or
mlua runtime. `SlotSchema` lives in `crates/entities/src/slot_table.rs` and derives no
`Default`, so all eight fields must be written out — `ctx.rs`'s `health_slot()` test
fixture is a working literal with `readonly: true` already, and is the thing to copy. Each
readonly guard precedes value conversion, so no valid payload is needed.

*`crates/postretro`.* `materialize_net_mesh_presentation`
(`scripting/builtins/net_descriptor.rs`) warns on an unregistered `entity_class` and logs
the meshless-descriptor case at `debug` with a different body. Both return `false`, so
level and body are the only discriminator.
`remote_enemy_presentation_unknown_class_leaves_transform_only` already covers the first
branch's return value and mesh absence — add the log assertion there. The meshless branch
has no test at all; add one. The module's only descriptor fixture, `enemy_mesh_descriptor`,
hardcodes `mesh: Some(..)`, so that test needs
`EntityTypeDescriptor { mesh: None, ..enemy_mesh_descriptor(..) }`. This is E15's
AC-DIGEST-3 site, and the only place level-plus-body matching and `debug` capture are
exercised against real engine code.

One note for a reader routed to `development_guide.md` §Workspace, which says
`postretro-net` is "postretro-free": a dev-dependency is not a violation. `collect_graph`
drops non-normal edges, so the layering graph never sees it, and the capture crate pulls
in only `log` — `crates/net`'s no-async-runtime `cargo tree` gate is unaffected.

## Sequencing

All three phases land before merge; E15 starts after.

**Phase 1 (sequential):** Task 1 — the crate must exist first.
**Phase 2 (sequential):** Task 2 — thin slice. Two loggers in one binary is the hardest
case, and it falsifies parallel isolation and exactly-once while the surface is still
cheap to move.
**Phase 3 (sequential):** Task 3, then Task 4. Both edit `crates/postretro` — Task 3 the
reactions capture module, Task 4 `net_descriptor.rs` — and both need its
`[dev-dependencies]` edge, which Task 3 adds. Running them concurrently races that manifest
line. Last chance to move the surface.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| One test logger per binary, workspace-wide; losing the race panics rather than skipping | Task 1 (`Once` installer, stored `set_logger` result, panic on collision) | Tasks 2 and 3 delete all four incumbents. A fifth would break only its own binary, which no per-crate test can catch — the grep gate is what generalizes | AC-CAP-7, AC-LOGGER-1 |
| No test needs a serializing lock to assert on logs | Task 1 (per-thread buffers) | Task 2 removes `WARN_TEST_LOCK` and `WARN_LOCK`; re-adding a suite-wide mutex restores the defect | AC-CAP-5, AC-UI-1 |
| Existing capture call sites do not move | Task 3 (adapters keep each helper's parameter shape) | 40 `postretro` references across 16 files plus the module declaration, one of them a bare `use`-imported `capture(..)`; a parameter change turns a 2-file task into an 18-file one. The renderer helper's return type is the one deliberate exception, with a single caller | AC-ADAPT-1 |
| Runtime logging is unchanged | Task 1 (dev-dependency only) | Tasks 2–4 add five manifest lines; promoting any out of `[dev-dependencies]` links the logger into the shipped binary | AC-BUILD-1 |

## Open questions

- **Cross-thread capture.** The `Arc<Mutex<_>>` slot keeps a `Send` handle additive. The
  trigger for building it: a test needing the `[Loader]` warns from `run_worker`. No E15
  criterion needs that. Until then the orphan count is what explains an empty capture.
- **`crates/level-compiler`'s reporter tests.** They construct `CollectingLogger` and call
  `log()` on it directly, never installing globally — no race, nothing to gain. Out of
  scope, recorded so it is not re-derived.
- **The `context/lib` capture is landed, deliberately ahead of the code.**
  `testing_guide.md` §3 carries the log-assertion contract in prose — one logger per test
  process, collision panics rather than skips, per-thread buffers, level-plus-body
  matching, orphan counting, and the rule that an asserted log line becomes contract. It
  names no signatures: per `context_style_guide.md`, method names and code samples are
  ephemeral, so it points at `crates/test-log-capture` for the shape. That pointer
  resolves when Task 1 lands. The router entry is in place, which is what makes the
  harness discoverable to an E15 task agent that never reads this plan.
