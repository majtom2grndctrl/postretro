# movement--vault

Brief · resumable · Epic 11 · reads: `context/lib/movement.md` §2–§6, `context/lib/networking.md` §Combat authority, §Version gates · read at a307e3f1

## Problem
Owner-requested capability, Epic 11's missing traversal verb. Step-up climbs only to `stepHeight`. Anything taller is a wall, even to a player airborne and pressing into it, and a swimmer cannot leave a pool whose edge outreaches its fluid jump. Done: a native `Vault` state mantles the player onto a top surface above step height and within an authored reach, from the air, the ground or the water. The pawn rises along a timed path and stands on top with its horizontal speed kept. An optional `movement.vault` block enables and tunes it, and offline play and co-op prediction agree.

## Decisions
- **Mantle, not hurdle.** Vault climbs onto a top surface and stands on it. State `Vault`, descriptor key `vault`, events `vault_started` / `vault_ended`, view-feel impulse key `vault`, following per-state naming.
- **The ledge probe is a stage before the intent** (`movement.md` §4). It runs from the tick-start pose and wish direction, and replay recomputes it, so entry needs no wire field and lags nothing. It runs only on ticks an entry could fire: block present, context listed in `from`, horizontal wish, and jump held or buffered for a `"jump"` context. It sees movers. Its cost is per pawn, never per mover; the linear mover scan inside every combined cast is a separate item.
- **Reach.** Land: height above the feet, past the step-up limit, at most `maxHeight`. Fluid: height above the fluid surface, at most `swimMaxHeight`. Landing needs standing-capsule clearance on the top.
- **Trigger per entry context.** `from` maps a context to its trigger; a missing key forbids entry from it. It maps one-to-one onto the composition draft's `states.<s>.vault`.
  - Contexts: `air` (airborne `Normal` or `Dash`), `ground` (grounded `Normal` or `Dash`), `swim` (`Swim`, tick eye out of the fluid). Crouch and slide never vault.
  - `"auto"` fires when the wish pushes into a valid ledge. `"jump"` fires on jump held or buffered there; vault consumes the buffer and sets `jump_spent`.
  - Default `{ air: "auto", swim: "auto" }`. From `Swim`, a valid ledge beats the fluid jump.
- **Motion is authored velocity, not a teleport.** The intent steers a rise-then-forward path to the stored target over `durationMs`, and the substrate still resolves collision. The live payload rides the state and the wire, as `Sliding`'s does. Exit enters grounded `Normal` with entry horizontal speed restored along the exit direction (`movement.md` §6), and ground reacquisition refreshes ability budgets.
- **A vault is committed.** Dash, jump and crouch are ignored. Death interrupts. Blocked progress aborts to airborne `Normal`, and knockback rejoins as always. A fluid-entered vault arms `swim_reentry_latched` so swim cannot re-capture the rise. A swim edge pre-empts a land-entered vault.
- **The target rides its mover.** A ledge on a mover stores the target in mover-local space, resolved each tick through that tick's mover pose. A rising lift is landed on, not missed. A despawned mover aborts the vault. Vault ticks take dash's widened correction tolerance.
- **Hands are busy.** `allowFire` defaults to `false`, the genre norm (Titanfall 2, Apex, Call of Duty, Halo clamber).
  - Entry cancels an in-progress activation, as death does.
  - A trigger held through the vault fires after exit without a re-press.
  - One predicate refuses starts on host-local, host-remote and client-predicted paths, judged at the start's stamped tick (`networking.md` §Combat authority).
- **The fluid jump becomes optional.** Omitting `swim.fluidJumpVelocity` leaves jump treading at the surface, which with `from.swim` gives mantle-only pool exits. Existing descriptors still validate.
- **Descriptor shape.** Flat `movement.vault` in today's shape; the composition draft migrates it. An invalid block drops with a warning naming the field, as swim does.
- **Wire.** New `WireMovementState::Vault`. `WIRE_VERSION` 29→30, `SNAPSHOT_VERSION` 19→20. Hot reload mid-vault drops to `Normal`.
- **Non-goals.**
  - Wall-contact forwarding: `movement--wall-run` owns it (cross-cutting-policies D8); vault reads none.
  - On-screen weapon lowering: the viewmodel shows none today, even for switches. It is its own draft, which vault reuses once it lands.
  - Mantling into a crouch-only gap; hurdling to a far side.
  - NPC vaulting (separate static-only controller); a third-person vault pose (observers animate from velocity).
  - Expression fields beyond dash; the transition graph; a combined overlap query.

