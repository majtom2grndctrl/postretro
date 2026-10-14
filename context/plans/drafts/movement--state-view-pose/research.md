# movement--state-view-pose — research

Read at c06cd6c. Symbols, not lines.

## Grounded facts

| Claim | Source |
|---|---|
| Impulse: one critically damped spring per state key. Edges add a displacement that relaxes to zero at the row's `tension`. Swim has no key. | `view_feel::impulse::{evaluate, state_index, IMPULSE_STATE_COUNT}` |
| Slide sustained: one blend scalar, `1-exp(-rate·dt)` toward 1 while `Slide`, toward 0 otherwise. Multiplied by scale after the integrator advances. | `view_feel::slide::evaluate` |
| Composition: bob, tilt, sway and impulse go through `map_output_to_camera` (pitch + is up). Slide dip is subtracted after bob, floored at 0.05 m clearance. FOV = impulse + slide. | `frame_eye::assemble_frame_eye` |
| `RenderCamera::new` adds pitch offset to aim pitch with no clamp. `Camera::rotate` clamps aim to ±89° (`PITCH_LIMIT`). | `camera.rs` |
| Tick input and aim read `camera.pitch`, not the render camera. | `MovementInput.facing_pitch` in `main.rs` input build; `host_aim_pitch` in `net_serialize_and_send` |
| Viewmodel re-applies view-feel yaw/pitch/roll and eye offset in camera space. | `viewmodel_camera_space_transform` |
| The driver carries only `movement_state.kind()`, velocity, grounded and eye clearance. | `frame_eye::ViewFeelDriver`, built in `frame_loop` Eye stage |
| View feel follows the local pawn only. | `followed_player_pawn` → `EntityRegistry::local_player_movement_pawn` |
| `view_feel` is excluded from the tuning payload, so it stays local. | `netcode::movement_state::movement_state_to_wire` doc comment |
| Vault payload: `target` = landing feet, `start_offset` = entry feet − target, both in mover-local space when `mover` is set. Path: a smoothstep rise that the forward move overlaps near its end, over whole ticks (`vault_path_ticks`), ending at the entry speed. `elapsed_ms` is whole ticks (`vault_clock`); `vault_path_ms` is the path's own length. | `MovementState::Vault`; `physics::movement::vault_path::VaultPath` (`crates/physics/src/movement/vault_path.rs`); `physics::movement::vault::VaultFrame`; public `physics::movement::{vault_clock, vault_path_ticks, vault_path_ms}` |
| Vault frames are upright within ~0.8°, so the vertical and horizontal-magnitude parts of frame-local offsets hold in world space. | `UPRIGHT_MIN_UP_Y`, `VaultFrame::is_upright` |
| Vault rides the wire, so the client predicts it and a remote pawn's state replicates. | `WireMovementState::Vault`, `netcode::movement_state` |
| Unknown top-level `viewFeel` keys are ignored, not rejected. | `view_feel_params_from_js` / `_lua` probe known keys only |
| Dev player: eye 1.5 m above feet (`halfHeight` 0.8, `radius` 0.2, `eyeHeight` 0.5), `vault.maxHeight` 1.6 m, `durationMs` 420. | `content/dev/scripts/player.ts` |

## The stopgap

Commit c06cd6c sets `impulse.states.vault = { tension: 6, enter: { pitch: -4 } }`. A critically damped return from x0 follows x0·e^(−kt)(1+kt). At k = 6:

| t | share left | pitch |
|---|---|---|
| 0.20 s (mid-climb) | 0.66 | −2.7° |
| 0.42 s (exit) | 0.28 | −1.1° |
| 0.70 s | 0.08 | −0.3° |

So it decays throughout the climb and lingers after landing. It is the same −4° for every ledge, and a reconcile that ends the vault fires no edge, so the kick runs on.

## Paths

