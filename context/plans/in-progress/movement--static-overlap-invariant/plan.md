# movement--static-overlap-invariant — plan of record

mode: compact
status: blocked
read at: fd8982c

## Block — Acceptance row B1 needs restatement

B1 reads: "Ramps, stairs at step height, step-up onto a ledge, sliding along a wall, and
running across a coplanar seam keep the displacement recorded at b0a5c0f, within 1 mm
per tick."

At b0a5c0f the stair climb and the single step-up are not clear of static geometry, so
no pose that satisfies the invariant can match their recorded displacement on every tick.

Evidence: task 1 fixtures on unmodified source, canonical test capsule (r 0.4, half
height 0.8, step 0.3), walking +X at 7 m/s, 1 m treads.
- **Stairs at 0.3 m rise:** every step embeds the pawn in the step corner: 7.0 cm,
  3.4 cm, 5.6 cm on the three steps reached in 40 ticks.
- **Single 0.3 m step-up:** 7.0 cm on its approach tick.
- **Cause:** step-up fires while the riser is still up to 0.7 m ahead, because its probe
  distance is `max(v·dt, step + radius)`. The lifted pawn advances one tick's travel
  without reaching the riser. Ground-stick's swept down-cast then meets the step's top
  edge with a steep normal, and the centre-ray fallback drops the pawn 0.35 m straight
  into the corner. This is research bypass 2, reached by ordinary stairs.
- **Clean at b0a5c0f, max tolerance 0.5 mm:** the ramp, wall slide and seam run.

Prototype of the fix: a swept move cast through the skin dispatcher at a 0.5 mm target
distance, replacing the free advance and the ground-stick drop.
- **Ramp, wall slide, seam:** match b0a5c0f exactly, 0.0 mm per tick.
- **Stairs and step-up, horizontal:** within 0.25 mm per tick.
- **Stairs and step-up, end pose:** within 0.6 mm.
- **Stairs and step-up, vertical:** differs only on the embed ticks, by up to 13.6 cm,
  because the pawn stays on the step edge instead of sinking into it.

Proposed wording for B1:

> Ramps, stairs at step height, step-up onto a ledge, sliding along a wall, and running
> across a coplanar seam keep the displacement recorded at b0a5c0f, within 1 mm per
> tick. The baseline is a committed test that runs before the fix. A seam slide that
> loses free-advance distance is noted in the plan of record. On a tick whose b0a5c0f
> pose itself overlapped static geometry (at b0a5c0f, ground-stick's centre-ray drop
> sinks a pawn approaching a stair riser up to 7 cm into the step corner), only the
> horizontal displacement is held to 1 mm, and the climb still ends within 1 mm of the
> b0a5c0f end pose.

The alternatives are to drop stairs and step-up from B1, or to keep B1 and accept a
different fix. The second would need to reproduce the b0a5c0f embed and so contradicts
the invariant. I recommend the wording above.

### Other task-1 findings (no restatement needed)
- **A1 lip fixture:** a 0.3 m chamfered lip at 25° embeds a pawn walking onto it by
  3.7 cm (free advance). At the ramp foot of a 2 m ramp the embed grows every tick, to
  22 cm. A bare 90° lip edge never leaves the skin band. Its tilted contact is only
  in-band, so the A1 fixture uses the chamfer.
- **Free advance on a steady ramp:** the advance stays inside the 20 mm skin band (about
  8 mm horizontal per tick on 20°). A skin-target sweep would cut it and lose about
  9.6 mm on a tick; a tangent sweep gains about 10 mm per tick. The 0.5 mm-target sweep
  keeps it exactly, so that is the conversion shape.
- **Bypass reproductions at b0a5c0f:**
  - Ground-stick fallback over a 0.35 m drop: 9.5 cm.
  - Unswept step-up rise under a ceiling 0.3 m over the standing head: 6.4 cm. The pawn
    also never climbs a step whose top has room for it (B2's case).
  - Floor push into a ceiling 19 mm over the resting head, step height 0: 1.1 mm.
  - Stuck-stop rollback at the default 1 mm threshold sinks at most 0.7 mm, all inside
    the skin band. The A7 fixture needs a larger descriptor `stuck_stop_threshold`, or
    gets named in its test with that reason.
- **Pre-existing, outside this brief:** stairs whose tread is shorter than
  `step_height + radius` (0.7 m for the canonical capsule) never step up at walk speed.
  The lifted forward probe hits the next riser, so the pawn creeps along the first riser
  edge at about 2 mm per tick.

On resume: rebuild the task-1 fixtures from the geometry above (the session's
exploration was not committed), then continue at task 1 with the owner's B1 wording.

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
| B1 displacement baseline within 1 mm/tick | `overlap_baseline::*` committed before the fix | **needs restatement** (see Block) |
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
| 1 | Riskiest premise first: test-only overlap helper, the lip-edge fixture, the displacement baseline and every per-bypass fixture, run on pre-fix source; commit baseline + lip expected-failure | integrating executor | — | explored; blocked on B1 |
| 2 | Behaviour-preserving: swept-move helper and shared step-up / snap-to-floor primitives in a sibling module; player substrate and agent controller call them; agent copies deleted | integrating executor | 1 | |
| 3 | Overlap check: tolerance query, armed assertion, build/env gate, report with texture + TrenchBroom position, warn-once on `CollisionWorld`, caller logging (host, prediction, agents), timing stage | integrating executor | 2 | |
| 4 | Convert every unswept move at its source; flip lip fixture | integrating executor | 3 | |
| 5 | Resize growth clearance: anchored stand-up, growing entry | integrating executor | 3 | |
| 6 | Remaining AC fixtures, grep gates, determinism replay | integrating executor | 4, 5 | |
| 7 | Measurement on `stress-env-volumes` | integrating executor | 3 | |
| 8 | Preflight, review panel, fixes | integrating executor | 6, 7 | |
