# Research — test log capture

Grounding notes for `index.md`. Decisions live in the spec; this is what produced them.

## The premise was half right

"No log-capture exists anywhere in `crates/`" is true for *test* capture, but two
pieces of prior art matter.

### 1. `crates/level-compiler/src/logger.rs` — a shipped `log::Log` impl

Production code, not test infrastructure: the warning sink behind the compiler's
plain and interactive reporters. It already defines

```rust
pub struct CapturedRecord {
    pub level: Level,
    pub target: String,
    pub message: String,
}
```

— the exact three-field owned record the harness needs (`logger.rs:13-19`), plus
`install()` calling `log::set_boxed_logger` + `log::set_max_level` (`logger.rs:141-152`)
and an `env_filter::Filter` for `RUST_LOG`-compatible directives.

Not reusable from `crates/net`: `postretro-level-compiler` is a **binary** crate
carrying `shambler`, `ratatui`, `crossterm`, `nalgebra`, `bvh`, `rayon`. An edge
from `net` to it inverts the layering in `development_guide.md` §Workspace. The new
crate mirrors the field set and the `install()` shape; it shares no code.

### 2. `crates/ui/src/theme_gate_test.rs:200-322` — an ad-hoc test logger, already fighting the race

A `CountingLogger` behind a `Once`, a process-global `AtomicUsize WARN_COUNT`, and a
process-global `Mutex WARN_TEST_LOCK` that serializes the three tests using it. Its own
comment states the failure mode:

```rust
// Ignore an Err: another test (or env_logger) may have set a logger
// first; the count is only meaningful under the serial lock anyway, and a
// pre-installed logger means our counter never increments — guarded below.
```

`warns_for_build` returns `Option<usize>`; all three call sites (`:254`, `:287`, `:317`)
`eprintln!("… skipping: another logger is installed")` and pass. **A broken harness
reads as green.** This is the strongest available argument for the loud-panic
requirement (AC-8) and the reason the UI migration is the thin slice — it is a real
exactly-once consumer that exists today.

## Cross-thread logging is real, and grounded

`crates/postretro/src/startup/worker.rs:42-63` — `spawn_level_worker` runs on a
`thread::spawn`ed worker and emits:

- `log::info!("[Loader] PRL loaded successfully from {path_str}")`
- `log::warn!("[Loader] PRL file not found: {p} — starting without map")`

A thread-local buffer cannot see these. That is the whole reason for orphan counting
(AC-9) and for holding the buffer behind `Arc<Mutex<…>>` in the thread-local slot
rather than a bare `RefCell<Vec<…>>` — a `Send` handle is then additive.

`crates/scripting-core/src/staged_manifest.rs:65` also spawns a worker, but it is
**not** a hazard for E15's six `Debug-build criterion` hot-reload ACs: the file contains
zero `log::` calls. The worker returns structured `StagedManifestDiagnostic` values
over an `mpsc` channel; the commit-and-warn site E15 Task 2 adds runs on the polling
(main) thread.

## The E15 demand, verified

Verified log-asserting ACs (`context/plans/drafts/E15--session-lifecycle/index.md`):

| AC | Line | Demand |
|---|---|---|
| AC-GATE-5 | 338 | version skew "appears in a host-side log and nowhere else" — `info`, both versions. Task 6 detail at `:592-595` |
| AC-MANIFEST-2 | 409 | staged reload changing mod id/version "**warns** and leaves the installed value unchanged". Detail at `:1038` |
| AC-LEVEL-7 | 449 | absent catalog id "logs a diagnostic naming the id". Detail at `:802` |
| AC-DIGEST-3 | 361 | transform-only degradation; `materialize_net_mesh_presentation` logs it (`:210`) |

Plus the wire-decode failure path (`:1024`), the absent-half degradation (`:1100`), and
the boot-load early-return warning (`:807`). The `[Net]` body tag is live today:
`crates/net/src/transport.rs:365,390,394`. `development_guide.md` §6.1 makes the
`[Subsystem]` prefix a rule, which is why **body substring is the primary key** and
target is a secondary filter.

## Shipped log-only degradation paths (Task 3's real targets)