| | 1 Per-state literal pose | 2 Vault look-at block | 3 Impulse hold | 4 Pose rows + state terms (chosen) |
|---|---|---|---|---|
| Author writes | `pose.vault.pitch: -12` | `vaultFocus: { strength }` | `vault.enter.holdMs: 400` | `pose.vault.ledgeFocus` and/or literals |
| Level | low: author picks degrees | high, single-purpose | low | both |
| Owner ask | "at least look down" | "focus the ledge" | neither, really | both |
| Composes | like slide | a new mechanism beside slide | inside impulse | replaces slide, beside impulse |
| Ends on silent exit | yes (state-driven) | yes | no (timer) | yes |
| Scales with ledge | no | yes | no | yes |
| Wall-run | roll has no wall-side sign | none | none | a row plus a wall-signed term |
| Cost | small | small | smallest | small to moderate (slide migration) |

Rejected:

- **3 Impulse hold.** It conflates transient and sustained. A fixed hold cannot track an early end or a mover, and the edge it waits on is missing when a reconcile ends the state.
- **1 alone.** It cannot follow geometry or sign roll by wall side. It ships as the literal half of 4.
- **2 alone.** A third sustained mechanism, vault-shaped, with no home for slide or wall-run.
- **Render-rate runtime IR on pose fields.** The most expressive option, but the movement IR namespace is tick-side, so render-rate evaluation is new infrastructure. Left open: number fields can widen to `NumberOrIr`.
- **Pose writes aim.** It would keep the reticle honest. But it writes tick input that is predicted and replicated, steals the mouse, and breaks render-only view feel.

## Focus geometry

Ledge line angle = atan2(top_y − eye_y, max(horizontal, floor)), clamped to ≤ 0. Without the floor, horizontal distance goes to 0 as the pawn arrives over the landing spot and the angle approaches −90°.

Illustrative dev numbers, assuming 0.6 m horizontal at entry:

| Ledge | At entry | Later |
|---|---|---|
| 1.0 m | 0.5 m below the eye, line ≈ −40° | — |
| 1.6 m | 0.1 m above the eye, contributes 0 | turns downward once the eye clears the top mid-rise |

`maxPitch` saturates low ledges almost at once and lets tall ledges dip only as they come into view below the eye.

Aim-referenced pull: offset = max(−maxPitch, strength · min(0, line − aim)).

- Aim at or below the line: zero, and mouse pitch maps 1:1.
- Above the line and unsaturated: d(view)/d(aim) = 1 − strength.
- Saturated: back to 1:1.

Rival: a horizontal-referenced dip (offset = strength · line, independent of aim). Always 1:1, but it stacks onto a player already looking down and never quite "focuses".

## Comparable games

DICE's Mirror's Edge interview: they expected simulation sickness from camera animation. A reticle fixed to screen centre jittered during it, so they tied the reticle partly to a distant focal point ([indianvideogamer interview](https://www.indianvideogamer.com/features/the-most-comprehensive-mirrors-edge-interview/333/amp); camera in the eyes to cut bob, [Engadget hands-on](https://www.engadget.com/2008-07-17-joystiq-e3-hands-on-mirrors-edge.html)). That is the reticle-versus-aim divergence this brief documents rather than solves.

The rest is from well-known behavior; web search did not confirm it:

- **Titanfall 2 / Apex mantle:** short (~0.3–0.5 s), the camera dips and lifts with the hands, mouse look stays live, no firing.
- **DOOM Eternal ledge grab:** very fast with a small dip, full look control.
- **Dying Light:** stronger head-follow camera on climbs, partly constrained look.
- **Ghostrunner and Titanfall wall-run:** sustained roll away from the wall for the run's length.
- **Dusk and Ultrakill:** no mantle camera.

Pattern: an additive, short, bounded offset with the mouse kept live is accepted widely. Forced or locked looks are tolerated only briefly. This supports a capped pull with reduced-motion suppression, not a lock.

## Doors

- **Wall-run:** a `wallRun` row plus a term signed toward the wall. The roadmap row already names vault seams to generalize.
- **Sustained FOV draft:** its slide row is superseded. Its `run` tier target and absolute-FOV unit are the remaining decisions, to be rebased onto `pose`. That needs owner disposition, outside this brief.
- **Composition draft:** `pose.<state>` moves to its state layer.
- **Spectator follow-cam:** evaluates from replicated payload, no wire change.
