# movement--vault

Brief · resumable · Epic 11 · reads: `context/lib/movement.md` §2–§6, `context/lib/networking.md` §Version gates, §Combat authority · read at a307e3f1

## Problem
Owner-requested capability, Epic 11's missing traversal verb. Step-up climbs only up to `stepHeight` from the ground. Anything taller is an unclimbable wall, even to a player airborne and pressing into it. A swimmer cannot leave a pool whose edge is higher than its fluid jump can carry it. Done: a native `Vault` state mantles the player onto a top surface above step height and within an authored reach, from the air or out of fluid. The pawn rises along a timed path and ends standing on top with its horizontal speed. It is enabled and tuned by an optional `movement.vault` descriptor block. Offline play and co-op prediction behave the same.

## Decisions
- **Mantle, not hurdle.** Vault climbs onto a top surface and stands on it. Clearing a thin obstacle to its far side is a non-goal. State `Vault`, descriptor key `vault`, events `vault_started` / `vault_ended`, view-feel impulse key `vault`, all following the existing per-state naming (`movement.md` §4, §1).
- **The probe runs at tick start, owned by the substrate.** A substrate stage resolves a ledge candidate before the intent from the tick-start position, capsule, and horizontal wish direction. Intents receive the result and never cast. This is the environment-resolution shape (`movement.md` §4): a pure function of tick-start pose and the collision world. Reconcile replay therefore recomputes it on the first replayed tick, with no wire field and no one-tick lag. It diverges from "contact flows forward" for this one probe only. `movement.md` §4 gains the carve-out at promotion. The probe runs only when a `vault` block exists, the state can enter vault, and there is horizontal wish. On open floor it stops after one forward sweep. Its geometry follows `step_up_lift` and reuses the clearance sweeps. It sees movers.
- **Wall-contact forwarding stays with `movement--wall-run`.** Vault reads no forwarded wall normal, so the decision D8 assigned in `plans/done/movement--cross-cutting-policies` stands unchanged.
- **Reach.** On land, ledge height is measured from the feet. It must exceed the step-up limit and be at most `maxHeight`. In fluid it is measured from the fluid surface (derived from immersion), at most `swimMaxHeight`. Land and water reach tune independently. Landing requires standing-capsule clearance on the top. Mantling into a crouch-only gap is a non-goal.
- **Trigger.** `trigger: "auto" | "jump"`, default `"auto"`.
  - Vault enters from airborne `Normal`, `Dash`, and `Swim`. With `trigger: "jump"` it also enters from grounded `Normal`.
  - `auto`: an airborne pawn, or a swimmer whose tick eye is out of the fluid, pushing its wish into a valid ledge.
  - `jump`: jump held or buffered at a valid ledge, from the ground, the air, or the surface. Vault consumes the buffer and sets `jump_spent`.
  - From `Swim`, a valid ledge wins over the fluid jump. Otherwise jump behaves as today.
  - `fromSwim: false` disables fluid entry entirely and keeps pure Quake-style exits.
