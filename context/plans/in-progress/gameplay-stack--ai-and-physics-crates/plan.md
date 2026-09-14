# gameplay-stack--ai-and-physics-crates — plan of record

mode: resumable
status: approved
read at: 630c5554b

## Corrections

- The brief was grounded at `66e5caa`; current source is `630c5554b`. The only production seam change in cited source is the already-promoted M3 context wording; the tick edge, AI effects, physics subtree, graph helpers, and state constants still match the Decisions.
- Research says `EntityTypeDescriptor` is scripting-core-owned/re-exported; it now lives in `postretro-entities`. Planning around this by importing the type from entities in the new AI crate while leaving VM-coupled descriptor parsing in scripting-core.
- Research names two netcode test-only `AiRuntime` constructions; `ingest_hit_harness_test.rs` now also drives `simulate_tick_with_presentation_aim` and `AiRuntime` in its expanded cross-stage harness. Planning around this by migrating every netcode test-only tick caller to the injected seam and adding a dev-dependency on `postretro-ai`; production netcode remains AI-free.
- Research flags `alloc_probe` as a test-only boundary widening; current sim already exposes it under `#[cfg(any(test, feature = "test-support"))]`. The physics and AI crates will reuse that existing test-support seam rather than widen it again.
- The brief Path suggests extracting physics first. The build workflow requires the first task to test the riskiest premise through a thin slice, so the synchronous production `AiHost` seam and ORD ordering proofs land in place before the mechanical physics move. This changes task order, not any Decision or Acceptance meaning.
- Research treats `kinematic_mover` as depending only on collision and movement, but live `kinematic_mover/commands.rs` also owns production scripting-core registrar wiring. Planning around this by moving the deterministic command applier into physics while retaining the VM-coupled registrar/target wiring in sim as `mover_commands`; this preserves the accepted physics dependency tree and the scripting handler-placement rule.
- Compiling `postretro-sim`'s unit-test target after the AI move exposed Cargo's expected dev-cycle identity split: the test target's local `postretro-sim` types are distinct from the normal `postretro-sim` instance below `postretro-ai`. A `test-support`-only runner adapter reconstructs only the immutable `NavGraph` value across those identities; the production runner remains zero-copy, navigation stays high as the research decision requires, and neither sim nor netcode gains a normal AI edge.

## Delegated answers

- `data_archetype::find_descriptor` placement — keep the existing public helper in `postretro-sim` and import it from `postretro-ai`. AI already has an intentional down-edge to sim, the lookup operates on the sim runtime's descriptor slice, and moving it lower would add domain policy to the engine-data floor without reducing the dependency graph.

## Baselines

- Pre-split workspace test-name set: `baseline-test-names.txt`, 7,246 unique names, SHA-256 `5558dfc222033782254fb915d70ccd6f26234d0c78c216c510689ad63ee4210a`.
- Pre-split textual Rust `unsafe` count: 17 `rg -n '\bunsafe\b' crates --glob '*.rs'` matches. The only actual unsafe block/impl is the previously approved `crates/sim/src/alloc_probe.rs`; the remaining matches are prose or test strings.
- Warm AI edit (`scripting_systems/ai/mod.rs` timestamp only): `cargo build -p postretro --timings` rebuilt sim, netcode, and the binary in 5.65s.
- Warm sim-proper edit (`impact_policy.rs` timestamp only): `cargo build -p postretro --timings` rebuilt sim, netcode, and the binary in 4.67s.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 workspace build/test/dev-tools | Final `/preflight`, plus `cargo build -p postretro --features dev-tools` | achievable as stated |
| A2 AI dependent invariant and regenerated graph | Focused `layering_invariants_hold`; `cargo run -p xtask -- crate-graph --check`; inspect generated `context/lib/crate-graph.md` | achievable as stated |
| A3 AI normal dependency tree | `cargo tree -p postretro-ai -e normal` plus forbidden-name grep | achievable as stated |
| A4 netcode excludes AI | `cargo tree -p postretro-netcode -e normal` plus `postretro-ai` negative grep | achievable as stated |
| A5 physics dependency and reverse-dependency trees | `cargo tree -p postretro-physics -e normal`; `cargo tree -i postretro-physics` with exact workspace dependent comparison | achievable as stated |
| A6 rebuild isolation | Warm timestamp-only `cargo build -p postretro --timings` probes for one AI file and one physics file; compare compiled package sets | achievable as stated |
| A7 extracted paths absent from sim | Scripted negative `rg`/path-existence gate for the five named paths | achievable as stated |
| A8 test-name superset | Capture post-split `cargo test --workspace -- --list`, normalize to unique names, and `comm -23 baseline post` expecting empty output; compare ignored markers separately | achievable as stated |
| A9 M15 Phase 0 determinism | Focused unchanged determinism harness target/filter with a nonzero matched-test count | achievable as stated |
| A10 ORD-2/ORD-3 production-host ordering | Relocated `postretro-ai` focused tests using sim's concrete `AiHost` implementation under `test-support` | achievable as stated |
| A11 ORD-1 registry-exhaustion refusal | Relocated focused log-capture test through the concrete sim host: no cooldown, spawn, or event and exactly one warning | achievable as stated |
| A12 ORD-4 sentiment borrow/order | Relocated focused test through the concrete sim host: decay visible, effect write borrow-safe, next-tick visibility | achievable as stated |
| A13 `Send + Sync` assertions | Compile-time assertion tests for `postretro_sim::nav::NavGraph` and `postretro_physics::collision::CollisionWorld` | achievable as stated |
| A14 MT design note and unchanged unsafe count | Grep the committed note path; repeat the exact 17-match unsafe command and diff normalized results, allowing only path relocation of the approved probe | achievable as stated |
| A15 every visibility widening has a cross-crate consumer | Diff-derived list of `pub(crate)` → `pub`, checked by a scripted cross-crate reference scan and manual review | achievable as stated |
| A16 warm timing report | Record pre/post AI and sim-proper timing totals and compiled package sets in this plan | achievable as stated |
| M1 campaign behavior | Owner, in-engine runbook on `campaign-test.prl` | manual-blocking |
| M2 locomotion/rest animation including no-locomotion edge | Owner, in-engine observation with named fixtures/graph | manual-blocking |
| M3 two same-tick attackers | Owner, in-engine observation; automated same-batch tests provide supporting proof | manual-blocking |
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
| 5 | Add `Send + Sync` guards, the MT-readiness note, layering/graph gates, dependency-tree checks, path/unsafe/public-widening audits, test-set comparison, and post-split timing probes | integrating executor | 4 | pending |
| 6 | Run review-readiness checks, `/review-panel` → `/fix-review-findings` focused retest loops, then `/preflight` once; fill the landing table and publish the blocking manual runbook as `test-ready` | integrating executor | 5 | pending |
| 7 | After owner supplies every blocking manual result, update durable context, move the brief to `done/`, and commit the landing state | integrating executor | 6, owner proof | pending |

## Manual proof policy

All six manual rows are treated as blocking because the brief does not authorize landing first. After automated proof and review pass, set `status: test-ready` and stop with exact run commands, fixtures, expected observations, and a place for the owner to report each result. No manual row becomes an inferred pass.
