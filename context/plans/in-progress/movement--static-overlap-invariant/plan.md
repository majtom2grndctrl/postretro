# movement--static-overlap-invariant — plan of record

mode: compact
status: active
read at: fd8982c

No crate source changed between the brief's `read at` (b0a5c0f) and fd8982c, so the
grounded Decision reads stand. Source re-read this session: `movement/substrate.rs`,
`movement/mod.rs` (tick), `movement/teleport.rs`, `movement/dispatch.rs`,
`intents/{crouching,normal,slide}.rs`, `swim_transition.rs`, `collision/mod.rs`,
`collision/moving.rs`, `sim/src/agent/mod.rs`, `sim/src/agent_steering.rs`,
`sim/src/sim/host_movement.rs`, `netcode/src/prediction.rs`, `sim/src/sim/cpu_stages.rs`.

## Corrections
- Toolchain: the workspace requires rustc 1.99; the container shipped 1.97 → `rustup update stable` (1.99.1). No source impact.
- "Timing stage precedent: the `Environment` CPU stage" → that stage is fed by an
  `EnvironmentQueryTimer` attached to the `EnvironmentView`. The overlap check is not an
  environment query, so it gets its own accumulator on `CollisionWorld` (atomic, enabled
  per tick by `simulate_tick` only while timing is on) feeding a new `SimStage::OverlapCheck`
  under `Tick`, spanning `Movement` and `SteerAgents` exactly as `Environment` does.
- "the movement code returns an overlap report" → the player tick returns it on
  `MovementEvents` (a tick-local, never-serialized struct; no wire field), the agent
  controller on its `SlideResult`.

## Delegated answers
- The check's variable name — `POSTRETRO_OVERLAP_CHECK`: `1` turns the check on in any
  build, `0` turns it off in any build, unset keeps the build default (on with debug
  assertions, off in release). Matches the `POSTRETRO_*_TIMING` naming of the other
  diagnostic switches; the `0` value exists so the measured AC can compare set/unset in
  one build.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 lip-edge reproduces first | `overlap_fixtures::a_pawn_pressed_along_a_tilted_convex_lip_edge_ends_clear` (committed `should_panic` first, flipped by the fix) | achievable as stated |
| A2 per-bypass fixtures overlap at b0a5c0f | each fixture run on pre-fix source; results table below | achievable as stated |
| A3 vault fixtures | `overlap_fixtures::vault_*` | achievable as stated |
| A4 step 0 floor push / ground-stick under ceiling (P7) | `overlap_fixtures::floor_push_*` | achievable as stated |
| A5 step with too little headroom | pawn + agent fixtures | achievable as stated |
| A6 ground-stick beside wall, pawn + agent | pawn + agent fixtures | achievable as stated |
| A7 corner wedge stuck-stop rollback | `overlap_fixtures::stuck_stop_*` | achievable as stated |
| A8 two bypasses one tick (P5) | fixture | achievable as stated |
| A9 step-up then floor push under overhang (P4) | fixture | achievable as stated |
| A10 still pawn over ledge edge (P7) | fixture | achievable as stated |
| A11 idle agent over ledge edge (P6) | sim agent fixture | achievable as stated |
| A12 rider walks into lip (P2) | fixture | achievable as stated |
| A13 mover push toward wall (P1) | fixture | achievable as stated |
| A14 check resumes after mover embed (P8) | fixture | achievable as stated |
| A15 warning text, once per load, reload re-arms | sim log-capture test through the test-only hook | achievable as stated |
| B1 displacement baseline within 1 mm/tick | `overlap_baseline::*` committed before the fix | achievable as stated |
| B2 headroom for capsule but not lift margin still steps up | pawn + agent fixtures | achievable as stated |
| B3 step height zero | pawn + agent fixtures | achievable as stated |
| B4 teleport arrival beside wall lands grounded | teleport test | achievable as stated |
| B5 pawn steps onto/snaps to mover top; agent ignores | fixture pair | achievable as stated |
| B6 controllers keep call rules; slope by own walkable | fixtures | achievable as stated |
| B7 carried into ceiling behaves as before; no assert | existing mover tests + fixture | achievable as stated |
| B8 existing suites pass armed | `cargo test` physics/sim/netcode/ai/postretro | achievable as stated |
| C1 predictor replay reproduces host poses exactly | netcode test | achievable as stated |
| D1 grep gate: no unswept position writes | source-scan test | achievable as stated |
| D2 stand-up / slide exit under low ceiling | fixtures | achievable as stated |
| D3 airborne stand-up above floor (P11) | fixture | achievable as stated |
| D4 larger crouch capsule entry under low ceiling | fixture | achievable as stated |
| D5 grep gate: agent holds no step-up/snap logic | source-scan test | achievable as stated |
| D6 default off in release, on with debug assertions, variable on in release | unit test of the gate resolver + release-build check | achievable as stated |
| D7 diff gate: no wire / descriptor field | `git diff` over `crates/net`, wire convert, foundation descriptor | achievable as stated |
| D8 measured cost on `stress-env-volumes` | `POSTRETRO_CPU_TIMING` runs, set/unset | achievable as stated (needs a runnable engine in the container) |
| M1–M4 | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Riskiest premise first: test-only overlap helper, the lip-edge fixture, the displacement baseline and every per-bypass fixture, run on pre-fix source; commit baseline + lip expected-failure | integrating executor | — | |
| 2 | Behaviour-preserving: swept-move helper and shared step-up / snap-to-floor primitives in a sibling module; player substrate and agent controller call them; agent copies deleted | integrating executor | 1 | |
| 3 | Overlap check: tolerance query, armed assertion, build/env gate, report with texture + TrenchBroom position, warn-once on `CollisionWorld`, caller logging (host, prediction, agents), timing stage | integrating executor | 2 | |
| 4 | Convert every unswept move at its source; flip lip fixture | integrating executor | 3 | |
| 5 | Resize growth clearance: anchored stand-up, growing entry | integrating executor | 3 | |
| 6 | Remaining AC fixtures, grep gates, determinism replay | integrating executor | 4, 5 | |
| 7 | Measurement on `stress-env-volumes` | integrating executor | 3 | |
| 8 | Preflight, review panel, fixes | integrating executor | 6, 7 | |