- **Motion is velocity, not teleport.** The intent authors velocity along a rise-then-forward path to the stored target over `durationMs`. The substrate still resolves collision, and tick interpolation smooths the camera. The live payload (target, elapsed time, entry horizontal speed and direction) lives on the state and is wire-mirrored, following `Sliding`. A successful exit enters grounded `Normal` with the entry horizontal speed restored along the exit direction (`movement.md` §6 carry at the dispatch edge). Ground reacquisition is the landing-refresh point for ability budgets.
- **A vault is committed.** Dash, jump and crouch are ignored while vaulting. Death interrupts it. Blocked progress aborts to airborne `Normal`, and knockback still rejoins as always. A vault entered from fluid arms `swim_reentry_latched`, as the fluid jump does, so swim cannot re-capture the rise. A swim edge still pre-empts a vault entered on land.
- **Mover ledges are in scope.** The path is world-space, and exit ground-stick acquires `GroundRef::Mover`. A mispredicted mover pose is corrected by reconcile (`networking.md` §Current contract), and vault ticks get the same widened correction tolerance as dash and slide.
- **Fire during a vault is an authored choice.** `allowFire`, default `true`. One predicate (shooter in `Vault` and `allowFire` false) refuses fire on host-local, host-remote, and client-predicted paths alike. On the host the predicate reads the shooter's movement state after that command's movement step, so a predicted shot and its host verdict agree. Weapon lowering and animation are non-goals.
- **The fluid jump becomes optional.** `swim.fluidJumpVelocity` may be omitted. Absent means jump at the surface only treads unless a ledge is in reach, which gives a mantle-only design. This changes a shipped field from required to optional; existing descriptors still validate.
- **Descriptor shape.** A flat `movement.vault` block in today's shape. If `player-descriptor-composition` lands, it migrates the block to `states.vault`. An invalid block is dropped with a warning naming the field, and the rest loads, following swim (`keep_or_drop_swim_block`), not slide's hard error.
- **Wire.** New `WireMovementState::Vault` variant. `WIRE_VERSION` 29→30 and `SNAPSHOT_VERSION` 19→20, following slide. A hot reload mid-vault drops the pawn to `Normal`; only `Swim` survives reload.
- **Non-goals.** NPC vaulting: agents use a separate static-only controller. A third-person vault pose for observers: remote locomotion derives from velocity. Expression fields beyond dash. The transition graph. A combined overlap query: the clearance test phrases as sweeps.

### Scripting surface
```ts
// Proposed design — content/dev/scripts/player.ts
movement: {
  vault: {
    maxHeight: 1.6,      // m above feet; must exceed stepHeight
    swimMaxHeight: 0.9,  // m above the fluid surface
    durationMs: 320,     // rise-and-over time, > 0
    trigger: "auto",     // "auto" | "jump"; default "auto"
    fromSwim: true,      // mantle out of fluid; default true
    allowFire: true,     // fire while vaulting; default true
  },
  swim: { /* … */ fluidJumpVelocity: 6.5 }, // now optional: omit for mantle-only exits
  viewFeel: { impulse: { states: { vault: { enter: { fov: 0, pitch: -3, roll: 0 } } } } },
}
// Reactions: "vault_started", "vault_ended"
```
`maxHeight`, `swimMaxHeight` and `durationMs` are required in a present block. `trigger`, `fromSwim` and `allowFire` default as shown. The Luau descriptor mirrors every field, and both ship.

## Acceptance
### Automated
**Reach and clearance**
- [ ] A ledge at the step-up limit is stepped up with no vault. A ledge just above the limit vaults.
- [ ] A ledge exactly at `maxHeight` vaults; one just above it refuses.
- [ ] Headroom on top exactly fitting the standing capsule vaults; slightly less refuses.
- [ ] With no `vault` block, a valid ledge never vaults, and the probe casts nothing.
- [ ] On open floor with a `vault` block, the probe stops at its first forward sweep: no lift or down sweep runs.

**Trigger**
- [ ] `auto`: airborne with wish into the ledge vaults. No wish, or wish away, refuses.
- [ ] `auto`: a grounded pawn walking into a ledge does not vault.
- [ ] `auto`: an airborne dash into a ledge vaults. A crouching or sliding pawn does not.
- [ ] `jump`: airborne without jump refuses. A buffered jump vaults and clears the buffer. A grounded jump into a ledge vaults.

**Motion and exit**
- [ ] A vault ends grounded on the top within `durationMs` plus one tick. Exit horizontal speed equals entry horizontal speed.
- [ ] The air-jump budget refreshes on exit. Events fire in order `vault_started` then `vault_ended`, once each.
- [ ] Rendered eye height rises monotonically over the vault ticks, with no single-tick jump larger than the path step.
- [ ] A dash press, jump or crouch mid-vault is ignored.
- [ ] A path blocked mid-vault aborts to airborne `Normal`, and the pawn moves freely the next tick.
- [ ] Death mid-vault leaves the vault state.
- [ ] A vault onto a mover ledge ends grounded on that mover.

