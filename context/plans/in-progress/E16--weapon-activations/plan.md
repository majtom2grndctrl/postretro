# Weapon activations — execution record

Status: active.
Branch: `codex/weapon-activations`.
Owner approved the public API and network migration. The demo multiplier remains authored data.

## Coordination

The approved spec owns behavior. Workers own bounded implementation tasks; coordinator owns integration, review, and final checks. Run one Cargo producer against the shared target at a time. Preserve unrelated window-mode and gamepad drafts.

Plan promotion and claim are committed and published on main through `1c7bda3c7`. Automatic approval review initially rejected that publication; the owner then explicitly authorized the push, which succeeded. Implementation stays on the feature branch.

## Tasks

| Task | Owner | Status | Evidence |
|---|---|---|---|
| 1. Network timing and shared kernel | activation_timing | complete | 64 sim weapon regressions, 4 kernel tests, 2 wire/version tests, 19 netcode tests, 2 input tests passed; production check passed; independent review fixes verified |
| 2. Descriptor and SDK | authoring_prepare | complete | Foundation 6, legacy IR 10, component restore 1, scripting activation 6/weapon 33, sim allocation 1/tuning 4/typegen 38, netcode tuning 8/fixture 1, compiler Luau 7 passed; final production check passed |
| 3. Shared weapon execution | execution_prepare | next | Pending |
| 4. Prediction and presentation | unassigned | waiting for execution | Pending |
| 5. Verification and review | coordinator | preparing runbook | Pending |

## Acceptance proof

| AC | Required proof | Status |
|---|---|---|
| 1 | Primary-fire regression tests and reference launch | pending |
| 2 | Full/partial plasma tests; authored 3×/6× fixtures on host/client | pending |
| 3 | Three timed shots, three debits, release/hold and aim/bloom tests | pending |
| 4 | Resource matrix, interruption, lifecycle tests | pending |
| 5 | TS/Luau parity, invalid data, generated type gate, API review | pending |
| 6 | Conditioned link and command queue edge/identity tests | pending |
| 7 | Render catch-up and immutable shot/correction tests | pending |
| 8 | HUD lifecycle and owner/remote presentation; visual/audio check | pending |
| 9 | Bounded allocation/work proof and repeatable before/after timings | pending |
| 10 | Focused checks, independent review, final preflight, engine run | pending |

## Verification notes

No runtime implementation or performance result is implied by approval of the design. Record actual commands, test counts, measurement fixtures, and manual gaps here as work completes.

- Baseline source: `1c7bda3c7` (approved plan claim; runtime unchanged). Existing release executable predates that revision and is not a verified timing baseline.
- Local baked fixtures exist: `content/dev/maps/campaign-test.prl`, `spawner-test.prl`, and `movement-feel.prl`. Offscreen capture skips gameplay/HUD, so it cannot replace a running-engine weapon check.
- Task 1 shared types: foundation owns activation/shot identity and installed immutable timing data. Combat-model owns pure advancement. Full resource/FSM and presentation integration remain later tasks.
- Task 1 checks reported: `cargo test -p postretro-sim --lib weapon:: --quiet` (64 passed), combat-model activation tests (4 passed), and net activation tests (2 passed). These establish the foundation only; the acceptance rows remain pending integration.
- Task 1 independent review found declaration-overflow FIRE misclassification and expired unknown-edge replay; both repaired with regression tests. The follow-up found stale admission could reach execution despite rejected edge tracking; the queue now suppresses that start and exposes its token for a correlated rejection. Reviewer verified all three fixes.
- Additional Task 1 evidence: `cargo test -p postretro-netcode --lib activation --quiet` (19 passed), `cargo test -p postretro --bin postretro input::activation:: --quiet` (2 passed), `cargo check -p postretro-netcode -p postretro-sim -p postretro --quiet` (passed after final queue correction). Binary input tests prove quick press/release capture and zero-tick suspension cancellation.
- Task 3 must consume `ResolvedPawnCommand.rejected_activation`, bind actual execution to the host ledger, and remove the temporary unknown-start movement-cursor bridge. Tasks 3–4 must replace the client timing adapter's standalone cursor with central state ownership, wire hold restart and capture termination, consume reliable outcomes, and remove first-shot-only/empty-hit catch-up behavior. These are unfinished integration seams, not completed feature claims.
- Performance fixture: compare release `simulate_tick` on baseline and changed source with one pawn, one primary hitscan weapon, one stationary target, no AI/movers/scripts, identical fixed inputs and 1/60 delta. Build world/collision state once outside timing; existing `run_local_only_tick` rebuilds it and must not be measured directly. Warm 300 ticks, then collect 6000 tick samples into preallocated storage; report p50/p95 and actual encoded input bytes outside the measured window. Constructor adapters may reflect the descriptor/input schema migration but must preserve workload. Run serially under matching conditions and repeat to establish noise.
- Allocation proof can reuse the existing `sim::alloc_probe::AllocSnapshot` after installation and warmup around activation advancement and bound expression evaluation. Its allocator is existing approved code; no new unsafe instrumentation is needed. Full simulation allocations are a separate measurement from the activation hot-path claim.
- Task 2 independent review found that shared IR arithmetic totalization could hide overflow and reconstructed host tuning could erase a reload-switch override. Checked activation evaluation now retains invalid intermediate evidence using the existing evaluator walk; legacy evaluation keeps its semantics. Host tuning transports the optional reload-switch override. Reviewer verified both repairs.
- Task 2 scale evaluation allocation probe reported zero allocations. Host tuning preserves local lane sound overrides while applying host gameplay actions, including lane removal/addition. SDK helper extraction also updates the independent script-compiler Luau prelude consumer; its seven focused tests passed. Final `cargo check -p postretro-sim -p postretro-netcode -p postretro --quiet` passed. Only the documented unintegrated capture lifecycle methods remain unused until Tasks 3–4.
