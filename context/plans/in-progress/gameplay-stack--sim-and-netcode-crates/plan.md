# gameplay-stack--sim-and-netcode-crates — plan of record

mode: resumable
status: proposed
read at: 01d12d66

## Corrections

- `read at 3acc6d6` → `01d12d66` changes only the draft/ready plan files; no source, manifest, or routed context source changed. The promoted Decisions and Path remain grounded.

## Delegated answers

- `install_descriptor_player_health_range` boundary — move the helper to sim; it uses only `SlotTable`, `MapEntity`, and entity descriptors, and its production level-install caller is a normal binary → sim edge. Keep `descriptor_health_range_is_role_invariant_and_accepts_first_baseline` in `postretro-netcode/state_slots.rs`: it exercises netcode replication/apply and calls sim through netcode's normal dependency.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| `cargo build --workspace`, `cargo test --workspace`, and `cargo build -p postretro --features dev-tools` pass | final preflight | achievable as stated | |
| `layering_invariants_hold` and regenerated `crate-graph.md` pass and list both crates | focused invariant test; `xtask crate-graph --write --check` | achievable as stated | |
| sim normal tree excludes render stack and netcode, while includes VM runtimes through scripting-core | `cargo tree -p postretro-sim -e normal` | achievable as stated | |
| netcode normal tree includes sim and excludes render stack | `cargo tree -p postretro-netcode -e normal` | achievable as stated | |
| sim library omits netcode but sim test targets link it | `cargo build -p postretro-sim`; `cargo test -p postretro-sim --no-run` | achievable as stated | |
| binary/sim/netcode edit isolation holds | warm-build `touch` experiment with Cargo recompilation evidence | achievable as stated | |
| no moved module remains under `crates/postretro/src/` | path audit plus grep gate | achievable as stated | |
| no test is lost and every moved test is named | workspace test count comparison; named moves in Task 4 | achievable as stated | |
| M15 Phase 0 determinism harness is unchanged and passes | focused `postretro-sim` determinism test | achievable as stated | |
| `gen-script-types` preserves `sdk/types/` bytes and committed typedef test passes | byte comparison and focused typedef test | achievable as stated | |
| xtask `mint-identity` invokes the sim binary and produces a mod sidecar at the unchanged SDK depth | focused xtask wrapper test | achievable as stated | |
| weapon placement preserves legacy and instance-authored precedence | focused foundation/sim/netcode placement tests | achievable as stated | |
| spawned-enemy windup is 250 ms and preserves larger cooldown | focused spawner test | achievable as stated | |
| windup guard is one-way against netcode interpolation ceiling | focused cross-crate netcode test | achievable as stated | |
| allocation guards still arm through sim's crate-root allocator | focused alloc-probe test with deliberate guarded allocation | achievable as stated | |
| every `pub(crate)` widening has an external caller | review/audit of each changed public item | achievable as stated | |
| campaign-test plays identically | owner, in-engine runbook | manual-visual | |
| animated clips and hit zones work on moving animated enemies | owner, in-engine runbook | manual-visual | |
| lights, fog, emitters, particles, and damage overlays present correctly | owner, in-engine runbook | manual-visual | |
| dev-tools launch and debug panel draw | owner, in-engine runbook | manual-visual | |
| co-op joins, replicates, reconciles, and survives host level change | owner, two-process loopback runbook | manual-network | |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Partition `scripting/systems` at the frame/fixed-tick seam and sink `ClipMetadata` to `postretro-model` plus `PresentationDrawInput` to `postretro-foundation`; prove that `hit_zones → mesh_anim` is the only surviving fixed-tick link. | integrating executor | — | pending |
| 2 | Prepare the boundaries on the one-crate tree: relocate frame timing/presentation pool, sink weapon placement, separate spawn windup, move App drains up, establish test-support surfaces, and resolve all binary-only test reaches (including the delegated lifecycle helper). | integrating executor | 1 | pending |
| 3 | Create and transplant `postretro-sim` wholesale: collision, fixed-tick systems, scripting runtime/handlers, bins, allocator, manifests, and public seams; retain only deliberate test-support access. | integrating executor | 2 | pending |
| 4 | Create and transplant `postretro-netcode` wholesale above sim; move sim-type test callers `sim/weapon_stage.rs` and `scripting/systems/ai_tests.rs` into netcode, move/drop remaining binary reaches, and name every changed test file. | integrating executor | 3 | pending |
| 5 | Integrate binary orchestration and durable docs; verify dependency trees, isolation, graph, targeted behavior, code visibility audit, and all automated acceptance before review. | integrating executor | 4 | pending |
| 6 | Run review-panel → fix-review-findings → focused retest, then final preflight; record automated results and leave manual runbook evidence explicitly outstanding or land after it arrives. | integrating executor | 5 | pending |

## External manual runbook

1. Run `cargo run -p xtask -- run content/dev/maps/campaign-test.prl`; exercise movement, enemy engagement, weapon fire, triggers, movers, animated enemies/hit zones, and presentation bridges.
2. Run `cargo run -p xtask -- run --features dev-tools -- content/dev/maps/campaign-test.prl`; verify the debug panel draws.
3. Start a loopback host and client; verify join, replication, reconciliation, then a host level change.
