# gameplay-stack--ai-and-physics-crates — plan of record

mode: resumable
status: in-progress
read at: 630c5554b

## Corrections

- The brief was grounded at `66e5caa`; current source is `630c5554b`. The only production seam change in cited source is the already-promoted M3 context wording; the tick edge, AI effects, physics subtree, graph helpers, and state constants still match the Decisions.
- Research says `EntityTypeDescriptor` is scripting-core-owned/re-exported; it now lives in `postretro-entities`. Planning around this by importing the type from entities in the new AI crate while leaving VM-coupled descriptor parsing in scripting-core.
- Research names two netcode test-only `AiRuntime` constructions; `ingest_hit_harness_test.rs` now also drives `simulate_tick_with_presentation_aim` and `AiRuntime` in its expanded cross-stage harness. Planning around this by migrating every netcode test-only tick caller to the injected seam and adding a dev-dependency on `postretro-ai`; production netcode remains AI-free.
- Research flags `alloc_probe` as a test-only boundary widening; current sim already exposes it under `#[cfg(any(test, feature = "test-support"))]`. The physics and AI crates will reuse that existing test-support seam rather than widen it again.
- The brief Path suggests extracting physics first. The build workflow requires the first task to test the riskiest premise through a thin slice, so the synchronous production `AiHost` seam and ORD ordering proofs land in place before the mechanical physics move. This changes task order, not any Decision or Acceptance meaning.
- Research treats `kinematic_mover` as depending only on collision and movement, but live `kinematic_mover/commands.rs` also owns production scripting-core registrar wiring. Planning around this by moving the deterministic command applier into physics while retaining the VM-coupled registrar/target wiring in sim as `mover_commands`; this preserves the accepted physics dependency tree and the scripting handler-placement rule.
- Compiling `postretro-sim`'s unit-test target after the AI move exposed Cargo's expected dev-cycle identity split: the test target's local `postretro-sim` types are distinct from the normal `postretro-sim` instance below `postretro-ai`. A `test-support`-only runner adapter reconstructs only the immutable `NavGraph` value across those identities; the production runner remains zero-copy, navigation stays high as the research decision requires, and neither sim nor netcode gains a normal AI edge. `test_tick_runner_nav_bridge_preserves_section_and_path_queries` directly proves serialized-section, path-query, and runner-visible behavior fidelity across the bridge.

## Delegated answers

- `data_archetype::find_descriptor` placement — keep the existing public helper in `postretro-sim` and import it from `postretro-ai`. AI already has an intentional down-edge to sim, the lookup operates on the sim runtime's descriptor slice, and moving it lower would add domain policy to the engine-data floor without reducing the dependency graph.

## Baselines

