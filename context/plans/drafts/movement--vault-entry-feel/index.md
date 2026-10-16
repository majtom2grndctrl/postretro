# movement--vault-entry-feel

Brief · compact · reads: `context/lib/movement.md` §1 §2 §4 (Vault), `context/lib/scripting.md` §5 §11, `context/lib/player_options.md` §5 · read at 41f21c8a

## Problem

The project lead asked for vault view feel that answers how the vault began: a still step-up mantle should read as a subtle bob, a sprinting leap onto a ledge as a hard catch. This is a requested capability. The held pose (`viewFeel.vault`) and the vault kick (`impulse.states.vault`) take literals only, so every vault presents the same camera regardless of entry. When this is done, an author writes the pose angles and the kick fields as fluent expressions over how the pawn caught the ledge: horizontal speed, vertical speed, grounded or swimming, and climb height. A running leap across a chasm or a long fall reads hard; a tall jump caught at its apex reads as gently as a step-up. Each vault evaluates them once and keeps its values through its kicks and its release.

## Decisions

- **Fields take expressions directly; no scale sidecar.** The pose's `pitch`, `roll` and `fov`, and the vault kick row's `tension` and `enter`/`exit` channels, accept a literal or a numeric expression. This is the dash pattern (`movement.md` §2, `plans/done/M14--movement-dash-runtime-values`): a literal stays bit-identical, and the engine pins when the field evaluates. `easeIn` and `easeOut` stay literal: kick `tension` carries entry-dependent snap, and `movement--state-view-pose` times its rows by rates, not progress shares.
- **One `motion` vocabulary.** `motion` is a frozen namespace exported from `"postretro"`, as `activation` is. Its refs lower to the input names dash already reads, with the same meanings, and add `swimming` and `climbHeight`. A name means the pawn's motion at the field's evaluation moment, so `motion.speed` in a vault field is the speed the vault began with. Each field site binds only the names the Boundary inventory lists for it; any other name, or a result type that does not match the field, rejects the descriptor naming the field.
- **Not a live store slot.** A live `player.speed` would read the climb path's velocity once the vault has begun. Movement and charge expressions also bind closed scopes with no store reads (`scripting.md` §11). A readonly speed slot for HUD or crossings stays a separate door with no consumer yet.
- **`defineEntity` lowers fluent refs** in every expression field of `components.movement`, in TypeScript and Luau. Authors write refs with no builder, raw `runtime.*` IR keeps working, and dash fields accept `motion` refs as a consequence.
- **Each vault evaluates once, at its entry edge.** The movement tick reports each vault's entry facts with its entry edge, on host, offline and client-predicted ticks alike. The facts are taken from the pawn's total motion (base, knockback and drift) before entry empties those layers, as dash reads it. The record is local presentation output like the edge, never a component, payload or wire field. View feel consumes the frame's edges whether or not an `impulse` block exists. The enter kick, the pose, the exit kick and the release all use that one evaluation.
- **Neutral facts when a vault has no evaluation.** A vault the camera shows or ends without its entry edge having evaluated (a reconcile join, or a view-feel reset that cleared the evaluation) evaluates once from neutral facts: payload speed and climb height when present, otherwise 0, grounded, not swimming, no vertical speed. A progress rewind keeps the vault's evaluation; only an entry edge starts a new vault.
- **Catch speed, not hang time.** Intensity comes from the pawn's motion into the ledge, which `speed` and `verticalSpeed` carry exactly. Time airborne is no input: a tall jump or a launch caught at its apex hangs long and arrives slowly. Every vault input is therefore an exact fact from the tick.
- **Results follow dash: clamp silently to the literal range.** Pose angles clamp to ±90°, kick channels to ±180° and then the impulse `max`, and kick `tension` to [0.1, 240].
- **Overlap keeps the shipped weight and blends values.** A vault that starts while a release is running presents the larger of the two weights, as `movement.md` §1 ships, with the two value sets blended by their weights. A later release starts from the pose as presented. Literal-only descriptors present exactly as today. The vault kick spring is shared, so the newest evaluation sets its `tension`.
- **Vault fields only.** Other kick rows, the shared impulse `tension` and `max`, and the other view-feel blocks stay literal and reject expressions. Each extends by pinning its own evaluation moment when asked. The grounded and swimming inputs replace the per-transition map `plans/done/movement--state-transition-feel` named as its extension point: one row reads how it was entered instead of keying rows by source state. This also chooses differently from `player-descriptor-composition`, which calls branching an expression on `grounded` a workaround: for a presentation response to one transition, the branch is the shape.
- **Presentation and accessibility unchanged.** `view_feel_scale` and reduced motion multiply the evaluated output (`player_options.md` §5), and evaluation happens at every scale. View feel stays out of the tuning payload, so co-op is untouched.
- **Builds after `movement--state-view-pose`.** That brief moves the held pose into `pose.vault` and settles its timing model. This brief's pose fields, overlap rule and release-tied evaluation lifetime rebase onto it before build; the vocabulary, entry record, lowering and kick fields do not depend on it.
- **Non-goals.** New dash inputs: `swimming` is always false in a dash, which enters only from `Normal` or `Crouching`, and `climbHeight` means nothing outside a vault.

