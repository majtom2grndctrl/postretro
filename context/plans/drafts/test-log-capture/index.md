# Test Log Capture

## Goal

Give tests a parallel-safe way to assert on `log` records — level, target, and message
body, in emission order. Several Epic 15 acceptance criteria are written against warns
and diagnostics and are review gates today because nothing in the workspace can observe
a log record from a test. The one existing attempt (`crates/ui`) serializes its suite
and silently skips when it loses the `set_logger` race.

## Scope

### In scope

- A workspace crate, dev-dependency only, holding one process-global `log::Log` impl
  that routes records into per-thread buffers.
- An RAII guard that attaches a fresh buffer on construction and detaches it on drop,
  including during panic unwind.
- Positive, exactly-once, and negative assertions keyed on level plus message-body
  substring, with target as an additional filter, plus raw access to the ordered
  record list.
- Loud failure when the harness is not the process's active logger, and a count of
  records logged on threads with no buffer, surfaced in every assertion failure.
- Converting the three `crates/ui` warning-count tests off their ad-hoc counting logger.
- Wiring the dev-dependency into `crates/net`, `crates/postretro`, and
  `crates/scripting-core` with a guard test per crate.

### Out of scope

- **Capturing records emitted on threads the test did not create a guard on.**
  `crates/postretro/src/startup/worker.rs:42` logs `[Loader]` warns from a spawned
  worker; those are counted, not captured. See Direction for why the door stays open.
- **Migrating off the `log` facade.** The engine stays on `log` + `env_logger`.
- **Replacing `crates/level-compiler/src/logger.rs`.** That is a shipped production
  warning sink, not test infrastructure, and lives in a binary crate above `net`.
- **`RUST_LOG` / `env_filter` directive support.** The harness captures everything and
  filters at assertion time.
- **Rescuing E15's negative-existence criteria** — "no digest moves", "demotes nobody",
  "no table retains an entry". Those are value assertions on state and gain nothing
  from log capture.
- **Rescuing E15's AC-DIGEST-8** (a field addition must *fail to compile*). That stays a
  review gate plus the sentinel module. No `trybuild`.

## Direction

**Problem.** `log::set_logger` is process-global and settable once, and `cargo test`
runs tests in parallel threads by default. Every per-crate attempt at capture therefore
has to choose between serializing the suite and cross-contaminating, and the shipped
attempt in `crates/ui/src/theme_gate_test.rs` does both — a process-global counter, a
serializing `Mutex`, and three call sites that `eprintln!` and pass when another logger
won the race. A harness that reads green when it is broken is worse than none.

**Prior commitments.**
- `development_guide.md` §6.1 mandates a `[Subsystem]` tag prefix in the **message
  body** (`[Net]`, `[Loader]`, `[UI]`). Body substring is therefore the primary
  assertion key and `target` is a secondary filter, not the other way round.
- `crates/level-compiler/src/logger.rs:13-19` already defines
  `CapturedRecord { level, target, message }`. The new crate mirrors that field set so
  the workspace has one owned-record shape. It shares no code: `postretro-level-compiler`
  is a binary crate carrying `shambler`, `ratatui`, and `rayon`, and an edge from
  `crates/net` to it inverts `development_guide.md` §Workspace layering.
- `crates/xtask/src/crate_graph.rs` builds its graph from **normal** edges only, so a
  dev-only member adds a graph node with no edges. `layering_invariants_hold` is
  unaffected; the committed `context/lib/crate-graph.md` goes stale and the
  `crate-graph --check` preflight gate fails until regenerated.

*Divergence.* `testing_guide.md` §3 lists no log-assertion pattern, so this adds one.
The argument for it: §1 names "Degradation paths" a priority target, and for several
E15 degradations — the transform-only remote-entity fallback, the version-skew info
line, the absent-catalog-id diagnostic — the log record is the *only* observable the
degradation produces. The guide gains a §3 entry at promotion.

**Alternatives rejected.**
- **`testing_logger` from crates.io** — same thread-local shape, and the closest rival.
  Rejected on three grounds: latest release is 0.1.1 published 2018-08-07 (unmaintained
  for ~8 years); it offers no exactly-once helper, which E15 needs; and it gives no
  signal at all when it loses the `set_logger` race, which is the specific defect the
  `crates/ui` prior art demonstrates. The workspace annotates and pins every dependency;
  taking an 8-year-stale one to save ~60 lines is the wrong trade.