- Pre-split workspace test-name set: `baseline-test-names.txt`, 7,246 unique names, SHA-256 `5558dfc222033782254fb915d70ccd6f26234d0c78c216c510689ad63ee4210a`.
- Pre-split textual Rust `unsafe` count: 17 `rg -n '\bunsafe\b' crates --glob '*.rs'` matches. The only actual unsafe block/impl is the previously approved `crates/sim/src/alloc_probe.rs`; the remaining matches are prose or test strings.
- The following timings are local warm-cache spot measurements from one machine. Raw Cargo timing pages, console logs, and machine metadata were not retained, so the elapsed values are reported observations rather than independently auditable artifacts. Future comparisons use the exact capture template in `evidence/README.md`.
- Pre-split AI edit (`scripting_systems/ai/mod.rs` timestamp only): `cargo build -p postretro --timings` rebuilt sim, netcode, and the binary in 5.65s.
- Pre-split sim-proper edit (`impact_policy.rs` timestamp only): `cargo build -p postretro --timings` rebuilt sim, netcode, and the binary in 4.67s.
- Post-split AI edit (`crates/ai/src/facing.rs` timestamp only): rebuilt only `postretro-ai` and the binary in 4.02s; sim and netcode were isolated as required.
- Post-split sim-proper edit (`crates/sim/src/impact_policy.rs` timestamp only): rebuilt sim, netcode, AI, and the binary in 6.73s.
- Post-split physics edit (`crates/physics/src/lib.rs` timestamp only): rebuilt physics, sim, netcode, AI, and the binary in 7.15s.
- Post-fix test identity audit: **pending coordinator capture**. Run `audit-test-identities.sh` from `evidence/README.md`, retain the baseline/post snapshots and comparison report, then record the actual post count and SHA-256 from `evidence/test-identities/comparison/summary.txt`. Do not reuse the earlier unretained 7,251-name observation as final proof.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 workspace build/test/dev-tools | Final `/preflight`, plus `cargo build -p postretro --features dev-tools` | historical pass — coordinator refresh pending after the new evidence tests |
| A2 AI dependent invariant and regenerated graph | Focused `layering_invariants_hold`; `cargo run -p xtask -- crate-graph --check`; inspect generated `context/lib/crate-graph.md` | passed — 1/1; generated graph current |
| A3 AI normal dependency tree | `cargo tree -p postretro-ai -e normal` plus forbidden-name grep | passed — direct sim/foundation/physics edges present; forbidden runtime/render names absent |
| A4 netcode excludes AI | `cargo tree -p postretro-netcode -e normal` plus `postretro-ai` negative grep | passed |
| A5 physics dependency and reverse-dependency trees | `cargo tree -p postretro-physics -e normal`; `cargo tree -i postretro-physics` with exact workspace dependent comparison | passed — reverse workspace set is sim, netcode, AI, binary |
| A6 rebuild isolation | Warm timestamp-only `cargo build -p postretro --timings` probes for one AI file and one physics file; compare compiled package sets | observed in local warm-cache spots — compiled sets matched the expected boundaries, but raw logs/timing pages were not retained; use `evidence/README.md` for an auditable repeat |
| A7 extracted paths absent from sim | Scripted negative `rg`/path-existence gate for the five named paths | passed — no matches |
| A8 test-name superset | Run `audit-test-identities.sh` per `evidence/README.md`; retain pre/post complete and ignored captures plus comparison outputs | pending coordinator capture — fill actual counts/checksums from `comparison/summary.txt`; require empty `missing.txt` and `ignored-status-changes.txt` |
| A9 M15 Phase 0 determinism | Run `test_tick_runner_nav_bridge_preserves_section_and_path_queries`, then the retained `simulate_tick_determinism_harness_matches_run_to_run_and_spawn_order` and full determinism filter with nonzero matched counts. The scenarios/assertions are retained; invocation is ported through the test-only duplicate-sim identity bridge. | pending coordinator rerun — prior 33/33 covered the retained scenarios, but the new direct bridge-fidelity proof has not run |
| A10 ORD-2/ORD-3 production-host ordering | Relocated `postretro-ai` focused tests using sim's concrete `AiHost` implementation under `test-support` | passed in AI 234/234 suite |
| A11 ORD-1 registry-exhaustion refusal | Relocated focused log-capture test through the concrete sim host: no cooldown, spawn, or event and exactly one warning | passed in AI 234/234 suite |
| A12 ORD-4 sentiment borrow/order | Direct `sim_tick_decays_sentiment_before_same_tick_ai_target_selection` and `faction_sentiment_backstab_brawl_decay_reaction_persistence_and_fifo` tests through the production tick/host composition: decay visible before AI, effect write borrow-safe, next-tick visibility | pending coordinator run — tests added after the recorded AI suite |
| A13 `Send + Sync` assertions | Compile-time assertion tests for `postretro_sim::nav::NavGraph` and `postretro_physics::collision::CollisionWorld` | passed — 1/1 each |
| A14 MT design note and unchanged unsafe count | Grep the committed note path; repeat the exact 17-match unsafe command and diff normalized results, allowing only path relocation of the approved probe | passed — routed note present; 17 matches unchanged |
| A15 every visibility widening has a cross-crate consumer | Diff-derived list of `pub(crate)` → `pub`, checked by a scripted cross-crate reference scan and manual review | passed — unused nav widenings reverted; every remaining owner API has a sim, AI, or binary consumer |
| A16 warm timing report | Record pre/post AI and sim-proper timing totals and compiled package sets in this plan | reported — local warm-cache spot measurements only; no raw timing artifacts were retained, so use `evidence/README.md` for an auditable repeat |
| M1 campaign presentation plus AI behavior | Owner runs `campaign-test.prl` for load/play/presentation and `movement-feel.prl` for descriptor-backed AI/faction behavior | manual-blocking |
| M2 locomotion/rest animation including no-locomotion edge | Owner observes movement-feel animations; direct `no_locomotion_graph_restores_authored_playback_rate_through_sim_tick` supplies the literal no-locomotion proof | manual-blocking; direct test pending coordinator run |
| M3 concurrent-attack visual smoke | Owner observes concurrent attacks in-engine; `impact_time_faction_write_reaches_all_brains_on_the_next_tick` supplies exact same-fixed-tick dual-resolution proof | manual-blocking; automated proof retained |
| M4 dev-tools launch and AI debug | Owner, `--features dev-tools` launch and chase/brain debug interaction | manual-blocking |
| M5 co-op join/reconcile/level-change/remote animation | Owner, two-session co-op runbook | manual-blocking |
| M6 frame-time no regression | Owner, pinned `campaign-test.prl` before/after capture using the same machine/settings | manual-blocking |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Introduce sim's synchronous effects-only `AiHost` trait and concrete production host in place; route AI outcome effects through it and prove ORD-1–4 with focused existing tests before moving files | integrating executor | — | done — host seam compiled; exhaustion 1/1, same-batch 6/6, faction-write 1/1, sentiment harness 1/1 passed |
| 2 | Extract `collision`, `movement`, and `kinematic_mover` wholesale into `postretro-physics`; preserve test-only alloc/death seams behind `test-support`; update direct consumers and run focused physics/sim tests | integrating executor | 1 | done — physics 197/197, sim mover commands 14/14, cross-crate auto-close 1/1; physics/sim/netcode/binary checks and normal forward/reverse trees passed |
| 3 | Sink locomotion/rest graph queries and faction/tolerance field constants to foundation; update sim and netcode consumers with focused graph/client-apply tests | integrating executor | 2 | done — sim locomotion 12/12 and netcode remote-walk application 2/2 passed; foundation/sim/netcode checks clean |
| 4 | Extract the AI module and full `ai_tests.rs` into `postretro-ai`; invert both sim tick entry points behind the injected closure; update binary/dev-tools and every sim/netcode test harness without adding a production netcode→AI edge | integrating executor | 1, 2, 3 | done — AI 234/234, sim determinism 33/33, netcode migrated harnesses 4/4 + 6/6 + 2/2; sim/netcode/dev-tools test checks clean; normal sim/netcode trees exclude AI |
| 5 | Add `Send + Sync` guards, the MT-readiness note, layering/graph gates, dependency-tree checks, path/unsafe/public-widening audits, test-set comparison, and post-split timing probes | integrating executor | 4 | in progress — graph/trees/paths/unsafe/public API audits completed; durable identity/ignored-status audit awaits coordinator capture; local timing spot measurements are recorded with repeat instructions |
| 6 | Run review-readiness checks, `/review-panel` → `/fix-review-findings` focused retest loops, then `/preflight` once; fill the landing table and publish the blocking manual runbook as `test-ready` | integrating executor | 5 | in progress — prior review/preflight was green; coordinator must run the new direct proofs and identity audit, refresh evidence, then restore `test-ready` |
| 7 | After owner supplies every blocking manual result, update durable context, move the brief to `done/`, and commit the landing state | integrating executor | 6, owner proof | pending |