### Scripting surface

```typescript
import { defineEntity, motion } from "postretro";

// Speed into the ledge plus half of any downward speed; a rising or apex catch adds nothing.
const impact = motion.speed.plus(motion.verticalSpeed.clamp(-40, 0).times(-0.5));
const hard = impact.dividedBy(11).clamp(0.25, 1.5); // step-up 0.25, running leap ~1, long fall up to 1.5
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
- [ ] TypeScript and Luau load the dev player with the Scripting surface example. Both reject the same cases, naming the field path: an unknown input name; `climbHeight` or `swimming` in a dash field; `chargesRemaining`, `cooldownMs` or `elapsedMs` in a vault field; a Bool expression in a number field; an expression in `easeIn` or `easeOut`, on a non-vault kick row, in the shared impulse `tension` or `max`, or in any other view-feel block; a fluent ref in a descriptor not built through `defineEntity`.
- [ ] A descriptor with only literals presents pose and kick output bit-identical to the same descriptor before this change, overlapping vaults included.
- [ ] A vault field written as raw `runtime.*` IR presents the same as its `motion` spelling. Dash with `motion.grounded.select(0.4, 0.7)` behaves identically to `runtime.select(runtime.read("grounded"), 0.4, 0.7)`, grounded and airborne.
#### Inputs
- [ ] A grounded vault reads `grounded` true and `swimming` false; an air catch reads `grounded` false.
- [ ] A fall into a ledge reads negative `verticalSpeed` and a catch on the rise positive, though the climb rises from the vault's first tick. Knockback and drift count, as in dash. (P2, P23)
- [ ] With the example's expression, a straight-up tall jump caught at its apex presents the same as a still step-up, and a running leap caught on the way down presents harder than either.
- [ ] A swim exit reads `swimming` true.
- [ ] `speed` equals the entry speed, knockback and drift included. `climbHeight` equals the landing height minus the entry feet, on a static ledge and on a moving mover.
- [ ] Offline, host and client-predicted vaults all evaluate from their real entry facts, never neutral ones. (P18)
#### Evaluation timing
- [ ] The first frame of a vault kicks with that vault's values, never the previous vault's or zero, including the first vault after load and a vault entered on the frame a descriptor refreshes. (P1, P19)
- [ ] Pose expressions with no `impulse` block evaluate each vault from its own entry. Vault kick expressions with no held pose kick each vault with its own values. (P11, P16)
- [ ] Motion that changes during the climb does not change presented values, and a reconcile that shifts progress with no new entry edge keeps them. The release after the climb, and an early end's release, keep them. (P3, P6)
- [ ] Two back-to-back vaults with different entries each present their own values: when the first's exit and the second's entry land in one frame, when two vaults begin in one frame's ticks, and when a vault blocked early is followed at once by another. (P4, P20, P21)
- [ ] A vault that starts during a release, or three vaults in quick succession, present no step beyond the new vault's own ease-in. (P12, P22)
- [ ] A vault whose entry and exit both land in one frame, never shown, still kicks with its own values. (P13)
- [ ] A vault entered while the previous exit kick still settles: from the new entry edge, the remaining kick settles at the new vault's evaluated tension. (P15, P25)
- [ ] A vault the camera joins after a reconcile evaluates once from neutral facts, with no enter kick. A refresh on the frame a vault ends gives its exit kick neutral facts. A reconcile that ends one vault silently, then a predicted vault, gives the second its own evaluation and the first no exit kick. On single-player and host, a hot reload mid-vault releases the pose with its evaluation. (P5, P7, P14, P24, P26)
#### Bounds
- [ ] Expressions evaluating far past every range present clamped values: pose within ±90°, kicks within ±180° and `max`, `tension` within [0.1, 240]. An expression dividing by an input that reads 0 presents a finite, clamped value. Composed render pitch stays within ±89° as a regression guard.
#### Regression guards
- [ ] `view_feel_scale` 1, 0.5 and 0 (reduced motion) present full, half and zero. Restoring the scale mid-vault resumes with no snap and shows the values the vault began with. (P10)
- [ ] The viewmodel transform is unchanged by the evaluated pose.
- [ ] `WIRE_VERSION`, `SNAPSHOT_VERSION` and the wire fixtures are unchanged. Committed SDK typedefs match the registry. Grep gates: no new movement-component or vault-payload field, no player speed store slot, and no render-side motion tracker.
- [ ] `content/dev/scripts/player.ts` and the `movement-vault.ts`/`.luau` twins author vault feel through `motion` and load.
### Manual
- [ ] In `vault-test`: a still waist-high mantle reads as a subtle nudge, a running leap across a gap as a hard snap and lean, a long fall into a ledge heaviest. A tall jump caught at its apex and a pool exit stay gentle.
- [ ] With expressions pushed absurdly high, the camera looks exaggerated but never flips or tears.
- [ ] Reduced motion on: no vault pose or kick, whatever the entry.

## Path

- Precedents: dash's `NumberOrIr` fields, `DashPrograms` binding at component construction, and `resolve_number`'s clamp (`physics::movement::intents::dash`); `foundation::movement::scope::MovementScope` (indexed inputs, `for_validation`); `sdk/lib/activation.ts` (frozen fluent namespace); `loweredValue` in `sdk/lib/util/expression_refs.ts` and its Luau twin (IR hidden from the parser, so lowering is SDK-side); `defineEntity`'s `lowerLoadoutReferences` as the existing lowering pass to sit beside.
- Seams: `physics::movement::vault::vault_entry` sees total motion before entry peels the layers; `MovementEvents` already carries local per-tick data (`swim_fluid`, `landed`); offline and host pass only state edges to view feel while client prediction passes full events, so the record needs a path on both; `frame_loop` queues every tick's edges in order as `TimedMovementEdge`; edges are consumed inside `view_feel::impulse` today, before `view_feel::vault::evaluate`; `VaultViewParams` and the vault `ImpulseStateParams` take `NumberOrIr` (`ViewFeelParams` is serde, so bind where view feel is driven); `scripting-core` `movement_view_feel` parsers.
- Shape chosen: expression-valued fields, a shared `motion` vocabulary, evaluation at the entry edge, and weight-blended overlap. Rivals: `research.md`.
- First slice: `pitch` from `motion.speed` on the dev player in `vault-test`. It falsifies the riskiest assumption, that a frozen entry value reads as a different catch rather than as noise.
- `view_feel.rs` is large. Put the evaluation records beside `vault.rs`, and keep `view_feel.rs` to wiring.
- Update `docs/scripting-reference.md`: its dash read-name table becomes the `motion` table with a per-field supported-inputs column. Add a note to `movement--state-view-pose` that `pose.vault`'s angle fields accept `motion` expressions and that its single row weight keeps the value blend, so a quick re-vault does not step.
- At promotion: `scripting.md` §11's adopter list gains the first presentation-side adopter, evaluated per edge, not per tick, and free of determinism constraints. `movement.md` §1 records the local entry record and the value blend under overlap. `movement.md` §2's "no reads outside the movement component" line notes that view-feel inputs are presentation reads it does not govern.

## Boundary inventory

camelCase on script sides, snake_case in Rust. No FGD, no PRL, not on the wire. TypeScript and Luau both ship.

| Name | Rust | JS / TS | Luau |
|---|---|---|---|
| namespace | IR input names, bound per field site | `motion` | `motion` |
| dash inputs | `speed`, `verticalSpeed`, `grounded`, `chargesRemaining`, `cooldownMs`, `elapsedMs` (existing read names) | same, `NumberRef` / `BoolRef` | same |
| vault inputs | `speed`, `verticalSpeed`, `grounded` (shared names), `swimming`, `climbHeight` (new) | same | same |
| vault pose fields | `VaultViewParams` `pitch`/`roll`/`fov` as `NumberOrIr` | `viewFeel.vault.*` | same |
| vault kick fields | vault `ImpulseStateParams` `tension`, `enter`, `exit` channels as `NumberOrIr` | `impulse.states.vault.*` | same |
| entry record | local movement-event output beside the vault entry edge; not a component field, not on the wire | none | none |

## Open questions
- Execution: switch to resumable with two phases? Recommend yes. Phase 1 (the `motion` vocabulary, the entry record, the `defineEntity` lowering and the vault kick fields) has no dependency on `movement--state-view-pose` and can build now; phase 2 (pose angle expressions, overlap) waits for it. The kick carries most of the hard-catch feel, and the vocabulary is a modder foundation whose first consumers ship with it (`research.md` §Product fit). — owner: project lead — **blocks build**
- Rebase the pose fields, the overlap rule and the pins written against the shipped progress-based block (P12, P22, P24) onto `pose.vault` once `movement--state-view-pose` lands — **blocks build**
- Where evaluation records and bound programs live, how a record is tied to its vault for the exit kick and the release, and whether the vault kick row gets its own type or non-vault rows reject IR at parse — **delegated**
- `check_ir` node and depth bounds for view-feel programs — **delegated**
