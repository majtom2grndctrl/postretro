# movement--vault-entry-feel

Brief · compact · reads: `context/lib/movement.md` §1 §2 §4 (Vault), `context/lib/scripting.md` §5 §11, `context/lib/player_options.md` §5 · read at 41f21c8a

## Problem

The project lead asked for vault view feel that answers how the vault began: a still step-up mantle should read as a subtle bob, a sprinting leap onto a ledge as a hard catch. This is a requested capability. The held pose (`viewFeel.vault`) and the vault kick (`impulse.states.vault`) take literals only, so every vault presents the same camera regardless of entry. When this is done, an author writes those fields as fluent expressions over the pawn's motion as the vault began (horizontal speed, vertical speed, time airborne, grounded or swimming, climb height), and each vault evaluates them once and keeps the result through its release. The same `motion` vocabulary gives dash's existing expression fields a fluent spelling.

## Decisions

- **Fields take expressions directly; no scale sidecar.** `viewFeel.vault`'s `pitch`, `roll`, `fov`, `easeIn` and `easeOut`, and the vault kick row's `tension`, `enter` and `exit` channels, accept a literal or a numeric expression. This is the dash pattern (`movement.md` §2, `plans/done/M14--movement-dash-runtime-values`): a literal stays bit-identical, and the engine pins when the field evaluates. Timing and the enter and exit kicks can each answer the entry, which a magnitude multiplier cannot. It is also the door `movement--state-view-pose` names for `pose.vault`, so its fold can carry these fields unchanged.
- **One `motion` vocabulary for every movement expression.** `motion` is a frozen namespace exported from `"postretro"`, as `activation` is, holding fluent refs that lower to the same input reads dash binds today. It keeps dash's names and meanings (`speed`, `verticalSpeed`, `grounded`, `chargesRemaining`, `cooldownMs`, `elapsedMs`), adds `swimming` and `airborneMs` (time since the pawn was last grounded or swimming), and adds `climbHeight`, the one vault-only input. A name means the pawn's motion at the field's pinned evaluation moment, so `motion.speed` in a vault field is the speed the vault began with. Each field site binds only the names it can supply: dash fields the six dash names, vault fields `speed`, `verticalSpeed`, `grounded`, `swimming`, `airborneMs` and `climbHeight`. Any other name, or a result type that does not match the field, rejects the descriptor naming the field.
- **Not a live store slot.** A live `player.speed` would read the climb path's velocity once the vault has begun, not the run that led into it. Movement and charge expressions also bind closed scopes with no store reads (`scripting.md` §11). A readonly speed slot for HUD or crossings stays a separate door with no consumer yet.
- **`defineEntity` lowers fluent refs** anywhere in `components.movement`, in TypeScript and Luau, as it already lowers loadout references. Authors write fluent refs in fields with no builder. Raw `runtime.*` IR keeps working. Dash adopts fluent spelling in the same change, which gives the lowering a second consumer from day one.
- **Vault fields evaluate once per vault, on the first frame that shows it.** That frame is the one with its entry edge, or a pose join (a reconcile or a view-feel reset that shows it in progress, or a rewind read as a new vault). The pose and both kicks of that vault use that one evaluation through the climb and its release, and a later vault evaluates again. A joined vault has no enter kick, as today.
- **Input sources are render-side; no tick, wire or payload change.** `speed` and `climbHeight` come from the vault payload the local pawn already carries: the entry speed, and the vertical part of the start offset, which is true vertical because vault frames are upright. View-feel state tracks `verticalSpeed`, `grounded`, `swimming` and `airborneMs` from the driver's per-frame pawn facts while the pawn is not vaulting, and freezes them at evaluation. They can lag by the ticks the vault's own frame ran. Placement follows `movement.md` §1: view feel reads the pawn and writes nothing back.
- **Results follow dash: clamp silently to the literal range.** Pose angles clamp to ±90°, kick channels to ±180°, then the impulse `max`, and kick `tension` to [0.1, 240]. `easeIn` clamps to [0, 1] and `easeOut` to [0, 1 − `easeIn`]. A literal pair summing past 1 still rejects. A non-finite node is excluded by the IR evaluator's finite guard. Composed render pitch stays within ±89°.
- **Vault row only.** Other kick rows stay literal. The vocabulary and the tracked state before the vault are general, so each row extends by pinning its own evaluation moment when asked. The context inputs replace the per-transition map `plans/done/movement--state-transition-feel` named as its extension point: one row reads how it was entered instead of keying rows by source state.
- **Presentation and accessibility unchanged.** `view_feel_scale` and reduced motion multiply the evaluated output (`player_options.md` §5). The viewmodel still never receives the held pose. View feel stays out of the tuning payload, so co-op is untouched.
- **Non-goals.** New dash inputs: `swimming` and `airborneMs` would need tick-side state dash's scope does not have, and `climbHeight` means nothing outside a vault. Any tick-side site that later binds `airborneMs` or `swimming` replaces the render tracker as their single source, rather than adding a second. Inputs that change during the climb: values freeze at the evaluation moment, as dash entry fields do. Remote pawns: view feel follows only the local pawn.