## Manual proof policy

All six manual rows are treated as blocking because the brief does not authorize landing first. After automated proof and review pass, set `status: test-ready` and stop with exact run commands, fixtures, expected observations, and a place for the owner to report each result. No manual row becomes an inferred pass.

## Landing evidence

Captured on 2026-09-14 after the earlier review/fix loop. This table is historical until the pending identity audit and newly added direct proofs are run.

| Gate | Command or evidence | Result |
|---|---|---|
| Review panel | architecture, correctness, API-boundary, test-quality, and final hygiene lenses | passed — final reviewers reported no findings |
| Workspace build | `cargo build --workspace` | passed |
| Dev-tools build | `cargo build -p postretro --features dev-tools` | passed |
| Formatting | `cargo fmt --check` | passed |
| Lints | `cargo clippy --target-dir target/preflight-clippy -- -D warnings` | passed |
| Full tests | `cargo test` | historical pass; coordinator refresh pending after the new evidence tests |
| Layering | `cargo test -p xtask layering_invariants_hold`; `cargo run -p xtask -- crate-graph --check` | passed; generated graph current |
| Focused behavior | physics 200/200; AI 234/234; sim determinism 33/33; netcode migrated harnesses 4/4 + 6/6 + 2/2 | historical pass; direct A9/A12/M2 proof reruns pending |
| Test identity | `audit-test-identities.sh` complete/ignored pre/post capture and comparison | pending — record actual counts/checksums from retained `evidence/test-identities/comparison/summary.txt` |
| Structural audits | normal dependency trees, reverse physics tree, removed-path grep, visibility consumers, unsafe count | passed — dependency contracts match A2–A7/A15; unsafe remains 17 textual matches |
| Compile isolation | local warm-cache timestamp-only spot probes recorded in Baselines | reported — package boundaries matched; raw timing artifacts were not retained |

The formal preflight was run once after the complete review-fix set. A scoped `cargo clean` was needed first because the workspace `target/` cache had reached 71 GB and filled the volume; it removed only recoverable build artifacts.

## Owner manual runbook

Record each outcome as `M<n> pass|fail — notes`. Keep resolution, graphics settings, camera route, and machine load fixed for before/after performance comparisons. The `.prl` files exist in this checkout; if either AI fixture is stale, rebuild it first:

```sh
cargo run -p postretro-level-compiler -- content/dev/maps/movement-feel.map -o content/dev/maps/movement-feel.prl
cargo run -p postretro-level-compiler -- content/dev/maps/combat-demo.map -o content/dev/maps/combat-demo.prl
```