- **Per-crate ad-hoc loggers** (the shipped `crates/ui` shape) — rejected: duplicating a
  `log::Log` impl per crate reinstates two loggers in one binary, which is the race the
  harness exists to remove.
- **`--test-threads=1` or a `#[serial]` attribute** — rejected: makes correctness a
  runner flag, so a plain `cargo test` cross-contaminates.

**Placement.** The axis is workspace crate vs. per-crate test module, not engine vs. mod.
Chosen: workspace crate, dev-dependency only. Four crates need the same code, and the
per-crate alternative is the rejected alternative above. Recorded for the reviewer
rather than self-cleared.

**Foreclosures.** Installing the process logger from a dev-dependency forecloses any
*other* test-only logger in the same test binary — intended, and made loud rather than
silent. Cross-thread capture is deliberately not built, but the per-thread slot holds
`Arc<Mutex<Vec<_>>>` rather than a bare `RefCell<Vec<_>>` precisely so a `Send` handle
is an additive change, not a rewrite. Undoing the whole thing costs one crate deletion,
four manifest lines, and restoring the `crates/ui` counting logger from git history.
Not a one-way door.

## Acceptance criteria

- [ ] **AC-CAP-1** — A test asserting on a captured record matches on level **and**
      message-body substring together: the same substring emitted at a different level
      does not satisfy the assertion, and a different substring at the same level does
      not either. Records are additionally filterable by target (module path) prefix, so
      an assertion scoped to one crate's module path is not satisfied by an identical
      body logged from another crate's.
- [ ] **AC-CAP-2** — The raw record list is retained in emission order, and every
      record's level, target, and body are readable from it.
- [ ] **AC-CAP-3** — An exactly-once assertion passes against one matching record and
      **fails** against two; a negative assertion passes against zero and fails against
      one. Both are exercised in each direction, not just the passing one.
- [ ] **AC-CAP-4** — `trace` and `debug` records are captured, not only `warn` and
      above. (The `log` facade's runtime max level defaults to `Off`, so an uninstalled
      max level silently drops everything.)
- [ ] **AC-CAP-5** — **Parallel isolation.** Two threads each holding their own guard
      and each emitting distinct records see only their own. Neither observes the
      other's, and neither test needs a serializing lock.
- [ ] **AC-CAP-6** — **Panic hygiene.** After a test panics while holding a guard, the
      next guard constructed on that same thread observes zero records. Exercised
      through an actual unwind, not by calling a clear method.
- [ ] **AC-CAP-7** — **Loud on collision.** If another logger already owns the process,
      constructing a guard panics with a message naming the conflict. It never captures
      nothing and reports success — the failure mode the `crates/ui` prior art has today.
      Constructing a second guard on a thread that already holds one also panics rather
      than resetting the live buffer.
- [ ] **AC-CAP-8** — **Legible failures.** Every failed assertion prints what was
      expected, how many matched, and the full captured record list with level, target,
      and body for each. It also reports how many records were logged on threads with no
      buffer since the guard was constructed, labelled as a process-wide count, so a
      worker-thread emission reads as an explanation rather than as an empty capture.
- [ ] **AC-UI-1** — The three `crates/ui` warning-count criteria assert
      unconditionally: no skip path, no `Option` return, and no test-serializing mutex
      remain in that file. They pass under the default parallel runner, and the
      unknown-token cases still assert exactly one warning per build.
- [ ] **AC-REACH-1** — `crates/net`, `crates/postretro`, and `crates/scripting-core`
      each contain a test that captures a record emitted inside that crate's own test
      binary and asserts on it. Each fails loudly, per AC-CAP-7, if a competing logger is
      ever introduced into that binary.
- [ ] **AC-BUILD-1** — A non-test build of the engine links none of this: the capture
      crate does not appear among `postretro`'s normal-edge dependencies, and the shipped
      `env_logger` runtime path is unchanged.