### Scripting surface
```ts
// Proposed design — content/dev/scripts/player.ts
movement: {
  vault: {
    maxHeight: 1.6,      // m above feet; above stepHeight
    swimMaxHeight: 0.9,  // m above the fluid surface
    durationMs: 320,     // > 0
    from: {              // context → "auto" | "jump"; missing key = no entry
      air: "auto",
      swim: "auto",      // omit for Quake-style fluid exits only
      // ground: "jump", // opt in: jump onto a ledge from standing
    },                   // default { air: "auto", swim: "auto" }
    allowFire: false,    // default false
  },
  swim: { /* … */ fluidJumpVelocity: 6.5 }, // optional: omit for mantle-only exits
  viewFeel: { impulse: { states: { vault: { enter: { fov: 0, pitch: -3, roll: 0 } } } } },
}
// Reactions: "vault_started", "vault_ended"
```
`maxHeight`, `swimMaxHeight` and `durationMs` are required in a present block. A present `from` replaces the default whole. The Luau descriptor surface mirrors every field, proven by a TS/Luau fixture pair.

## Acceptance
### Automated
**Reach, clearance and probe cost**
- [ ] A ledge at the step-up limit steps up; just above it vaults.
- [ ] A ledge exactly at `maxHeight` vaults; just above refuses.
- [ ] Top headroom exactly fitting the standing capsule vaults; slightly less refuses.
- [ ] No `vault` block: a valid ledge never vaults.
- [ ] Probe casts are zero on any tick where no entry could fire: no block, a grounded pawn under the default `from`, no horizontal wish, crouching or sliding, or a `"jump"` context without jump. On open floor a passing tick stops at one forward sweep. A passing tick never exceeds a fixed bound, pinned in the plan of record.

**Trigger**
- [ ] `air: "auto"`: wish into the ledge vaults; no wish or wish away refuses. An airborne dash into it vaults.
- [ ] Default `from`: walking or jumping into a ledge from the ground does not vault.
- [ ] `ground: "jump"`: a grounded jump into a ledge vaults; walking into it does not. `ground: "auto"`: walking into it vaults.
- [ ] `air: "jump"`: no jump refuses; a buffered jump vaults and clears the buffer.
- [ ] `from: { ground: "jump" }` alone forbids air and swim entry.

**Motion and exit**
- [ ] A vault ends grounded on the top within `durationMs` plus one tick, with exit horizontal speed equal to entry.
- [ ] Exit refreshes the air-jump budget. `vault_started` then `vault_ended` fire, once each.
- [ ] Rendered eye height rises monotonically, no tick's step larger than the path step.
- [ ] Dash, jump and crouch mid-vault are ignored. Death mid-vault leaves the state.
- [ ] A path blocked mid-vault aborts to airborne `Normal`, free to move the next tick.
- [ ] A vault onto a stationary mover's ledge and onto a rising mover's ledge both end grounded on that mover. A mover despawning mid-vault aborts.

**Fluid**
- [ ] Eye out, edge within `swimMaxHeight` of the surface: mantles out. Swim does not re-enter during the rise, and the latch clears once dry.
- [ ] One edge height above the surface vaults in a deep pool and a shallow one.
- [ ] An edge 1.8 m above the feet within `swimMaxHeight` of the surface mantles; a land ledge 1.8 m above the feet refuses at `maxHeight` 1.6.
- [ ] Edge above `swimMaxHeight`, or `from` without `swim`: jump does today's fluid jump. Eye under the fluid: no mantle.
- [ ] `swim: "jump"`: pushing into a reachable edge treads; a jump mantles.
- [ ] `fluidJumpVelocity` omitted, no ledge in reach: jump treads and never leaves. The block validates.
- [ ] Swim entry and land-vault entry on one tick: swim wins.