### M1 — campaign presentation plus movement-feel AI behavior

Run the acceptance command:

```sh
cargo run -p xtask -- run content/dev/maps/campaign-test.prl
```

Confirm the map loads, plays, and presents as before the split. Then run the AI fixture that actually contains the named actors:

```sh
cargo run -p xtask -- run content/dev/maps/movement-feel.prl
```

In the arena, confirm the four `reference_enemy` actors and `limitator` acquire the player, face and pursue correctly, enter their authored attacks, and return to their normal state after losing a target. Use the three-enemy `crossfire_raider` cluster to confirm independent target changes and over-tolerance retaliation still occur.

Fixture note: the current `campaign-test.map` has no descriptor-backed AI placement, so it cannot by itself exercise the enemy/faction clauses written into M1. Report the campaign result and the `movement-feel` AI result together; do not infer one from the other.

### M2 — locomotion, rest, and the no-locomotion edge

Use the same `movement-feel.prl` run. Watch `reference_enemy` and `limitator` transition between their moving and stationary activities: locomotion clips must play while moving and idle/attack/rest clips must return to their authored rate when movement ends. The nearby `pose_fixture_enemy` should continue sampling its authored `Rest` clip without speed drift while its brain changes activities.

The shipped visual fixtures all have a discoverable chase or position-goal activity, so there is no honest visual fixture for the literal `locomotion_animation(graph) == None` half of this row. The direct `no_locomotion_graph_restores_authored_playback_rate_through_sim_tick` test covers that edge through the real sim tick. The coordinator must run and record that test before the owner combines its result with the visual authored-rate observation. Mark M2 failed if either proof fails; a dedicated visual no-locomotion fixture is not required.

### M3 — concurrent-attack visual smoke

In `movement-feel.prl`, remain inside the arena wave until at least two enemies complete attacks in the same combat interval. This is a visual concurrency smoke test: confirm neither actor shows a dropped impact or stuck attack/cooldown state. Exact same-fixed-tick dual resolution is automated by `impact_time_faction_write_reaches_all_brains_on_the_next_tick`, which asserts two attack events from one tick. Record any visible dropped hit or stuck attacker as a manual failure; do not claim exact tick alignment from observation alone.

### M4 — dev-tools AI diagnostics

```sh
cargo run -p xtask -- run --features dev-tools -- content/dev/maps/movement-feel.prl
```

Press `Alt+Shift+A` for the all-agent overlay and confirm paths, velocity/destination markers, and recursive brain-state labels update on the arena enemies. Press `Alt+Shift+N` for navmesh geometry. Press `Alt+Shift+G` once to spawn the chase agent, move around, and confirm it continuously retargets; press it again and confirm the existing agent is reused rather than duplicated. `Alt+Shift+Backquote` opens the debug panel.

### M5 — co-op authority, reconciliation, level change, and remote enemy animation

Use two terminals; the default host port is 27015:

```sh
# Terminal 1
RUST_LOG=info cargo run -p xtask -- run --features dev-tools -- content/dev/maps/combat-demo.prl --host

# Terminal 2
RUST_LOG=info cargo run -p xtask -- run --features dev-tools -- content/dev/maps/combat-demo.prl --connect 127.0.0.1:27015
```

On the client, confirm one local pawn, immediate local movement, a smoothly interpolated remote pawn, and exactly one host-authoritative instance of each remote enemy. Confirm the remote enemy's locomotion state animates while it moves. On the host press `Alt+Shift+L`; confirm the client follows the catalog-backed level change, remains joined, receives the new baseline without duplicate actors, and continues reconciling movement. If testing disconnect/rejoin too, confirm only one local pawn is restored.

### M6 — frame-time comparison

Compare the pre-split checkpoint `ffff98fe` with the post-fix candidate using separate clean worktrees or an existing pre-split capture. In both builds, use the same release-mode command, window resolution, graphics settings, camera position/route, and sampling duration:

```sh
POSTRETRO_GPU_TIMING=1 RUST_LOG=info cargo run -p xtask -- run --release -- content/dev/maps/campaign-test.prl
```

After shader/build warm-up, record a stable frame-time sample and the per-pass GPU timings when the adapter supports timestamp queries. Report before/after median and tail behavior (or the engine's stable displayed/logged values) and mark pass only if the post-split build shows no runtime regression rather than merely a faster compile.

Retain the exact revision, machine/toolchain metadata, commands, settings, route, duration, and raw logs under `evidence/runtime/`. The prior warm compile spot measurements do not substitute for this runtime capture.

## Manual result ledger

| Result | Owner evidence |
|---|---|
| M1 | pending |
| M2 | pending |
| M3 | pending |
| M4 | pending |
| M5 | pending |
| M6 | pending |