- [ ] **AC-BUILD-2** — `cargo run -p xtask -- crate-graph --check` passes with the new
      member present, and `layering_invariants_hold` still passes.

## Tasks

### Task 1: The capture crate

Add `crates/test-log-capture` (package `postretro-test-log-capture`) as a workspace
member, with `log` as its only dependency, and register it in the root `Cargo.toml`
under both `[workspace] members` and `[workspace.dependencies]` alongside the other
`postretro-*` path entries. A single `src/lib.rs` holds: an owned `CapturedRecord`
mirroring the field set in `crates/level-compiler/src/logger.rs:13-19`
(`level: log::Level`, `target: String`, `message: String`); a zero-sized `log::Log`
impl held in a `static`; a `Once`-guarded installer that calls `log::set_logger` and
`log::set_max_level(LevelFilter::Trace)` and records in an `AtomicBool` whether
`set_logger` succeeded; a thread-local slot holding
`Option<Arc<Mutex<Vec<CapturedRecord>>>>`; and a `LogCapture` RAII guard. The guard's
constructor runs the installer, panics if the stored flag says another logger owns the
process, panics if the thread's slot is already occupied, and otherwise stores a fresh
empty buffer — which is what makes "cleared on construction" true without a drain call.
`Drop` sets the slot to `None`, so a panicking test's unwind clears it too. The `log`
impl formats `record.args()` into a `String` **before** touching the thread-local, then
clones the `Arc` out of a shared borrow and locks it, so a caller `Display` impl that
itself logs cannot deadlock or double-borrow; a record arriving on a thread with an
empty slot increments a global orphan counter instead. Expose positive, exactly-once,
and negative assertions over (level, body substring), a target-prefix-scoped variant, a
mid-test clear, and a method returning the ordered records. Every assertion failure
formats the expectation, the match count, the full record list, and the orphan delta
since the guard was constructed. No `unsafe` (`development_guide.md` §3.5). Unit-test
the crate against AC-CAP-1 through AC-CAP-8 — including the two-thread isolation case
and a `catch_unwind` panic-hygiene case. Finally, regenerate the committed graph
snapshot with `cargo run -p xtask -- crate-graph --write` and commit it in the same
change: `crate_graph.rs` enumerates every workspace member when rendering layers, so
the new zero-edge member lands in `Layer 0 (leaves)` and `crate-graph --check` returns
non-zero until the doc is rewritten.

### Task 2: Convert the UI warning-count tests

Add `postretro-test-log-capture` to a new `[dev-dependencies]` section in
`crates/ui/Cargo.toml` (the file has none today) and rewrite the block at
`crates/ui/src/theme_gate_test.rs:200-322`. Delete `CountingLogger`, the `WARN_COUNT`
static, the `LOGGER_INIT` `Once`, the `WARN_TEST_LOCK` mutex, and `install_logger`.
Rewrite `warns_for_build` to construct a guard, build the tree, and return a plain
`usize` rather than `Option<usize>` — the probe-and-skip branch exists only because the
old logger could lose the `set_logger` race silently, and the guard now panics in that
case. Update all three call sites (`:254`, `:287`, `:317`) to assert unconditionally and
drop their `eprintln!` skip arms. Keep the existing exactly-one-warning semantics: the
unknown-token cases still assert one `[UI]` warning per build, now via the exactly-once
assertion rather than a counter delta. This is the thin slice — it is the only real
exactly-once consumer that exists today, and it exercises the guard under the default
parallel runner against three tests in one binary that previously had to serialize.

### Task 3: Wire the three E15 consumer crates

Add `postretro-test-log-capture` to `[dev-dependencies]` in `crates/net/Cargo.toml`,
`crates/postretro/Cargo.toml`, and `crates/scripting-core/Cargo.toml` (all three already
have a `[dev-dependencies]` section except `net`, which needs one). In each crate add a
single guard test that constructs a capture, emits a record from within that crate, and
asserts on it by level and body substring. These are not filler: they verify the one
property that can silently regress per-binary — that no competing logger owns that
crate's test process — and they will fail loudly rather than silently the day one is
introduced. Use each crate's own `[Subsystem]` tag convention in the emitted body
(`development_guide.md` §6.1) so the test also demonstrates the substring-matching shape
that E15's criteria will use. Do not add assertions against E15 behavior that does not
exist yet; E15's own tasks own those.

