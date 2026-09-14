# gameplay-stack--sim-and-netcode-crates — plan of record

mode: resumable
status: approved
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
| no test is lost and every moved test is named | workspace test count comparison; per-test move ledger below | achievable as stated | |
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
| 1 | Partition `scripting/systems` at the frame/fixed-tick seam: keep frame-path bridges binary-side, re-root fixed-tick handlers without widening their surface, and keep `mesh_render` consuming the moved CPU stores through that seam. Sink `ClipMetadata` to `postretro-model` and `PresentationDrawInput` to `postretro-foundation`; prove the record sinks and fixed-tick/frame-path dependency direction with focused tests. | integrating executor | — | complete — `cargo test -p postretro-foundation -p postretro-model -p postretro-render-cpu` (396 passed); `cargo check -p postretro`; `clip_table_resolves_names_and_keeps_first_on_duplicate` and `pool_uses_frame_time_for_new_spawn_age_and_expiry` (1 each) |
| 2 | Prepare the boundaries on the one-crate tree: relocate frame timing/presentation pool, sink weapon placement, separate spawn windup, move App drains up, establish test-support surfaces, and resolve all binary-only test reaches (including the delegated lifecycle helper). | integrating executor | 1 | complete — `cargo check -p postretro` and `cargo check -p postretro --features test-support`; focused placement resolver (1), remote attachment (1), windup guard (1), seat cleanup (1), touch tick (1), CLI composition (1), projectile presentation (1), and client control drain (1) |
| 3 | Create and transplant `postretro-sim` wholesale: collision, fixed-tick systems, scripting runtime/handlers, bins, allocator, manifests, and public seams; retain only deliberate test-support access. | integrating executor | 2 | complete — `cargo check -p postretro --bin postretro`; `cargo check -p postretro-sim --bins`; sim normal tree excludes the render stack and netcode; relocated generator preserves both SDK type-file hashes. Sim test-target compilation is intentionally completed with Task 4, when its retained netcode harness moves into the temporary crate. |
| 4 | Create and transplant `postretro-netcode` wholesale above sim. Move only the `ingest_hit_declaration_for_test` callers from `sim/weapon_stage.rs` and `scripting/systems/ai_tests.rs` into netcode-owned harness coverage (retain their unrelated source-adjacent tests), then move/drop every remaining binary-only test reach and complete the per-test move ledger. | integrating executor | 3 | complete — `cargo check -p postretro-netcode`; `cargo check -p postretro --bin postretro`; `cargo test -p postretro-sim --no-run`; moved ingress harness (6 passed) and snapshot harness (2 passed) run in netcode. |
| 5 | Integrate binary orchestration and durable docs (`development_guide.md` target shape and `scripting.md` command examples); verify dependency trees, isolation, graph, targeted behavior, code visibility audit, and all automated acceptance before review. | integrating executor | 4 | pending |
| 6 | Run review-panel → fix-review-findings → focused retest, then final preflight; record automated results and leave manual runbook evidence explicitly outstanding or land after it arrives. | integrating executor | 5 | pending |

## Remaining execution list

1. Complete Task 4: move `netcode/` into `postretro-netcode`, replace the temporary crate root, configure sim's test-only netcode edge, and relocate the ledger's remaining binary-only tests.
2. Complete Task 5: wire binary orchestration to both crates, remove moved binary sources, update context/docs and generated crate graph, then run the acceptance-specific checks and isolation experiment.
3. Complete Task 6: review-panel and fix loop, focused retests, one final preflight, and an explicit manual test-ready or landed-with-gaps result.

## Per-test move ledger

The test count is recorded before the transplant and compared after it. This ledger is updated in the same commit as each test relocation; a source-adjacent test stays in place unless it requires a netcode function whose signature names a sim-defined type.

| Current location | Planned ownership | Reason |
|---|---|---|
| `sim/weapon_stage.rs` — `ingest_hit_declaration_for_test` callers | netcode-owned harness coverage | The helper names sim `CollisionWorld` and `HitZoneStore`; a sim test target would see a second sim compilation through its netcode dev-dependency. |
| `scripting/systems/ai_tests.rs` — `ingest_hit_declaration_for_test` callers | netcode-owned harness coverage | Same type-identity constraint; the tests exercise a netcode ingestion entry point. |
| `sim/touch.rs`, `sim/determinism_tests.rs`, `impact_effects.rs` harness reaches | remain source-adjacent in sim through netcode `test-support` only where their signatures use shared lower-crate types | They do not carry a sim-defined type across the dev-dependency cycle. |
| netcode tests named in the brief Path table | netcode if they reach sim/normal dependencies; binary tests or reach removal otherwise | Nothing moves down merely to serve a test. |
| `netcode/seat.rs` hold-expiry tests | remain netcode | `clear_released_seat_slot_values` now lives beside `SeatTable`; no binary root reach remains. |
| `netcode/remote_materialize.rs` attachment test | remain netcode | It resolves bindings through sim and asserts `AttachmentBinding`; the former frame attachment-emission assertion is deliberately removed. |
| `netcode/projectile_presentation.rs` observer test | remain netcode | It asserts registry-side sprite/light materialization and cadence data; renderer collector/light-bridge assertions are removed. |
| `netcode/mod.rs` positional-map CLI test | binary test | It composes net flag parsing with binary `resolve_map_path`. |
| `sim/touch.rs` zero-tick latch coverage | split | The sim test now asserts a direct latched press; the input-latch frame behavior remains binary-owned coverage. |
| `netcode/state_slots.rs` UI snapshot seam | binary test pending Task 4 | It reaches `App::build_ui_slot_snapshot`; retain the net replication fixture behind netcode `test-support` when moved. |
| `enemy_replication_harness_test.rs` trigger-pool fixture | binary integration test or sim installation seam pending Task 4 | Do not let a netcode test use a binary-only lifecycle fixture. |

## External manual runbook

1. Run `cargo run -p xtask -- run content/dev/maps/campaign-test.prl`; exercise movement, enemy engagement, weapon fire, triggers, movers, animated enemies/hit zones, and presentation bridges.
2. Run `cargo run -p xtask -- run --features dev-tools -- content/dev/maps/campaign-test.prl`; verify the debug panel draws.
3. Start a loopback host and client; verify join, replication, reconciliation, then a host level change.