### Scripting surface

```typescript
import { defineEntity, motion } from "postretro";

// ~1 at run speed, up to +0.5 more for a long fall; a still step-up stays a nudge.
const hard = motion.speed.dividedBy(11)
  .plus(motion.airborneMs.dividedBy(600).clamp(0, 0.5))
  .clamp(0.25, 1.5);
const airCatch = motion.grounded.or(motion.swimming).not();

export const playerEntity = defineEntity({
  components: {
    movement: {
      dash: {
        momentumRetention: motion.grounded.select(0.4, 0.7),
        // remaining dash fields as in content/dev/scripts/player.ts
      },
      viewFeel: {
        vault: {
          pitch: hard.times(-6),
          roll: airCatch.select(hard.times(2), 0),
          fov: hard.times(-3),
          easeIn: motion.speed.gt(8).select(0.05, 0.15), // a fast catch snaps in sooner
          easeOut: 0.2,
        },
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
    },
  },
});
```

Luau ships the same surface through `require("postretro").motion`, with method calls such as `hard:times(-6)` and the bracket spelling `scripting.md` §5 sets for reserved words: `motion.grounded["or"](motion.grounded, motion.swimming)`.

## Acceptance

### Automated
- [ ] TypeScript and Luau load the Scripting surface example, completed with the dev player's remaining fields. Both reject the same cases, naming the field path: an unknown input name; `climbHeight`, `swimming` or `airborneMs` in a dash field; `chargesRemaining`, `cooldownMs` or `elapsedMs` in a vault field; a Bool expression in a number field; an expression on a non-vault kick row; a literal `easeIn` + `easeOut` past 1; a fluent ref in a descriptor not built through `defineEntity`.
- [ ] A descriptor with only literals presents pose and kick output bit-identical to the same descriptor before this change.
- [ ] Dash with `motion.grounded.select(0.4, 0.7)` behaves identically to `runtime.select(runtime.read("grounded"), 0.4, 0.7)`, grounded and airborne.
#### Inputs
- [ ] A grounded vault reads `grounded` true, `swimming` false and `airborneMs` 0.
- [ ] An air catch after a known airborne time reads that time within the vault frame's tick span. A fall into a ledge reads negative `verticalSpeed`; a catch on the rise reads positive.
- [ ] A swim exit reads `swimming` true and `airborneMs` 0, even after a long time in the water.
- [ ] `speed` equals the payload's entry speed, knockback and drift included, and does not read the climb path's velocity. `climbHeight` equals the landing height minus the entry feet, on a static ledge and on a moving mover.
#### Evaluation timing
- [ ] Motion that changes during the climb does not change presented values. The release after the climb, and an early end's release, keep them.
- [ ] Two back-to-back vaults with different entries each present their own values.
- [ ] A vault the pose joins in progress (reconcile, mid-vault view-feel reset) evaluates once at the join, with no enter kick.
- [ ] The enter kick, the exit kick and the pose of one vault use one evaluation.
#### Bounds
- [ ] Expressions evaluating far past every range present clamped values: pose within ±90°, kicks within ±180° and `max`, `tension` within [0.1, 240], `easeIn` + `easeOut` at most 1, composed render pitch within ±89°. The view never flips.
#### Regression guards
- [ ] `view_feel_scale` 1, 0.5 and 0 (reduced motion) present full, half and zero. Restoring the scale mid-vault resumes with no snap.
- [ ] The viewmodel transform is unchanged by the evaluated pose.
- [ ] `WIRE_VERSION`, `SNAPSHOT_VERSION` and the wire fixtures are unchanged. Committed SDK typedefs match the registry.
- [ ] `content/dev/scripts/player.ts` and the `movement-vault.ts`/`.luau` twins author dash and vault feel through `motion` and load.
### Manual
- [ ] In `vault-test`: a still waist-high mantle reads as a subtle nudge, a sprint-jump catch as a hard snap and lean, a long fall into a ledge heaviest. A pool exit stays gentle.
- [ ] With expressions pushed absurdly high, the camera looks exaggerated but never flips or tears.
- [ ] Reduced motion on: no vault pose or kick, whatever the entry.