**Fluid**
- [ ] Eye out of the fluid, edge within `swimMaxHeight` of the surface: mantles out. Swim does not re-enter during the rise, and the latch clears once dry.
- [ ] The same edge height above the surface vaults in a deep pool and in a shallow one.
- [ ] An edge 1.8 m above the feet but within `swimMaxHeight` of the surface mantles from fluid. A land ledge 1.8 m above the feet refuses when `maxHeight` is 1.6.
- [ ] Edge above `swimMaxHeight`: jump does today's fluid jump.
- [ ] Eye under the fluid: no mantle.
- [ ] `fromSwim: false`: a reachable edge gets the fluid jump, never a vault.
- [ ] `fluidJumpVelocity` omitted, no ledge in reach: jump treads, never leaves. The swim block validates.
- [ ] Swim entry and land-vault entry on one tick: swim wins.

**Fire**
- [ ] `allowFire: false`: fire is refused mid-vault on the host-local, host-remote, and client-predicted paths. It is accepted on the tick the vault ends.
- [ ] `allowFire: true`: fire mid-vault is accepted on all three paths.

**Descriptor and lifecycle**
- [ ] An invalid `vault` field drops the block with a warning naming the field. The rest of the descriptor loads.
- [ ] The Scripting surface example loads from `content/dev/scripts/player.ts` and its Luau twin, and vaults in a test world.
- [ ] A hot reload mid-vault leaves the pawn in `Normal`.

**Netcode**
- [ ] `Vault` round-trips through the wire mirror. A non-finite or out-of-contract payload is rejected.
- [ ] Predict/reconcile harness: a vault on a static ledge, with the host ack landing on the entry tick, produces zero correction.
- [ ] Predict/reconcile harness: a vault onto a moving mover stays within the vault correction tolerance.
- [ ] Existing movement, swim and netcode suites pass unchanged.

### Manual
- [ ] A dev station (graded ledges, a low-ceiling top, a mover ledge, a pool with graded edges) plays with no camera pop. Slide→jump→vault, dash→vault and pool exits feel right.
- [ ] A co-op client at about 100 ms latency vaults a static ledge with no visible correction.

## Path
- Seams: `MovementState` / `MovementStateKind` (foundation), `dispatch_state_intent`, `environment_transition` (swim edge and latch), `resolve_at_tick_start` (the pre-intent stage precedent), `step_up_lift`, `clearance_probe`. Netcode checklist and fire-gate sites: `research.md`.
- Shape: a pre-intent substrate probe. The rival is a forwarded probe result carried on the outer wire state, which costs every snapshot and lags entry by a tick.
- First slice: probe plus a land vault on a static ledge in the physics unit tests, then the predict/reconcile harness with the ack on the entry tick. This falsifies the replay-determinism premise before the descriptor, fluid and fire work.
- `crates/physics/src/movement/mod.rs` (6k+ lines) and `substrate.rs` are past 800 lines. Put the probe and vault tests in their own modules (`swim_tests.rs` precedent), not in `mod.rs`.

## Boundary inventory
| Name | Rust | Wire | JS / TS | Luau |
|---|---|---|---|---|
| state | `MovementState::Vault`, `MovementStateKind::Vault` | `WireMovementState::Vault` | n/a | n/a |
| block | `PlayerMovementDescriptor.vault` | n/a (tuning stays local) | `vault` | `vault` |
| fields | `max_height`, `swim_max_height`, `duration_ms`, `trigger`, `from_swim`, `allow_fire` | n/a | `maxHeight`, `swimMaxHeight`, `durationMs`, `trigger`, `fromSwim`, `allowFire` | same as JS |
| trigger values | enum `Auto`, `Jump` | n/a | `"auto"`, `"jump"` | `"auto"`, `"jump"` |
| swim field | `SwimParams.fluid_jump_velocity: Option<f32>` | n/a | `fluidJumpVelocity?` | `fluidJumpVelocity?` |
| events | entry/exit address | n/a | `"vault_started"`, `"vault_ended"` | same |
| impulse key | `ImpulseStates.vault` | n/a | `vault` | `vault` |

## Open questions
- Vault payload layout on the wire (which target and speed fields) — **delegated**: the executor pins it in the plan of record, following `Sliding`'s pattern of carrying only what replay needs.
- Exact rise-then-forward path curve — **delegated**: report the chosen shape and how it meets the monotonic-eye row.
