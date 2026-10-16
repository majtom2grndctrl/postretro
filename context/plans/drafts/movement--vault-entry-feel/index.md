# movement--vault-entry-feel

Brief · compact · reads: `context/lib/movement.md` §1 §2 §4 (Vault), `context/lib/scripting.md` §5 §11, `context/lib/player_options.md` §5 · read at 41f21c8a

## Problem

The project lead asked for vault view feel that answers how the vault began: a still step-up mantle should read as a subtle bob, a sprinting leap onto a ledge as a hard catch. This is a requested capability. The held pose (`viewFeel.vault`) and the vault kick (`impulse.states.vault`) take literals only, so every vault presents the same camera regardless of entry. When this is done, an author writes the pose angles and the kick fields as fluent expressions over the pawn's motion as the vault began (horizontal speed, vertical speed, time airborne, grounded or swimming, climb height). Each vault evaluates them once and keeps its values through its kicks and its release.

## Decisions

- **Fields take expressions directly; no scale sidecar.** The pose's `pitch`, `roll` and `fov`, and the vault kick row's `tension` and `enter`/`exit` channels, accept a literal or a numeric expression. This is the dash pattern (`movement.md` §2, `plans/done/M14--movement-dash-runtime-values`): a literal stays bit-identical, and the engine pins when the field evaluates. `easeIn` and `easeOut` stay literal: the kick's `tension` carries entry-dependent snap, and `movement--state-view-pose` times its rows by rates, not progress shares, so its `pose.vault` fold carries only the angle fields, and carries them unchanged.
- **One `motion` vocabulary.** `motion` is a frozen namespace exported from `"postretro"`, as `activation` is. Its fluent refs lower to the input reads dash already binds: `speed`, `verticalSpeed`, `grounded`, `chargesRemaining`, `cooldownMs` and `elapsedMs`, same names and meanings. It adds `swimming`, `airborneMs` and `climbHeight`. A name means the pawn's motion at the field's evaluation moment, so `motion.speed` in a vault field is the speed the vault began with. Dash fields bind the dash names; vault fields bind `speed`, `verticalSpeed`, `grounded`, `swimming`, `airborneMs` and `climbHeight`. Any other name, or a result type that does not match the field, rejects the descriptor naming the field.
- **Not a live store slot.** A live `player.speed` would read the climb path's velocity once the vault has begun. Movement and charge expressions also bind closed scopes with no store reads (`scripting.md` §11). A readonly speed slot for HUD or crossings stays a separate door with no consumer yet.
- **`defineEntity` lowers fluent refs** in every expression field of `components.movement`, in TypeScript and Luau, beside its loadout lowering. Authors write fluent refs with no builder, and raw `runtime.*` IR keeps working. Dash fields accept `motion` refs as a consequence.
- **Each vault evaluates once, at its entry edge.** The movement tick reports the vault's entry facts with its entry edge: entry speed, climb height, and the vertical speed, grounded flag and swim state the pawn had as the vault began. The record is local presentation output, like the edge itself, and never reaches the component, the vault payload or the wire. View feel evaluates the vault's fields when it applies that edge, and the enter kick, the pose, the exit kick and the release all use that one evaluation. A vault with no evaluation when its pose or exit kick needs one evaluates once from neutral facts: payload speed and climb height when present, otherwise 0, with grounded true, not swimming, no vertical speed and no airtime. That covers a vault the camera joins after a reconcile or a view-feel reset. A rewind keeps the vault's evaluation; only a new entry edge evaluates again.
- **`airborneMs` is tracked render-side.** It counts the simulated time of the frames that run while the pawn is airborne, resets while the pawn is grounded, swimming or vaulting, and is read when the entry edge applies, so it can lag by the ticks of that frame. No tick-side timer exists: the coyote timer counts time in water as airborne. View-feel resets keep the count; a pawn change or level install restarts it.
- **Results follow dash: clamp silently to the literal range.** Pose angles clamp to ±90°, kick channels to ±180° and then the impulse `max`, and kick `tension` to [0.1, 240].
- **Overlap.** A vault that starts while the previous vault's pose is releasing presents both: the release with the previous vault's values, plus the new vault's pose with its own, each within its clamp. This replaces `movement.md` §1's larger-weight rule, which assumed one value set for both. The vault kick spring is shared, so from a new entry edge the whole spring decays at the newest evaluated `tension`. (P12, P15)
- **Vault fields only.** Other kick rows, the shared impulse `tension` and `max`, and the other view-feel blocks stay literal and reject expressions. Each extends by pinning its own evaluation moment when asked. The grounded and swimming inputs replace the per-transition map `plans/done/movement--state-transition-feel` named as its extension point: one row reads how it was entered instead of keying rows by source state.
- **Presentation and accessibility unchanged.** `view_feel_scale` and reduced motion multiply the evaluated output (`player_options.md` §5), and evaluation happens at every scale. View feel stays out of the tuning payload, so co-op is untouched.
- **Non-goals.** New dash inputs: `swimming` is always false in a dash, which enters only from `Normal` or `Crouching`; `airborneMs` has no tick-side source; `climbHeight` means nothing outside a vault. A later tick-side source for `airborneMs` replaces the render tracker rather than adding a second meaning.