## Path

- Precedents: dash's `NumberOrIr` fields, `DashPrograms` binding at component construction, and `resolve_number`'s clamp (`physics::movement::intents::dash`); `foundation::movement::scope::MovementScope` (indexed inputs, `for_validation`); `sdk/lib/activation.ts` (frozen fluent namespace); `sdk/lib/util/expression_refs.ts` (`numberNode`/`boolNode`, whose WeakMap is why lowering must happen SDK-side); `defineEntity`'s `lowerLoadoutReferences` in `sdk/lib/data_script.ts` and `.luau`.
- Seams: `VaultViewParams` and the vault `ImpulseStateParams` take `NumberOrIr` (`ViewFeelParams` is serde; bind where view feel is driven, not in the descriptor); `scripting-core` `movement_view_feel` parsers; `postretro::view_feel::vault::evaluate` and `view_feel::impulse`; `frame_eye::ViewFeelDriver`, which already carries velocity and grounded per frame; `frame_loop::presented_vault`, which reads the payload.
- Shape chosen: expression-valued fields plus a shared `motion` vocabulary. Strongest rival: a charge-style per-channel `scale` sidecar. It reads as base times multiplier, but it cannot shape timing or split the enter and exit kicks, and it would duplicate the fields once `pose.vault` widens. Research: `research.md`.
- First slice: `pitch` from `motion.speed` on the dev player in `vault-test`. It falsifies the riskiest assumption, that a frozen entry value reads as a different catch rather than as noise.
- `view_feel.rs` sits past 1300 lines. Put the tracker and the vault evaluation beside `vault.rs`, and keep `view_feel.rs` to wiring.
- Update `docs/scripting-reference.md`: its dash read-name table becomes the `motion` table with a per-field supported-inputs column. Add a carry-over note to `movement--state-view-pose`: the fold keeps the vault row's progress-share timing (`easeIn`/`easeOut`) as expression fields, since its rate-based rows would otherwise drop them.
- At promotion: `scripting.md` §11's adopter list gains this as the first presentation-side adopter (evaluated per frame, not per tick, and free of determinism constraints). `movement.md` §2's "no reads outside the movement component" line notes that view-feel inputs are render-side reads it does not govern.

## Boundary inventory

camelCase on script sides, snake_case in Rust. No FGD, no PRL, not on the wire. TypeScript and Luau both ship.

| Name | Rust | JS / TS | Luau |
|---|---|---|---|
| namespace | IR input names, bound per field site | `motion` | `motion` |
| dash inputs | `speed`, `verticalSpeed`, `grounded`, `chargesRemaining`, `cooldownMs`, `elapsedMs` (existing read names) | same, `NumberRef` / `BoolRef` | same |
| vault view inputs | `speed`, `verticalSpeed`, `grounded` (shared names), `swimming`, `airborneMs`, `climbHeight` (new) | same | same |
| vault pose fields | `VaultViewParams` `pitch`/`roll`/`fov`/`ease_in`/`ease_out` as `NumberOrIr` | `viewFeel.vault.*` | same |
| vault kick fields | vault `ImpulseStateParams` `tension`, `enter`, `exit` channels as `NumberOrIr` | `impulse.states.vault.*` | same |

## Open questions

- Where evaluation and bound programs live (`ViewFeelState` versus the driver); how the first frame with a vault is detected when the edge and the payload arrive on different frames; and whether the vault kick row gets its own type or non-vault rows reject IR at parse — **delegated**
- `check_ir` node and depth bounds for view-feel programs — **delegated**