Task 3 was originally "emit a record, assert the record" — a test of the `log` crate, not
of this one. Grounding turned up three shipped sites per the keep-if-it-tests-our-logger
rule, all currently untested:

| Crate | Site | Why the log is the only observable |
|---|---|---|
| `net` | `transport.rs:390` (`info`, `[Net] client {id} accepted`), `:394` (`warn`, `[Net] rejecting client {id}`) | Reached today by `loopback_matching_version_is_accepted` and `loopback_diverged_app_version_is_rejected_with_typed_reason`, which drive a real `NetServer` over a bound UDP socket. `HandshakeOutcome` is asserted; the operator-facing diagnostic never is. Exactly the pair E15's admission criteria extend |
| `scripting-core` | `store_bridge.rs:75`, `:116`, `:139` — `storeWrite`, `setState`, text-edit | All three **return `Ok(())`** after refusing a write to a read-only slot. The caller cannot distinguish refusal from success. The only existing readonly test (`:806`) covers declaration validation, not the write refusal |
| `postretro` | `scripting/builtins/net_descriptor.rs:226` (`warn`, unregistered class), `:233` (`debug`, no mesh block) | Both return `false` from `materialize_net_mesh_presentation`, so the return value cannot discriminate the two causes — level and body are the only signal. This is E15's AC-DIGEST-3 site, and the one place `debug`-level capture (AC-CAP-4) is exercised against real engine code |

The `postretro` row is also the clearest illustration of the boundary in
`Alternatives rejected`: a return value *does* exist here, so a value assertion covers
"transform-only." It cannot cover *why*, and the two reasons are deliberately logged at
different levels.

## Workspace-member side effects

`crates/xtask/src/crate_graph.rs` builds its graph from `cargo metadata --no-deps`,
keeping only **normal (non-dev, non-build)** edges (`crate_graph.rs:32-42`, module doc
`:1-20`). Consequences of adding a dev-only member:

- `layering_invariants_hold` (`:496`) is unaffected — it asserts on `postretro`
  dependents, `foundation` deps, and `entities` deps only, none of which move.
- `render_doc` (`:~330`) enumerates `graph.layers()` over **all** members. A zero-edge
  member lands in `Layer 0 (leaves)`, so the committed `context/lib/crate-graph.md`
  goes stale and `crate-graph --check` returns 1 (`check_committed_doc`). Regeneration
  via `--write` is mandatory in the same commit.
- `dependents_ranking` omits crates nothing depends on, so the chokepoint section is
  untouched.
- `development_guide.md` §Workspace says "17 crates in a Cargo workspace" and carries a
  per-crate table — both need the new row at promotion.

## `testing_logger` status

crates.io: latest version **0.1.1, published 2018-08-07**, ~3.9M downloads,
`github.com/brucechapman/rust_testing_logger`. Same thread-local shape. Missing: an
exactly-once helper, and any loud signal when it loses the `set_logger` race — the
precise defect the `crates/ui` prior art demonstrates.

## Guard lifecycle

```mermaid
stateDiagram-v2
    [*] --> NoLogger
    NoLogger --> Installed : first LogCapture::start() — set_logger + set_max_level(Trace)
    NoLogger --> Contended : set_logger returns Err (another logger owns the process)
    Contended --> [*] : start() panics naming the conflict (never captures silently)

    state Installed {
        [*] --> Detached
        Detached --> Attached : start() puts a fresh Arc<Mutex<Vec>> in the thread-local slot
        Attached --> Attached : log() formats, clones the Arc, pushes in order
        Attached --> Detached : Drop clears the slot (runs on unwind too)
        Attached --> Panic : start() called again on this thread
    }

    note right of Detached
        A record logged here has no buffer:
        ORPHANS += 1, reported in every
        assertion failure message.
    end note
```

`start()` clears by *installing a fresh buffer*, and `Drop` clears by *removing it* —
so "cleared on construction and on drop" needs no drain call and holds identically
under a panicking test, whose unwind runs `Drop`.

Re-entrancy note: `record.args().to_string()` runs caller `Display` impls, which may
themselves log. The logger formats the message **before** touching the thread-local,
and clones the `Arc` out of a shared borrow before locking, so a re-entrant log cannot
deadlock or double-borrow.
