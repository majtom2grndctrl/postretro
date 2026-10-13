# movement--vault — plan of record

mode: resumable
status: blocked
read at: 27ca6a60
branch: claude/sweet-cerf-m0t396

The claim was pushed to `main` as `27ca6a60`. Every source symbol cited by Decisions, Path and `research.md` was re-read at that commit. The only changes since `a307e3f1` under physics, foundation, net and netcode are the Quake jump and gravity defaults (`21bce72f`, `728ec83e`). Decisions and Acceptance remain the brief's contract. The `A`/`R`/`T` labels below are planning handles, not reworded acceptance.

## Blocked on the owner

Two items stopped the plan before the owner's skim; B2 is resolved. Once B1 is resolved, status becomes `proposed` and the skim proceeds as normal for a resumable plan.

### B1. False Decision premise: the `from` mapping onto the composition draft

The Decision *Trigger per entry context* says that `from` "maps one-to-one onto the composition draft's `states.<s>.vault`." Here is what `context/plans/drafts/player-descriptor-composition/index.md` actually says:

- **`vault` is a state.** Its own key is `states.vault`, beside `ground`, `air` and `crouch` (§1, line 22: "`slide`, `wallRun`, `vault` as they land"). It is never a per-state child.
- **Per-state children are modifiers only.** The `states.<s>.X` child pattern belongs to modifiers (dash, §2). The draft's own working rule says "a state owns the tick's velocity intent; a modifier is a time-bounded overlay" (Open questions). Under that rule, a vault child block under each state would be a modifier, contrary to this brief's "native `Vault` state".
- **No `swim` state.** The draft's state vocabulary has no `swim`, so `from.swim` has nothing to map onto.

The flat `movement.vault.from` this brief builds does not depend on the sentence. Only the future migration does. Options:

1. **Drop the mapping sentence (recommended).** `from` stands as specified. The composition draft decides the migrated shape when it absorbs vault and swim; one plausible shape is `states.vault.from`.
2. **Amend the draft now.** Add `states.swim` and per-state `vault` entry children to the composition draft, and record why vault entry is a per-state child while vault itself is a state.

### B2. Resolved: the "Luau twin" acceptance row is restated