### Scripting surface

```typescript
import { defineEntity, motion } from "postretro";

// ~1 at run speed, up to +0.5 more for a long fall; a still step-up stays a nudge.
const hard = motion.speed.dividedBy(11)
  .plus(motion.airborneMs.dividedBy(600).clamp(0, 0.5))
  .clamp(0.25, 1.5);
const airCatch = motion.grounded.or(motion.swimming).not();

// Inside defineEntity({ components: { movement: { ... } } }):
viewFeel: {
  vault: { pitch: hard.times(-6), roll: airCatch.select(hard.times(2), 0), fov: hard.times(-3), easeIn: 0.15, easeOut: 0.2 },
  impulse: {
    tension: 12,
    max: { fov: 20, pitch: 8, roll: 6 },
    states: {
      vault: {
        tension: motion.speed.gt(8).select(20, 12),
        enter: { fov: hard.times(-2), pitch: hard.times(-4), roll: 0 },
        exit: { fov: 0, pitch: motion.climbHeight.dividedBy(-1.5).clamp(-2, 0), roll: 0 },
      },
    },
  },
},
dash: { momentumRetention: motion.grounded.select(0.4, 0.7) /* remaining dash fields unchanged */ },
```

Luau ships the same surface through `require("postretro").motion`, with method calls such as `hard:times(-6)` and the bracket spelling `scripting.md` §5 sets for reserved words: `motion.grounded["or"](motion.grounded, motion.swimming)`.

## Acceptance

### Automated
- [ ] TypeScript and Luau load the dev player with the Scripting surface example. Both reject the same cases, naming the field path: an unknown input name; `climbHeight`, `swimming` or `airborneMs` in a dash field; `chargesRemaining`, `cooldownMs` or `elapsedMs` in a vault field; a Bool expression in a number field; an expression in `easeIn` or `easeOut`, on a non-vault kick row, in the shared impulse `tension` or `max`, or in any other view-feel block; a fluent ref in a descriptor not built through `defineEntity`.
- [ ] A descriptor with only literals presents pose and kick output bit-identical to the same descriptor before this change.
- [ ] A vault field written as raw `runtime.*` IR presents the same as its `motion` spelling. Dash with `motion.grounded.select(0.4, 0.7)` behaves identically to `runtime.select(runtime.read("grounded"), 0.4, 0.7)`, grounded and airborne.
#### Inputs
- [ ] A grounded vault reads `grounded` true, `swimming` false and `airborneMs` 0.
- [ ] A fall into a ledge reads negative `verticalSpeed` and a catch on the rise positive, though the climb rises from the vault's first tick. (P2)
- [ ] An air catch after a known airborne time reads that time within the ticks of the entry frame. Stalls and level holds add no airborne time. (P8)
- [ ] A swim exit reads `swimming` true and `airborneMs` 0, even after a long time in the water.
- [ ] `speed` equals the entry speed, knockback and drift included. `climbHeight` equals the landing height minus the entry feet, on a static ledge and on a moving mover.
#### Evaluation timing
- [ ] The first frame of a vault kicks with that vault's values, never the previous vault's or zero, including the first vault after load. (P1)
- [ ] Motion that changes during the climb does not change presented values. The release after the climb, and an early end's release, keep them.
- [ ] Two back-to-back vaults with different entries each present their own values, also when the first vault's exit and the second's entry land in one frame. (P4)
- [ ] A vault that starts during the previous vault's release presents the release with the previous values plus its own pose, with no step beyond its own ease-in. (P12)
- [ ] A vault whose entry and exit both land in one frame, never shown, still kicks with its own values. (P13)
- [ ] Vault kick expressions with no held pose kick each vault with its own values. (P11)
- [ ] A vault the camera joins after a reconcile or a mid-vault view-feel reset evaluates once from neutral facts, with no enter kick. A reset keeps the airborne time counting. A refresh on the frame a vault ends gives its exit kick neutral facts. (P7, P14)
- [ ] A reconcile that shifts a vault's progress, with no new entry edge, leaves its presented values unchanged. (P6)
#### Bounds
- [ ] Expressions evaluating far past every range present clamped values: pose within ±90°, kicks within ±180° and `max`, `tension` within [0.1, 240]. An expression dividing by an input that reads 0 presents a finite, clamped value. Composed render pitch stays within ±89° as a regression guard.
#### Regression guards
- [ ] `view_feel_scale` 1, 0.5 and 0 (reduced motion) present full, half and zero. Restoring the scale mid-vault resumes with no snap and shows the values the vault began with. (P10)
- [ ] The viewmodel transform is unchanged by the evaluated pose.
- [ ] `WIRE_VERSION`, `SNAPSHOT_VERSION` and the wire fixtures are unchanged. Committed SDK typedefs match the registry.
- [ ] `content/dev/scripts/player.ts` and the `movement-vault.ts`/`.luau` twins author vault feel through `motion` and load.
### Manual
- [ ] In `vault-test`: a still waist-high mantle reads as a subtle nudge, a sprint-jump catch as a hard snap and lean, a long fall into a ledge heaviest. A pool exit stays gentle.
- [ ] With expressions pushed absurdly high, the camera looks exaggerated but never flips or tears.
- [ ] Reduced motion on: no vault pose or kick, whatever the entry.