**Fire**
- [ ] Default: a start mid-vault is refused on host-local, host-remote and client-predicted paths; a start on the exit tick is accepted. `allowFire: true` accepts mid-vault on all three.
- [ ] A burst in progress at entry fires no further shot. A trigger held through the vault fires after exit without a re-press.
- [ ] A host-remote start stamped one tick before entry and delivered after it fires, matching the client's predicted shot.

**Descriptor, lifecycle and netcode**
- [ ] An invalid `vault` field drops the block with a warning naming it; the rest loads.
- [ ] The Scripting surface example loads from `content/dev/scripts/player.ts`, and a TS/Luau twin fixture pair `content/dev/scripts/movement-vault.{ts,luau}` declares the same `vault` block in both runtimes. All three load and vault in a test world.
- [ ] Hot reload mid-vault leaves the pawn in `Normal`.
- [ ] `Vault` round-trips the wire mirror; a non-finite or out-of-contract payload is rejected.
- [ ] Predict/reconcile: a static-ledge vault with the ack on the entry tick corrects by zero. A moving-mover vault stays within the vault tolerance.
- [ ] Existing movement, swim and netcode suites pass unchanged.

### Manual
- [ ] A dev station (graded ledges, a low-ceiling top, static and rising mover ledges, a pool with graded edges) plays with no camera pop. Slide→jump→vault, dash→vault and pool exits feel right.
- [ ] A co-op client at about 100 ms latency vaults a static ledge with no visible correction.

## Path
- Seams: `MovementState` / `MovementStateKind`, `dispatch_state_intent`, `environment_transition` (swim edge, latch, pre-intent casts), `step_up_lift`, `clearance_probe`. Netcode checklist, fire-gate sites and mover-pose sources: `research.md`.
- Shape: a probe before the intent. Rival: a probe result forwarded to the next tick, which costs an outer wire field on every snapshot and lags entry a tick.
- First slice: probe and a land vault on a static ledge in physics tests, then the predict/reconcile harness with the ack on the entry tick. It falsifies replay determinism before descriptor, fluid and fire work.
- `crates/physics/src/movement/mod.rs` and `substrate.rs` are past 800 lines. Probe and vault tests get their own modules, as `swim_tests.rs` did.

## Boundary inventory
| Name | Rust | Wire | JS / TS | Luau |
|---|---|---|---|---|
| state | `MovementState::Vault`, `MovementStateKind::Vault` | `WireMovementState::Vault` | n/a | n/a |
| block | `PlayerMovementDescriptor.vault` | n/a (tuning stays local) | `vault` | `vault` |
| fields | `max_height`, `swim_max_height`, `duration_ms`, `from`, `allow_fire` | n/a | `maxHeight`, `swimMaxHeight`, `durationMs`, `from`, `allowFire` | same as JS |
| entry contexts | `from.air`, `from.ground`, `from.swim`: `Option<trigger enum>` | n/a | `air`, `ground`, `swim` | same as JS |
| trigger values | enum `Auto`, `Jump` | n/a | `"auto"`, `"jump"` | `"auto"`, `"jump"` |
| swim field | `SwimParams.fluid_jump_velocity: Option<f32>` | n/a | `fluidJumpVelocity?` | `fluidJumpVelocity?` |
| events | entry/exit address | n/a | `"vault_started"`, `"vault_ended"` | same |
| impulse key | `ImpulseStates.vault` | n/a | `vault` | `vault` |

## Open questions
- Vault wire payload (target, its frame and mover id, speed fields) — **delegated**: pin it in the plan of record, carrying only what replay needs, as `Sliding` does.
- How the host recovers the shooter's movement state at a delivered start's stamped tick — **delegated**: report the mechanism. If it needs new wire data, stop and bring it to the owner.
- The rise-then-forward curve — **delegated**: report the shape and how it meets the monotonic-eye row.