`player.ts` has no Luau twin. The owner took the fixture-pair wording, which follows the `fluids-swim` precedent (`fluids_swim_twins_run_as_mod_manifests_and_declare_the_same_surface`, plus the test pinning the dev `player.ts` block equal to the twin's). The brief's row D2 now reads it.

## Corrections

- **Path wording of a Decision:** the brief says "`dispatch_state_intent`". It lives in `crates/physics/src/movement/dispatch.rs:187`, not `mod.rs`. `outgoing_boost` (`dispatch.rs:64`) is an exhaustive match and needs a `Vault` arm.
- **`research.md`:** "`MovementState::Sliding` carries `floor_normal`." Only `WireMovementState::Sliding` carries it (`net/src/wire/components.rs:74`). It is filled from `PlayerMovementComponent.last_floor_normal`. The precedent still holds for the wire.
- **"Death interrupts" / "as death does":** physics movement has no death handling. `run_host_movement_tick` (`sim/src/sim/host_movement.rs:49`) ticks every pawn, alive or not, and player pawns never despawn. Planning around it with a shared sim helper, `drop_vault_if_dead(registry, pawn)`. It runs before the movement tick on the host (`sim/mod.rs:580`) and in `client_predict_movement_tick`, and leaves `Vault` for airborne `Normal`. Death is host-authoritative, so a client that predicts past its own death takes one correction. That correction is inside the vault tolerance.
- **Inputs to the pre-intent stage:** `apply_environment_transition` (`dispatch.rs:114`) receives `input` (raw `jump_pressed`, `wish_dir`) and the component, but not `jump_edges` or `dt`. "Held" reads `input.jump_pressed`. "Buffered" reads `component.jump_buffer_timer_ms > 0`. The probe zeroes the buffer and sets `jump_spent` itself, because `derive_jump_edges` only fires a buffered jump when grounded (`substrate.rs:785`). `dt` is threaded in.
- **Buffer re-arm hazard:** the buffer-arm step (`mod.rs:321-329`) excludes only `Swim`. A held jump during a vault would re-arm it. `Vault` joins the exclusion.
- **Typedef location:** SDK type declarations live in `crates/sim/src/scripting/primitives/mod.rs` (`PlayerMovementDescriptor` :740, `SwimParams` :870, `ImpulseStates` :937). New type names are also registered in `scripting-core/src/typedef/common.rs` (TS ~:188, Luau ~:365). `"auto" | "jump"` uses `register_enum`, as `ResolutionMode` does. The committed `sdk/types/postretro.d.{ts,luau}` regenerate with `cargo run -p postretro-sim --bin gen-script-types`. The fixtures `tests/fixtures/expected.d.{ts,luau}` are hand-updated.
- **Impulse springs:** `[ImpulseSpring; 4]` (`postretro/src/view_feel.rs:48,61,71,200`) grows to 5. `state_index` maps `Swim => None` and keeps doing so. Both parsers' closed key lists and their message ("expected normal, dash, crouch, or slide") gain `vault`.
- **Scripting surface example:** the brief shows `fluidJumpVelocity: 6.5`. The dev value has been 9.35 since the Quake defaults landed. The example keeps 9.35; the field's optionality is unchanged.
- **Descriptor drop policy:** dash, crouch and slide hard-error on a bad block; only swim warns and drops. Vault follows swim, per the Decision.
- **Event count:** `docs/scripting-reference.md:657` says "24 bare addresses". It becomes 26.
- **Cast counter:** physics has no instrumentation that counts casts. A `#[cfg(test)]` thread-local counter on the probe's own cast wrapper proves the probe-cost row. Production builds carry nothing.
- **Harness gaps:**
  - The predict/reconcile harness has no ack setter. Tests select reconcile windows after the fact with `windows_spanning` (`predict_reconcile_harness_swim.rs:224`). "Ack on the entry tick" is proven by choosing the window whose acked tick is the entry tick and asserting that one exists.
  - There is no rising-mover fixture. A vertical variant of `with_moving_platform` is added.
- **Replay and despawned movers:** `MoverHistoryBuffer` is never pruned on despawn (`client.rs:592,638`). Replay can return a pose for a mover the host has despawned. The host aborts the vault; the client converges on the next snapshot. That divergence counts as a moving-mover vault and is held to the vault tolerance.
- **Vault on tuning rebuild:** `carry_live_tick_state` has no `Vault` analogue of `leave_swim_without_swim_block`. Planning around it with `leave_vault_without_vault_block()`, called beside the swim one, so a rebuilt component without a vault block drops to `Normal`.
- **Merge validation:** `merge_wire_into_movement_state_checked` validates only the ground mover id. A `Vault` mover id is validated through the same `ground_mover_known` predicate. An unknown id rejects the merge, as an unknown ground ref does.
- **Fire timing:** the client judges fire before it predicts the tick's movement (`frame_loop/mod.rs:730`). The host-remote `guard_initiation` also runs before `simulate_tick`. The host-local weapon stage runs after movement (`sim/mod.rs:855`). All three paths therefore judge at tick start, and host-local captures its predicate before `run_movement_tick`. See *Delegated answers* for what "exit tick" means under that rule.
- **Press triggers:** a fresh press refused by a gate is consumed (`sim/src/weapon/execution.rs:269-279`). A press made during a vault and held to exit would never fire. Planning around it by keeping, not consuming, a press refused by the pawn-state gate alone, so it fires on the first tick the gate opens if the trigger is still held. This is a clarification of the existing Decision ("a trigger held through the vault fires after exit without a re-press"), applied to Press triggers. Hold triggers already restart without a re-press after `cancel_activation()`, as they do after death.

## Delegated answers

- **Vault wire payload.** The payload is `WireMovementState::Vault { elapsed_ms: f32, target: [f32; 3], start_offset: [f32; 3], mover: Option<u32>, entry_speed: f32 }`:
  - `target` is the landing feet position: mover-local when `mover` is set, world otherwise.
  - `start_offset` is the entry feet position minus `target`, in the same frame.
  - The exit direction is derived as `-start_offset` projected horizontally and normalized, so it is not carried.
  - `durationMs` stays local tuning. Hot reload drops to `Normal` anyway.

  Replay needs nothing else: the path, the exit velocity and the mover resolve from these fields and the tick's mover pose. The contract rejects any of: non-finite floats; `elapsed_ms < 0`; `entry_speed < 0`; `start_offset.y > 0` (the target is never below the start); a horizontal `start_offset` shorter than 1 mm (no exit direction); and an unknown mover id at merge. The foundation `MovementState::Vault` mirrors these fields with `Vec3`. No outer `WirePlayerMovementState` field is added.
- **How the host judges a start at its stamped tick, with no new wire data.** A host-only `RetainedStart` field `fire_blocked: Option<bool>` records the verdict (`netcode/src/activation_edges.rs:28`; the struct is already capped at 64 per client).
  1. Before `guard_initiation`, `host_prepare_remote_pawn_commands` (`postretro/src/main.rs:5082`) computes the shooter's vault predicate for each resolved command from the live, pre-movement pawn. This is the pawn state at the start of that client tick.
  2. It stamps every retained start with `start_tick <=` that tick that has no verdict yet.
  3. `guard_initiation` judges the delivered token by its stamp, falling back to the live predicate for a start delivered on its own resolution. It mirrors `delivered_aim`.
  4. A blocked start is refused like a cadence refusal (`host_activations.rs:162-168`).

  The gate does not read remote `can_fire`. That reads host time and also gates due shots. Known edge: when a start's own tick never resolved on the host, because the trim reseated the cursor past it, the stamp uses the first resolution at or after it.
- **The rise-then-forward curve.**
  - **Progress.** Let `s = elapsed / duration`. Vertical progress is `v(s) = smoothstep(0, 0.6, s)`. Forward progress is `f(s) = smoothstep(0.6, 1, s)`.
  - **Path point.** `p(s) = T + R·(start_offset_vertical·(1 − v(s)) + start_offset_horizontal·(1 − f(s)))`, where `T` is the target resolved through this tick's mover pose (identity for static) and `R` is that pose's rotation.
  - **Rise first, then forward.** The pawn rises fully before it moves forward. That is the path the probe proved clear, so the capsule never clips the lip.
  - **Authored velocity.** Each tick sets velocity to `(p(s_next) − position) / dt`. No gravity is applied, and the substrate still resolves collision.
  - **Monotonic eye.** `v` is non-decreasing, and crouch never vaults, so eye height is constant. The rendered eye, the interpolated feet plus the eye height, therefore rises monotonically.
  - **Per-tick step bound.** The largest per-tick eye step is `max_k (v(s_{k+1}) − v(s_k))·|start_offset.y|`, which is the "path step" the AC compares against. On a rising mover the mover's own tick delta adds to it, and the row is stated for static ledges.
  - **Final tick.** On the tick where `elapsed + dt ≥ duration`, the pawn lands on `T` with a small downward settle. This produces substrate floor contact, so `hit_floor` runs the one landing-refresh point, and the intent transitions to grounded `Normal` with `exit_dir · entry_speed`.
- **Probe shape and fixed cast bound.** The probe runs only on eligible ticks (block present, context in `from`, nonzero horizontal wish, and jump held or buffered for a `"jump"` context). Every cast goes through the combined world, so it sees movers. A passing tick runs at most **4 casts**:
  1. **Forward capsule sweep** from the tick-start pose along the horizontal wish, over `radius + 0.1 m`. No hit, or a walkable hit, ends the probe after 1 cast.
  2. **Down ray** onto the top surface. It starts `radius + skin` past the contact, at the reach ceiling plus a margin: the feet plus `maxHeight` on land, the fluid surface plus `swimMaxHeight` in water. A non-walkable hit, or a height outside the reach, refuses.
  3. **Upward capsule clearance sweep** from the tick-start pose to the landing height plus skin.
  4. **Forward capsule clearance sweep** at that height to the landing point.

  Sweeps 3 and 4 run from a known-clear pose, so they prove both the path and standing-capsule clearance on the top. That covers the headroom row, since there is no combined intersection test. The swim surface height is `feet + immersion · segment` from the resolved environment, which needs no cast.
- **Step-up and vault share one boundary.** The lower reach bound is one shared function, `step_up_limit(step_height)`, used by both `step_up_lift` and the probe. A ledge at that height steps up; one just above it vaults. The bound gains its first test.
- **Exit tick.** All three fire paths judge the pawn's state at tick start, so one predicate serves all of them. A pawn blocks starts when its tick starts in `Vault`, `allowFire` is false, and the tick is not the vault's scheduled final tick (`elapsed + dt < duration`).

  The "exit tick" is therefore the tick whose movement leaves `Vault`, and a start stamped on it is accepted, as the AC requires. A start stamped on the entry tick also passes (its tick starts in `Normal`). Its first shot fires, and the entry cancel stops any later shot of a burst.
- **Entry cancel without new wire data.**
  - **Client:** on the first tick that starts in `Vault`, it calls `cancel_activation()`, `local_terminal` and `capture.terminal`, as `apply_owner_liveness` does for death.
  - **Host-remote:** when the host's movement for resolved tick E enters `Vault`, it inserts a synthetic cancel edge for the live token, stamped with client tick E+1. The existing ordering holds a cancel until the execution clock reaches its tick (`activation_edges.rs:250-306`), so shots stamped through E are minted and none after.
  - **Host-local:** the same entry cancels in-sim.
- **Coordination.** The integrating executor owns every task, seam, Cargo run, `plan.md` update and commit. Task 5 (descriptor surface) and the SDK half of Task 6 may go to a worker once Task 1's foundation types land: their files do not overlap physics or netcode. Each worker gets the full brief, `research.md`, this plan and the routed docs.

## Verified source and boundary consumers

| Contract | Current anchor |
|---|---|
| State enum, component, landing refresh | foundation `movement/player_movement.rs`: `MovementStateKind` :139, `MovementState` :148, `kind()` :201, `refresh_on_landing` :490, `leave_swim_without_swim_block` :516, `from_descriptor` :363 |
| Tick order and pre-intent stage | physics `movement/mod.rs::tick_with_immersion` :210–390; `dispatch.rs::{apply_environment_transition :114, dispatch_state_intent :187, outgoing_boost :64}`; `swim_transition.rs::{environment_transition :74, fluid_jump_clears :182}` |
| Substrate and queries | `substrate.rs::{step_up_lift :88, integrate_collision :157, clearance probes :613–700, derive_jump_edges :765, advance_forgiveness :803}`; `collision/moving.rs::{CombinedCastHit :38, CombinedCollisionWorld :100, MoverPoseSource::pose :71}`; `mover_carry.rs` |
| Events | `mod.rs::{state_entry_address :115, state_exit_address :125}`; `sim/src/emission.rs:38`; `docs/scripting-reference.md:668` |
| Wire and netcode | `net/src/wire/components.rs` (`WireMovementState` :65, `all_finite` :159, `has_valid_state_contract` :187, drift guard :689); `netcode/src/lib.rs:771`; `netcode/src/movement_state.rs` (:21, :64, :106, :121, :147, round-trip :516); `prediction.rs:340` (`included_dash`), :58/:63 tolerances; `reconcile.rs:122,202,232`; `scripting-core/src/refresh_plan.rs:567` |
| Versions | `net/src/handshake.rs:32` (29) and tests :452, :599; `net/src/wire.rs:346` (19) and test :3584; `networking.md` §Version gates :963 |
| Fire gates | `sim/.../commands/local.rs:147-160`; `remote.rs:81-106`; `postretro/src/host_activations.rs:103-202`; `netcode/src/activation_edges.rs:28,176,250`; `postretro/src/client_weapon/frame.rs:113-156`; `sim/src/weapon/execution.rs:121-156,269-279` |
| Descriptor surface | foundation `data_descriptors/types/movement.rs` (`PlayerMovementDescriptor` :20, `SwimParams` :349, `ImpulseStates` :495); `scripting-core/.../swim_block.rs`; `{js,lua}/movement.rs`; `{js,lua}/movement_view_feel.rs`; `sim/src/scripting/primitives/mod.rs`; `xtask/src/environment_gates.rs:31` |
| `fluid_jump_velocity` consumers | `swim_transition.rs:92,103,185,195`; `intents/swim.rs:86,89`; `swim_block.rs:43`; validator `movement.rs:401`; plus 11 test literals, all listed in the descriptor report and updated mechanically to `Some(..)` |
| Harness and fixtures | `netcode/src/predict_reconcile_harness_swim.rs`; fixtures `with_moving_platform` (`MOVING_PLATFORM_ID = 77`); physics `ledge_and_wall_world` (mod.rs:533), `CollisionWorld::from_triangles_for_test`; dev maps `e24-environment-test.map` (pool "Ledges" station at 0.61/0.81/1.22 m), `kinematic-platform.map` |

## AC-to-proof

Physics rows run through `tick_with_immersion` on purpose-built triangle worlds in a new `vault_tests.rs` (and `vault_probe_tests.rs`), at `DT = 1/60`. Netcode rows use a new `predict_reconcile_harness_vault.rs`. Confirm every Cargo filter matches tests.

| AC | Acceptance (verbatim) | Proof | Status |
|---|---|---|---|
| R1 | A ledge at the step-up limit steps up; just above it vaults. | `vault_tests`: grounded walk with `ground: "auto"` into ledges at `step_up_limit` and `+1 cm`; assert step-up vs `Vault` entry | achievable as stated |
| R2 | A ledge exactly at `maxHeight` vaults; just above refuses. | `vault_probe_tests`: ledges at `maxHeight` and `+1 cm` (allowing for skin) | achievable as stated |
| R3 | Top headroom exactly fitting the standing capsule vaults; slightly less refuses. | Ceiling over the top at capsule height plus skin, and 1 cm lower | achievable as stated |
| R4 | No `vault` block: a valid ledge never vaults. | Same world, block absent, air and ground wish into it | achievable as stated |
| R5 | Probe casts are zero on any tick where no entry could fire: no block, a grounded pawn under the default `from`, no horizontal wish, crouching or sliding, or a `"jump"` context without jump. On open floor a passing tick stops at one forward sweep. A passing tick never exceeds a fixed bound, pinned in the plan of record. | `#[cfg(test)]` probe cast counter; one test per ineligible case asserts 0; open floor asserts 1; a passing vault asserts ≤ 4 (bound pinned above) | achievable as stated |
| T1 | `air: "auto"`: wish into the ledge vaults; no wish or wish away refuses. An airborne dash into it vaults. | `vault_tests` airborne `Normal` and airborne `Dash` cases | achievable as stated |
| T2 | Default `from`: walking or jumping into a ledge from the ground does not vault. | Default block; walk and jump-tick into the ledge; assert no `Vault` on any grounded tick | achievable as stated |
| T3 | `ground: "jump"`: a grounded jump into a ledge vaults; walking into it does not. `ground: "auto"`: walking into it vaults. | Three `vault_tests` cases | achievable as stated |
| T4 | `air: "jump"`: no jump refuses; a buffered jump vaults and clears the buffer. | Airborne press before contact (buffer armed), assert entry and `jump_buffer_timer_ms == 0` and `jump_spent` | achievable as stated |
| T5 | `from: { ground: "jump" }` alone forbids air and swim entry. | Air and swim cases with that block refuse | achievable as stated |
| M1 | A vault ends grounded on the top within `durationMs` plus one tick, with exit horizontal speed equal to entry. | Count ticks in `Vault`; assert grounded on top, horizontal speed ≈ entry | achievable as stated |
| M2 | Exit refreshes the air-jump budget. `vault_started` then `vault_ended` fire, once each. | Spend air jump before entry; assert refreshed after exit; collect `MovementEvents` addresses | achievable as stated |
| M3 | Rendered eye height rises monotonically, no tick's step larger than the path step. | Sample interpolated eye between consecutive ticks (presentation interpolation helper) across the vault; assert non-decreasing and each step ≤ computed path step | achievable as stated |
| M4 | Dash, jump and crouch mid-vault are ignored. Death mid-vault leaves the state. | Physics: inputs held mid-vault leave state and path unchanged, buffer not re-armed. Sim: `drop_vault_if_dead` on a zero-health pawn leaves `Vault` | achievable as stated |
| M5 | A path blocked mid-vault aborts to airborne `Normal`, free to move the next tick. | Insert a blocker on the path after entry; assert airborne `Normal` and wish movement next tick | achievable as stated |
| M6 | A vault onto a stationary mover's ledge and onto a rising mover's ledge both end grounded on that mover. A mover despawning mid-vault aborts. | `TestMoverPoses` static, rising and removed; assert `GroundRef::Mover(id)` at exit and abort on removal | achievable as stated |
| F1 | Eye out, edge within `swimMaxHeight` of the surface: mantles out. Swim does not re-enter during the rise, and the latch clears once dry. | Swim world from `swim_tests` fixtures plus an edge; assert latch armed, no swim entry, latch false after dry | achievable as stated |
| F2 | One edge height above the surface vaults in a deep pool and a shallow one. | Two fluid depths, same edge-over-surface height | achievable as stated |
| F3 | An edge 1.8 m above the feet within `swimMaxHeight` of the surface mantles; a land ledge 1.8 m above the feet refuses at `maxHeight` 1.6. | Pair of cases | achievable as stated |
| F4 | Edge above `swimMaxHeight`, or `from` without `swim`: jump does today's fluid jump. Eye under the fluid: no mantle. | Three cases; assert fluid-jump velocity / no `Vault` | achievable as stated |
| F5 | `swim: "jump"`: pushing into a reachable edge treads; a jump mantles. | Two cases | achievable as stated |
| F6 | `fluidJumpVelocity` omitted, no ledge in reach: jump treads and never leaves. The block validates. | Physics multi-tick with `None`; descriptor parse of a block without the field (JS and Luau) | achievable as stated |
| F7 | Swim entry and land-vault entry on one tick: swim wins. | Airborne pawn falling into fluid at ledge contact on the entry tick | achievable as stated |
| W1 | Default: a start mid-vault is refused on host-local, host-remote and client-predicted paths; a start on the exit tick is accepted. `allowFire: true` accepts mid-vault on all three. | Sim local-path test; host-remote `guard_initiation` test with stamped verdict; client `frame.rs` prediction test; exit tick as pinned above | achievable as stated |
| W2 | A burst in progress at entry fires no further shot. A trigger held through the vault fires after exit without a re-press. | Burst weapon across entry on all three paths; Hold and Press triggers held through exit | achievable as stated |
| W3 | A host-remote start stamped one tick before entry and delivered after it fires, matching the client's predicted shot. | Retain a start at E−1, hold delivery (live execution) past E, assert minted and matches client prediction | achievable as stated |
| D1 | An invalid `vault` field drops the block with a warning naming it; the rest loads. | JS and Luau parser tests, one per field, with `test-log-capture` asserting the warning names the field | achievable as stated |
| D2 | The Scripting surface example loads from `content/dev/scripts/player.ts`, and a TS/Luau twin fixture pair `content/dev/scripts/movement-vault.{ts,luau}` declares the same `vault` block in both runtimes. All three load and vault in a test world. | Fixture pair plus a surface-agreement test on the `fluids-swim` model; dev `player.ts` block pinned equal to the twin's; a test world vaults under each | achievable as stated |
| D3 | Hot reload mid-vault leaves the pawn in `Normal`. | `refresh_plan` test with live `Vault` | achievable as stated |
| D4 | `Vault` round-trips the wire mirror; a non-finite or out-of-contract payload is rejected. | Round-trip array, finiteness drift guard, contract rejections, unknown mover id at merge | achievable as stated |
| D5 | Predict/reconcile: a static-ledge vault with the ack on the entry tick corrects by zero. A moving-mover vault stays within the vault tolerance. | `predict_reconcile_harness_vault.rs`: static window selected at the entry tick, `assert_no_correction`; rising-platform fixture, worst ≤ `DASH_CORRECTION_MAX_M` | achievable as stated |
| D6 | Existing movement, swim and netcode suites pass unchanged. | Full `postretro-physics`, `postretro-netcode`, `postretro-net`, `postretro-sim` suites. Version-pinning asserts (handshake :452/:599, wire :3584) move to 30/20 as the Decision requires; the 11 `fluid_jump_velocity` literals become `Some(..)` with no behavior change | achievable as stated |
| N1 | A dev station (graded ledges, a low-ceiling top, static and rising mover ledges, a pool with graded edges) plays with no camera pop. Slide→jump→vault, dash→vault and pool exits feel right. | Owner, in-engine, on a new `content/dev/maps/vault-test.map` | manual |
| N2 | A co-op client at about 100 ms latency vaults a static ledge with no visible correction. | Owner, dev latency harness (`networking.md` §Testing the conditioned link) | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thinnest risky slice: foundation `VaultParams` (Rust only) and `MovementState::Vault`; the probe at the pre-intent stage, land context only; the vault intent, path and exit; `WireMovementState::Vault` with conversions, contract and versions 30/20; and the predict/reconcile harness with a static ledge and the ack on the entry tick. Proves R1–R3, M1, D4 and the D5 static half; falsifies replay determinism first. | integrating executor | — | |
| 2 | Trigger contexts and probe cost: `from` map, auto/jump, buffer consume, buffer-arm exclusion, cast counter. R4, R5, T1–T5. | integrating executor | 1 | |
| 3 | Committed motion and lifecycle: input ignores, blocked abort, death helper (host and client predict), events and landing refresh, mover-local target, rising-mover fixture, despawn abort, vault tolerance in `included_dash`. M2–M6, the D5 mover half. | integrating executor | 1 | |
| 4 | Fluid: swim context, `swimMaxHeight`, latch on fluid entry, swim pre-empting a land vault, optional `fluidJumpVelocity`. F1–F7. | integrating executor | 2 | |
| 5 | Descriptor and presentation surface: JS and Luau parse with warn-and-drop, typedefs and SDK regen, the `vault` impulse key with a 5-spring array, the reaction table, the `player.ts` example plus the D2 fixture as restated, hot reload and `leave_vault_without_vault_block`. D1–D3. | integrating executor (worker-eligible) | 1 | |
| 6 | Fire gates: shared predicate; host-local pre-movement capture; client `allow_start`; `RetainedStart` stamp and `guard_initiation`; entry cancel on three paths; Press-trigger pending press. W1–W3. | integrating executor | 3 | |
| 7 | Dev station map and manual-check notes for N1/N2; full gate D6. | integrating executor | 1–6 | |

Hot paths: the probe adds at most 4 combined casts per pawn on eligible ticks and none otherwise. It makes no per-tick allocation, and client replay repeats it per replayed tick, as every substrate cast is repeated. The per-cast mover scan remains the separately queued broad-phase item.
