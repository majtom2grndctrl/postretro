# movement--vault-entry-feel

Brief · compact · reads: `context/lib/movement.md` §1 §4 (Vault), `context/lib/scripting.md` §5, `context/lib/entity_model.md` §Components (Weapon vocabulary), `context/lib/player_options.md` §5 · read at 41f21c8a

## Problem

The project lead asked for vault view feel that answers how the vault began: a still step-up mantle should read as a subtle bob, a sprinting leap onto a ledge as a hard catch. This is a requested capability. The held pose (`viewFeel.vault`) and the vault kick (`impulse.states.vault`) take literal channels only, so every vault presents the same camera regardless of entry. When this is done, an author scales the pose and the kick per channel with expressions over the vault's entry conditions (horizontal speed, climb height, time airborne, vertical speed, and the context it started from), written in the same fluent style as charge-scaled shots. Each vault evaluates once and keeps that result through its release.

## Decisions

- **Syntax mirrors charge shots.** A frozen `vaultEntry` namespace holds read-only fluent inputs and two builders: `vaultEntry.pose(...)` returns the `viewFeel.vault` record and `vaultEntry.kick(...)` returns the `impulse.states.vault` row. Each builder converts fluent refs to raw IR, as `activation.shot()` does (`plans/done/E16--weapon-activations`). A raw `scale` holding literals or `runtime.*` IR is accepted without a builder, as `ShotScaleDescriptor` is.
- **`scale` is per channel: `pitch`, `roll`, `fov`.** Each field is optional, defaults to 1, and takes a literal or numeric IR. One expression shared across fields covers the single-intensity case. On a kick, `scale` multiplies both the `enter` and `exit` channels.
- **Scale domain is [0, 64], charge's domain.** It is wide on purpose, so authors find the right amount by overshooting. The final scaled channel clamps to the existing numeric bands: ±90° for pose channels; ±180° per kick channel, then the impulse `max`; ±89° on composed render pitch (`movement.md` §1). Those bands keep the camera numerically stable. They are not taste limits.
- **Inputs, closed scope.** Inputs are `speed` (m/s, horizontal speed at entry, knockback and drift included), `height` (m the feet rise to the top), `airborneMs` (time in airborne `Normal`/`Dash` before the vault; grounded or swimming resets it; 0 for a ground or swim entry), `verticalSpeed` (signed m/s before the vault, negative falling), and `fromAir`/`fromGround`/`fromSwim` (bools, the `movement.md` §4 entry contexts). Any other name, or a Bool expression in a scale field, fails to bind and rejects the descriptor naming the field. The scope has no store reads, writes or randomness, as charge's has none.
- **Render-side sources; no tick, wire or payload change.** `speed` and `height` derive from the vault payload the local pawn already carries (`MovementState::Vault { entry_speed, start_offset }`; frames are upright, so the offset's vertical part is true vertical). `airborneMs`, `verticalSpeed` and the context are tracked by view-feel state from the driver's per-frame pawn facts while not vaulting, and frozen at evaluation. Up to one tick of staleness is accepted. Placement follows `movement.md` §1: view feel reads the pawn and writes nothing back. Carrying these facts in the payload would bump the wire for a presentation effect.
- **One evaluation per vault, held through its release.** It runs on the first frame that shows the vault, through its entry edge or a pose join (a reconcile or a view-feel reset that shows it in progress, or a rewind read as a new vault). The pose and both kicks of that vault use the one result. A later vault evaluates again.
- **Failure follows charge.** A bad literal or structure rejects the descriptor in both runtimes. A non-finite or out-of-domain evaluated value zeroes that channel for that vault and warns once per descriptor and field.
- **`scale` is valid on the `vault` impulse row only.** Other rows reject it naming the field. No other state has entry inputs; `ledgeFocus` in `movement--state-view-pose` sets the vault-only-term precedent.
- **Presentation and accessibility unchanged.** `view_feel_scale` and reduced motion multiply after `scale` (`player_options.md` §5). The viewmodel still never receives the held pose. View feel stays out of the tuning payload, so co-op is untouched.
- **Built on the shipped `viewFeel.vault`.** `movement--state-view-pose` plans to fold it into `pose.vault`. This brief adds a note there that `scale` and `vaultEntry` carry over unchanged.
- **Non-goals.** Entry inputs for slide, dash or other states: the same shape extends there when asked. Inputs that change mid-climb: values freeze at entry, as charge freezes at release. Remote pawns: view feel follows only the local pawn.

### Scripting surface

```typescript
import { vaultEntry } from "postretro";

// ~1 at run speed, up to +0.5 more for a long fall; a still step-up stays a nudge.
const hard = vaultEntry.speed.dividedBy(11)
  .plus(vaultEntry.airborneMs.dividedBy(600).clamp(0, 0.5))
  .clamp(0.25, 1.5);

viewFeel: {
  vault: vaultEntry.pose({
    pitch: -6, roll: 2, fov: -3, easeIn: 0.15, easeOut: 0.2,
    scale: { pitch: hard, roll: vaultEntry.fromAir.select(hard, 0), fov: hard },
  }),
  impulse: {
    tension: 12,
    max: { fov: 20, pitch: 8, roll: 6 },
    states: {
      vault: vaultEntry.kick({
        tension: 14,
        enter: { fov: -2, pitch: -4, roll: 0 },
        scale: { pitch: hard, fov: hard },
      }),
    },
  },
},
```

Luau ships the same surface: `local hard = vaultEntry.speed:dividedBy(11):plus(...)`, and `vaultEntry.pose({ ... })` and `vaultEntry.kick({ ... })` take the same tables. Builder arguments are exactly the existing record fields plus `scale`. Unknown keys reject.

## Acceptance

### Automated
- [ ] TypeScript and Luau load the Scripting surface example. Both reject the same cases, naming the field path: a literal scale below 0, above 64 or non-finite; an unknown input name; a Bool expression in a scale field; an unknown `scale` key; `scale` on a non-vault impulse row; an unknown builder key.
- [ ] Scale 1 everywhere, and an omitted `scale`, present pose and kick output bit-identical to the same descriptor without this feature.
- [ ] Scale 0 on a channel presents nothing on that channel while the others move.
#### Inputs
- [ ] A grounded vault reads `airborneMs` 0 and `fromGround` true; the other context flags read false.
- [ ] An air catch after a known airborne time reads that time within one tick. A long fall into a ledge reads negative `verticalSpeed`; a catch on the rise reads positive.
- [ ] A swim exit reads `airborneMs` 0 and `fromSwim` true, even after a long time in the water.
- [ ] `speed` matches the payload's entry speed. `height` matches the landing height minus the entry feet, on a static ledge and on a moving mover.
#### Evaluation timing
- [ ] Inputs that change mid-climb do not change the presented scale. The release after the climb, and an early end's release, keep it.
- [ ] Two back-to-back vaults with different entries each present their own scale.
- [ ] A vault the pose joins in progress (reconcile, mid-vault view-feel reset) evaluates once at the join, with no enter kick.
- [ ] The enter kick and exit kick of one vault use the same evaluation as its pose.
#### Failure and bounds
- [ ] An expression that evaluates non-finite or outside [0, 64] zeroes that channel for that vault and warns once per descriptor and field, never per frame. The other channels are unaffected.
- [ ] A scale of 64 on large channels stays within ±90° for the pose, the kick's ±180° and `max`, and ±89° composed render pitch. The view never flips.
#### Regression guards
- [ ] `view_feel_scale` 1, 0.5 and 0 (reduced motion) present full, half and zero, after `scale`. Restoring the scale mid-vault resumes with no snap.
- [ ] The viewmodel transform is unchanged by the scaled pose.
- [ ] `WIRE_VERSION`, `SNAPSHOT_VERSION` and the wire fixtures are unchanged. Committed SDK typedefs match the registry.
- [ ] `content/dev/scripts/player.ts` and the `movement-vault.ts`/`.luau` twins author their vault feel through `vaultEntry` and load.
### Manual
- [ ] In `vault-test`: a still waist-high mantle reads as a subtle nudge, a sprint-jump catch as a hard snap and lean, and a long fall into a ledge heaviest. A pool exit stays gentle.
- [ ] With scale pushed to an absurd value, the camera looks exaggerated but never flips or tears.
- [ ] Reduced motion on: no vault pose or kick, whatever the entry.

## Path

- Precedents: `sdk/lib/activation.ts` and `activation.luau` (namespace, builder, `numberRef`/`numberNode` conversion); `foundation::weapon_activation::compiled` (`ActivationScope`, `CompiledScalar`, `check_ir` bounds, warn-once); `foundation::movement::scope::MovementScope` (indexed multi-input scope, Bool inputs, `for_validation` binding at install).
- Seams: `scripting-core` `movement_view_feel` parsers (`vault_view_params_from_js`, `optional_impulse_state_from_js`, and their Lua twins); `foundation` `VaultViewParams` and `ImpulseStateParams` take `NumberOrIr` scale fields (`ViewFeelParams` is serde, so store IR there and bind where view feel is driven); `postretro::view_feel::vault::evaluate` and `view_feel::impulse` apply scale; `frame_eye::ViewFeelDriver` already supplies velocity and grounded per frame; `frame_loop::presented_vault` reads the payload.
- Shape chosen: per-channel `scale` plus builders, mirroring charge. Strongest rival: one `intensity` multiplier. It is simpler, but it is a second vocabulary beside charge's and cannot keep roll gentle while pitch hits hard.
- First slice: pose `scale` from `speed` alone on the dev player in `vault-test`. It falsifies the riskiest assumption, that a frozen entry scale reads as a different catch rather than as noise.
- `view_feel.rs` sits past 1300 lines. Put the entry tracker and scale evaluation beside `vault.rs`, and keep `view_feel.rs` to wiring.
- Update `docs/scripting-reference.md` beside its view-feel section, and add the carry-over note to `movement--state-view-pose`.

## Boundary inventory

camelCase on script sides, snake_case in Rust. No FGD, no PRL, not on the wire. Both TypeScript and Luau ship.

| Name | Rust | JS / TS | Luau |
|---|---|---|---|
| namespace | input names bound by a view-feel entry scope | `vaultEntry` | `vaultEntry` |
| inputs | `speed`, `height`, `airborneMs`, `verticalSpeed`, `fromAir`, `fromGround`, `fromSwim` (IR input names) | same, `NumberRef` / `BoolRef` | same |
| builders | none (SDK only) | `vaultEntry.pose`, `vaultEntry.kick` | same |
| pose scale | `VaultViewParams::scale` (pitch, roll, fov as `NumberOrIr`) | `viewFeel.vault.scale` | same |
| kick scale | `ImpulseStateParams::scale`, vault row only | `impulse.states.vault.scale` | same |

## Open questions

- Where evaluation and bound programs live (`ViewFeelState` versus the driver), and how the first frame with a vault is detected when the entry edge and payload arrive on different frames — **delegated**
- Exact `check_ir` node and depth bounds for view-feel scale programs — **delegated**
