# movement--state-view-pose — research

Read at 3409c2fc. Symbols, not lines.

## Grounded facts

| Claim | Source |
|---|---|
| Impulse: one critically damped spring per state key. Edges add a displacement that relaxes to zero at the row's `tension`. Keys are a closed set without swim; unknown keys reject, inherited rows are skipped. | `view_feel::impulse::{evaluate, state_index, IMPULSE_STATE_COUNT}`; `validate_impulse_state_keys_js`/`_lua`, `has_own_impulse_state_key_js` |
| Slide sustained: one blend scalar, `1-exp(-rate·dt)` toward 1 while `Slide`, toward 0 otherwise, scaled after it advances. All four fields required; `fovIncrease` in [0, 90]; rates in [0.1, 240]. | `view_feel::slide::evaluate`; `SlideViewParams`, `slide_view_params_from_js`/`_lua` |
| Vault sustained: weight is a smoothstep envelope over presented progress, min'd with a join ramp from neutral and max'd with a release from the last weight over max(path_s × `easeOut`, 0.1 s). A progress drop past 5% reads as a new vault. Output is literal × weight × scale. | `view_feel::vault::{evaluate, envelope, MIN_RELEASE_S, RESTART_PROGRESS_DROP}` |
| Vault parse: all fields optional; `pitch`/`roll`/`fov` in [−90, 90], default 0; `easeIn`/`easeOut` in [0, 1], default 0.15/0.2, sum ≤ 1. Unknown and inherited keys are read, not rejected. | `vault_view_block::vault_view_params_from_fields`; `VaultViewParams` |
| Presented progress is over the path's own length, so it reaches 1 as the pawn reaches the top. | `VaultViewInput::presented`, `physics::movement::vault_path_ms` |
| Composition: bob, tilt, sway and impulse go through `map_output_to_camera`. Slide dip is subtracted after bob, floored at 0.05 m clearance. FOV = impulse + slide + vault. Vault pitch and roll are added only in the `RenderCamera::new` arguments, so `FrameEye.pitch_offset`/`roll` (the viewmodel's) exclude them. | `frame_eye::assemble_frame_eye`, `FrameEye` |
| `RenderCamera::new` clamps the composed pitch (aim + every view-feel term) to ±89°. Aim is clamped separately. | `camera.rs` `PITCH_LIMIT`, `RenderCamera::new`, `Camera::rotate` |
| Tick input and host aim read `camera.pitch`, never the render camera. | `MovementInput.facing_pitch` in `main.rs`; `host_aim_pitch` in `net_serialize_and_send` |
| The driver carries kind, velocity, grounded, eye clearance and `VaultViewInput { progress, path_s }`. No aim pitch, no vault payload. | `frame_eye::ViewFeelDriver`; `frame_loop` `presented_vault` |
| `clear_state_effects` resets impulse springs, slide blend and vault state; called on descriptor refresh and staged manifest commit. Pawn change, no driver and level install reset all state. | `ViewFeelState::clear_state_effects`; `sync_view_feel_driver`; `invalidate_refreshed_view_feel_state_effects`; `startup/lifecycle.rs` |
| View feel follows the local pawn only. | `followed_player_pawn` → `local_player_movement_pawn` |
| `view_feel` is excluded from the tuning payload. `ViewFeelParams` is serde without `deny_unknown_fields`. | `movement_state_to_wire` doc comment; `foundation` `data_descriptors/types/movement.rs` |
| Vault payload: `target` = landing feet, `start_offset` = entry feet − target, mover-local when `mover` is set. `VaultPath` is `pub(super)` to physics movement. | `MovementState::Vault`; `physics::movement::vault_path::VaultPath` |
| Vault frames are upright within ~0.8°, so vertical and horizontal-magnitude parts of frame-local offsets hold in world space. | `UPRIGHT_MIN_UP_Y`, `VaultFrame::is_upright` |
| Unknown top-level `viewFeel` keys are ignored. A retired-key precedent exists: "`components.ai` has been retired; author `components.behavior` instead". | `view_feel_params_from_js`/`_lua`; `js/entity.rs`, `lua/entity.rs` |
| Dev player: eye 1.5 m above feet (`halfHeight` 0.8, `radius` 0.2, `eyeHeight` 0.5 above the capsule centre), `vault.maxHeight` 1.6 m, `durationMs` 800, `allowFire: false`; vault pose `pitch −38.5, roll 10, fov −30, easeIn 0.33, easeOut 0.33`. The twins `movement-vault.ts`/`.luau` author no view feel. | `content/dev/scripts/player.ts` |
| Human docs for both blocks live only in `docs/scripting-reference.md` (§Slide camera feel, §Vault camera pose, the accessibility table). | `docs/scripting-reference.md` |

## Why the vault row keeps progress timing

Rates approach the hold asymptotically and release after the state ends, so the pose would still be easing out once the pawn stands on the top. Progress shares reach exactly 0 on the tick the path reaches the top, which the owner's feel pass tuned for. The rate rows keep rates because slide, crouch, dash and swim have no known length.

## Overlap value blend

A release (weight w_r, values v_r held) and a new vault (w_n, live values v_n) overlap:

presented = max(w_r, w_n) · (w_r·v_r + w_n·v_n) / (w_r + w_n), and 0 when both are 0.

With equal literals and no focus, this equals the shipped max(w_r, w_n) · v. It differs only when the value sets differ, which `ledgeFocus` makes routine (two ledges, two lines) and `movement--vault-entry-feel` makes routine again (two entries, two evaluations). Without the blend, a re-vault at a different ledge steps by roughly w_r · |v_r − v_n| on its first frame.

## Paths

| | 1 Per-state literal pose | 2 Vault look-at block | 3 Impulse hold | 4 Pose rows + state terms (chosen) |
|---|---|---|---|---|
| Author writes | `pose.vault.pitch: -12` | `vaultFocus: { strength }` | `vault.enter.holdMs: 400` | `pose.vault.ledgeFocus` and/or literals |
| Owner ask | "at least look down" (shipped, as `viewFeel.vault`) | "focus the ledge" | neither, really | both |
| Composes | one block per state | a third sustained block | inside impulse | replaces slide and vault blocks, beside impulse |
| Ends on silent exit | yes | yes | no (timer) | yes |
| Scales with ledge | no | yes | no | yes |
| Wall-run | roll has no wall-side sign | none | none | a row plus a wall-signed term |
| Cost | shipped | small | smallest | moderate (two migrations) |

Rejected:

- **3 Impulse hold.** It conflates transient and sustained. A fixed hold cannot track an early end or a mover, and the edge it waits on is missing when a reconcile ends the state.
- **1 alone.** It cannot follow geometry or sign roll by wall side. It ships as the literal half of 4.
- **2 alone.** A third sustained block, vault-shaped, with no home for other states.
- **Rates on the vault row.** Asymptotic, and not tied to the climb's end; see above.
- **Pose writes aim.** It would keep the reticle honest. But it writes tick input that is predicted and replicated, steals the mouse, and breaks render-only view feel.

## Focus geometry

Ledge line = atan2(top_y − eye_y, max(horizontal, floor)), clamped to ≤ 0. Without the floor, horizontal distance goes to 0 as the pawn arrives over the landing spot and the angle approaches −90°.

Reference view = aim + the row's literal `pitch`. Focus offset = max(−maxPitch, strength · min(0, line − reference)). Row output = weight · (pitch + focus), with roll and fov likewise weighted.

- Reference at or below the line: zero, and mouse pitch maps 1:1.
- Above the line and unsaturated: d(view)/d(aim) = 1 − strength.
- Saturated: back to 1:1.
- A literal that already reaches the line leaves no gap, so focus adds nothing; the shipped −38.5° hold would mostly do this, which is why the example drops it for focus.

Illustrative dev numbers, eye 1.5 m above feet, assuming 0.6 m horizontal at entry:

| Ledge | At entry | Later |
|---|---|---|
| 1.0 m | 0.5 m below the eye, line ≈ −40° | steepens toward the floor-bounded limit |
| 1.6 m | 0.1 m above the eye, contributes 0 | turns downward once the eye clears the top mid-rise |

`maxPitch` saturates low ledges almost at once and lets tall ledges dip only as they come into view below the eye.

Rival: a horizontal-referenced dip (offset = strength · line, independent of aim). Always 1:1, but it stacks onto a player already looking down and never quite focuses.

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

- **Wall-run:** a `wallRun` row plus a term signed toward the wall.
- **Vault entry feel:** `movement--vault-entry-feel` widens `pose.vault` angles to `motion` expressions evaluated at the entry edge. It rebases its pose fields, overlap rule and pins P12, P22 and P24 onto `pose.vault`; its first Decision's line that this brief times rows by rates is stale for the vault row.
- **Sustained FOV draft:** its slide row is superseded. Its `run` tier target and absolute-FOV unit are the remaining decisions, to be rebased onto `pose` by owner disposition, outside this brief.
- **Composition draft:** `pose.<state>` moves to its state layer.
- **Spectator follow-cam:** evaluates from replicated payload, no wire change.