## Path

- Precedents: dash's `NumberOrIr` fields, `DashPrograms` binding at component construction, and `resolve_number`'s clamp (`physics::movement::intents::dash`); `foundation::movement::scope::MovementScope` (indexed inputs, `for_validation`); `sdk/lib/activation.ts` (frozen fluent namespace); `sdk/lib/util/expression_refs.ts` and the Luau twin (IR hidden from the parser, so lowering is SDK-side); `lowerLoadoutReferences` and its `loweredValue` helper in `sdk/lib/data_script.ts` and `.luau`.
- Seams: `physics::movement::vault::vault_entry` computes every entry fact before the vault intent overwrites velocity; `MovementEvents` already carries local per-tick presentation data (`swim_fluid`); `frame_loop` queues every tick's edges in order as `TimedMovementEdge`; `view_feel::impulse` applies them before `view_feel::vault::evaluate` runs; `VaultViewParams` and the vault `ImpulseStateParams` take `NumberOrIr` (`ViewFeelParams` is serde, so bind where view feel is driven); `scripting-core` `movement_view_feel` parsers.
- Shape chosen: expression-valued fields, a shared `motion` vocabulary, and evaluation at the entry edge. Strongest rival: evaluation on the first frame that shows the vault, fed by a render-side tracker. It needs no tick output, but it misses vaults no frame shows, reads stale context across back-to-back vaults, and couples kicks to pose state. Research: `research.md`.
- First slice: `pitch` from `motion.speed` on the dev player in `vault-test`. It falsifies the riskiest assumption, that a frozen entry value reads as a different catch rather than as noise.
- `view_feel.rs` sits past 1300 lines. Put the evaluation records and the airborne tracker beside `vault.rs`, and keep `view_feel.rs` to wiring. The existing release-continuity test changes with the overlap rule.
- Update `docs/scripting-reference.md`: its dash read-name table becomes the `motion` table with a per-field supported-inputs column. Add a note to `movement--state-view-pose` that `pose.vault`'s angle fields accept `motion` expressions.
- At promotion: `scripting.md` §11's adopter list gains the first presentation-side adopter, evaluated per edge, not per tick, and free of determinism constraints. `movement.md` §1 records the local entry record and the single-source rule for `airborneMs`. `movement.md` §2's "no reads outside the movement component" line notes that view-feel inputs are presentation reads it does not govern.

## Boundary inventory

camelCase on script sides, snake_case in Rust. No FGD, no PRL, not on the wire. TypeScript and Luau both ship.

| Name | Rust | JS / TS | Luau |
|---|---|---|---|
| namespace | IR input names, bound per field site | `motion` | `motion` |
| dash inputs | `speed`, `verticalSpeed`, `grounded`, `chargesRemaining`, `cooldownMs`, `elapsedMs` (existing read names) | same, `NumberRef` / `BoolRef` | same |
| vault inputs | `speed`, `verticalSpeed`, `grounded` (shared names), `swimming`, `airborneMs`, `climbHeight` (new) | same | same |
| vault pose fields | `VaultViewParams` `pitch`/`roll`/`fov` as `NumberOrIr` | `viewFeel.vault.*` | same |
| vault kick fields | vault `ImpulseStateParams` `tension`, `enter`, `exit` channels as `NumberOrIr` | `impulse.states.vault.*` | same |
| entry record | local movement-event output beside the vault entry edge; not a component field, not on the wire | none | none |

## Open questions

- Where evaluation records and bound programs live, how a record is tied to its vault for the exit kick and the release, and whether the vault kick row gets its own type or non-vault rows reject IR at parse — **delegated**
- `check_ir` node and depth bounds for view-feel programs — **delegated**