## Sequencing

**Phase 1 (sequential):** Task 1 — the crate must exist before anything can depend on it.
**Phase 2 (sequential):** Task 2 — thin slice. Falsifies parallel isolation, exactly-once
semantics, and the dev-dependency edge against a real consumer before Task 3 replicates
the wiring three more times.
**Phase 3 (sequential):** Task 3 — consumes the guard semantics Task 2 validates.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Exactly one logger is installed per test process, and losing that race is loud rather than silent | Task 1 (`Once` installer + stored `set_logger` result + panic on collision) | Task 2 deletes the competing `crates/ui` logger; Task 3 must add no logger of its own. Any future crate introducing a second test logger breaks this in *that binary only* — which is why Task 3's per-crate tests exist | AC-CAP-7, AC-UI-1, AC-REACH-1 |
| No test needs a serializing lock to assert on logs | Task 1 (per-thread buffers) | Task 2 removes the shipped `WARN_TEST_LOCK`; re-adding any suite-wide test mutex to work around capture would silently restore the shipped defect | AC-CAP-5, AC-UI-1 |
| Runtime logging behavior is unchanged | Task 1 (dev-dependency only, no `env_logger` interaction) | Task 3 adds three manifest lines; promoting any of them out of `[dev-dependencies]` links the capture logger into the shipped binary | AC-BUILD-1 |

## Rough sketch

```rust
// crates/test-log-capture/src/lib.rs
pub struct CapturedRecord { pub level: Level, pub target: String, pub message: String }

pub struct LogCapture { buffer: Arc<Mutex<Vec<CapturedRecord>>>, orphans_at_start: usize }

impl LogCapture {
    pub fn start() -> Self;                                            // install + attach fresh buffer
    pub fn records(&self) -> Vec<CapturedRecord>;                      // ordered
    pub fn clear(&self);
    pub fn assert_logged(&self, level: Level, body: &str);             // >= 1
    pub fn assert_logged_once(&self, level: Level, body: &str);        // == 1
    pub fn assert_not_logged(&self, level: Level, body: &str);         // == 0
    pub fn assert_logged_from(&self, level: Level, target_prefix: &str, body: &str);
}
impl Drop for LogCapture { /* detach: slot = None */ }
```

Target matching is a **prefix** test against `record.target()`, which defaults to the
emitting module path — so `"postretro_net"` scopes to the whole crate and
`"postretro_net::transport"` to one module.

`LogCapture` must not be `Send`: the buffer it detaches on drop is the *thread's* slot,
so moving the guard across threads would detach the wrong one. A `PhantomData<*const ()>`
field is the usual way to express that.

Shape of the E15 assertions this enables, for reference — AC-GATE-5's host-side version
skew, AC-MANIFEST-2's staged-reload warning, AC-LEVEL-7's absent catalog id:

```rust
let capture = LogCapture::start();
server.poll_handshakes(&mut transport);
capture.assert_logged_once(Level::Info, "[Net] client 1 version");
capture.assert_not_logged(Level::Warn, "[Net] rejecting client 1");
```

## Open questions

- **Cross-thread capture.** Left out deliberately, and the per-thread slot's
  `Arc<Mutex<_>>` keeps the door open for a `Send` handle a spawned thread can install.
  The concrete trigger for building it: a test needing to assert the `[Loader]` warns
  emitted from `crates/postretro/src/startup/worker.rs:42`. No E15 criterion needs that
  today. Until then the orphan count in AC-CAP-8 is what tells a confused reader why
  their capture is empty.
- **Migrating `crates/level-compiler`'s reporter tests.** `logger.rs` tests its
  `CollectingLogger` directly, without ever installing it globally, so they have no race
  and nothing to gain. Out of scope, noted so a later reader does not re-derive it.
- **`context/lib` updates at promotion.** `development_guide.md` §Workspace says
  "17 crates" and carries a per-crate table; `testing_guide.md` §3 gains the
  log-assertion pattern and §4 the harness's placement. Per the drafting process these
  land at promotion, not now.
