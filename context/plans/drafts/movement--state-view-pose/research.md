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
| `clear_state_effects` resets impulse springs, slide blend and vault state with no release; called on a view-feel descriptor change and on a staged manifest commit that changed the followed pawn's movement. Hot reload is such a commit: it replaces the movement component, dropping dash, crouch, slide and vault to `Normal` (swim kept), so slide and vault snap today. Pawn change, no driver and level install reset all state. | `ViewFeelState::clear_state_effects`; `sync_view_feel_driver`; `invalidate_refreshed_view_feel_state_effects`; `refresh_plan` movement replace; `startup/lifecycle.rs`. `movement.md` §1 and the `vault::evaluate` doc claim a hot-reload release; both are stale |
| View feel follows the local pawn only. | `followed_player_pawn` → `local_player_movement_pawn` |
| `view_feel` is excluded from the tuning payload. `ViewFeelParams` is serde without `deny_unknown_fields`. | `movement_state_to_wire` doc comment; `foundation` `data_descriptors/types/movement.rs` |
| Vault payload: `target` = landing feet, `start_offset` = entry feet − target, mover-local when `mover` is set. `VaultPath` is `pub(super)` to physics movement. | `MovementState::Vault`; `physics::movement::vault_path::VaultPath` |
| Uprightness (within ~0.8°) is checked only at entry. Each tick re-resolves the mover frame with no check and follows the path through it, so a mover may tilt mid-vault. | `UPRIGHT_MIN_UP_Y`, `VaultFrame::is_upright` in `vault_entry`; `VaultFrame::resolve` in `intents::vault` |
| Unknown top-level `viewFeel` keys are ignored. Impulse rows skip inherited rows rather than reject them. A retired-key precedent exists: "`components.ai` has been retired; author `components.behavior` instead". | `view_feel_params_from_js`/`_lua`; `js/entity.rs`, `lua/entity.rs` |
| Dev player: eye 1.5 m above feet (`halfHeight` 0.8, `radius` 0.2, `eyeHeight` 0.5 above the capsule centre), `vault.maxHeight` 1.6 m, `durationMs` 800, `allowFire: false`; vault pose `pitch −38.5, roll 10, fov −30, easeIn 0.33, easeOut 0.33`. The twins `movement-vault.ts`/`.luau` author no view feel. | `content/dev/scripts/player.ts` |
| Human docs for both blocks live only in `docs/scripting-reference.md` (§Slide camera feel, §Vault camera pose, the accessibility table). | `docs/scripting-reference.md` |

## Why the vault row keeps progress timing

Rates approach the hold asymptotically and release after the state ends, so the pose would still be easing out once the pawn stands on the top. Progress shares reach exactly 0 on the tick the path reaches the top, which the owner's feel pass tuned for. The rate rows keep rates because slide, crouch, dash and swim have no known length.

## Overlap value blend

A release (weight w_r, values v_r held) and a new vault or a rewind (w_n, live values v_n) overlap:

presented = max(w_r, w_n) · (w_r·v_r + w_n·v_n) / (w_r + w_n), and 0 when both are 0.

With equal literals and no focus, this equals the shipped max(w_r, w_n) · v. A rewind past 5% (or two vaults with no `Normal` frame between them, which present only as a progress drop) is the most frequent overlap. The formula differs from shipped only when the value sets differ, which `ledgeFocus` makes routine (two ledges, two lines) and `movement--vault-entry-feel` makes routine again (two entries, two evaluations). Without the blend, a re-vault at a different ledge steps by roughly w_r · |v_r − v_n| on its first frame.

## Paths

| | 1 Per-state literal pose | 2 Vault look-at block | 3 Impulse hold | 4 Pose rows + state terms (chosen) |
|---|---|---|---|---|
| Author writes | `pose.vault.pitch: -12` | `vaultFocus: { strength }` | `vault.enter.holdMs: 400` | `pose.vault.ledgeFocus` and/or literals |
| Owner ask | "at least look down" (shipped, as `viewFeel.vault`) | "focus the ledge" | neither, really | both |
| Composes | one block per state | a third sustained block | inside impulse | replaces slide and vault blocks, beside impulse |
| Ends on silent exit | yes | yes | no (timer) | yes |
| Scales with ledge | no | yes | no | yes |
| Wall-run | roll has no wall-side sign | none | none | a row plus a wall-signed term |
| Cost | shipped | small | smallest | moderate (slide and vault migrations) |

Rejected:

- **3 Impulse hold.** It conflates transient and sustained. A fixed hold cannot track an early end or a mover, and the edge it waits on is missing when a reconcile ends the state.
- **1 alone.** It cannot follow geometry or sign roll by wall side. It ships as the literal half of 4.
- **2 alone.** A third sustained block, vault-shaped, with no home for other states.
- **Rates on the vault row.** Asymptotic, and not tied to the climb's end; see above.
- **Entry-aim reference.** Measure focus against aim at vault entry instead of live aim. Mouse pitch stays 1:1 for the whole climb, but the pull no longer stops at the line: a player who looks further down mid-climb gets it on top, bounded only by `maxPitch` and the ±89° clamp. It is the fallback if the first slice reads as a fought camera.
- **Pose writes aim.** It would keep the reticle honest. But it writes tick input that is predicted and replicated, steals the mouse, and breaks render-only view feel.

## Focus geometry

Ledge line = atan2(top_y − eye_y, max(horizontal, floor)) from the rendered eye (after bob and every row's eye drop), and no focus at all when top_y ≥ eye_y, with the landing spot carried into world space through the mover's presented pose when `mover` is set. Without the floor, horizontal distance goes to 0 as the pawn arrives over the landing spot and the angle approaches −90°.

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
- **Vault entry feel:** `movement--vault-entry-feel` widens `pose.vault` angles to `motion` expressions evaluated at the entry edge. It rebases its pose fields, overlap rule and pins P12, P22 and P24 onto `pose.vault`; its first Decision's line that this brief times rows by rates is stale for the vault row, and its P26 (hot reload releases the pose) is false against source.
- **Sustained FOV draft:** its slide row is superseded. Its `run` tier is input-derived inside `Normal` and targets an absolute FOV, so it does not fit a state row's additive offset; see the open question.
- **Composition draft:** `pose.<state>` moves to its state layer.
- **Spectator follow-cam:** evaluates from the replicated vault payload and that pawn's replicated aim, no wire change.

## Pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Row handoff: dash or swim into a vault; a swim edge pre-empting a land-entered vault. Crouch and slide never vault. | Kind changes on one tick. The leaving row eases out on its own timing (rate or release) while the entering row eases in on its own. | Rows add. Both present during the overlap. No frame steps. No single weight is handed between rows. |
| P2 | Two rows with `eyeDrop` at once (crouch↔slide, swim→vault). | Bob, then the summed drop of every row, then the 0.05 m floor once. | Eye stays at least 0.05 m above the feet. The floor bounds the sum, not each row. |
| P3 | Vault A releasing; vault B starts; B ends or aborts before A's release finishes. | B's end starts a new release while A's still runs. | The new release starts from the presented weight and blended values. No step on B's end frame. |
| P4 | View-feel scale 0 across a vault's end, restored mid-release. | Captured values are unscaled; scale multiplies output only. | After restore, the frame presents scale × release weight × captured values. Not neutral, not a restarted release. |
| P5 | Ledge focus during a release. | Geometry is gone after the end frame; the captured focus is an angle, not a line. | Focus is not re-evaluated against live aim. Mouse pitch maps 1:1 through the release. |
| P6 | Ledge focus on a full climb and on a joined vault. | Focus is computed, then scaled by the one row weight. | A join's first frame is near neutral. A full climb starts and ends at neutral. Focus never presents outside the row weight. |
| P7 | Ledge focus with an impulse kick, sway or another row active. | The line bounds aim + row literal pitch + focus; other terms compose after. | "Never past the line" holds for the row's reference view; rendered pitch may pass it by the other terms. |
| P8 | A mover tilts after vault entry. | Uprightness is checked at entry only; each tick re-resolves the frame. | Focus reads the landing spot through the mover's presented pose; no assumption of uprightness. |
| P9 | Hot reload or descriptor refresh mid-vault or mid-slide. | The commit replaces the movement component, then clears pose state on the same frame. | The pose snaps to neutral; no release runs. |
| P10 | Vault A ends and vault B starts with no frame presenting `Normal` between them; or a reconcile rewinds progress past 5%. | The driver reads only the latest tick, so B shows as a progress drop. | The drop captures A's presented values and blends them with B's; no step beyond B's ease-in, ledge focus included. |
| P11 | Hot reload mid-swim. | The commit keeps `Swim` and clears pose state. | The swim row snaps to neutral on the first frame after the commit and eases back in at `enterRate`. |
| P12 | Ledge focus on a moving mover. | The eye is assembled before movers are collected for render. | Focus reads the mover pose at the eye's own interpolation share for that frame, never the previous frame's. |
| P13 | Reconcile corrects a vault's landing target mid-climb. | Focus is a pure function of target, eye and aim. | Focus re-aims within one frame, with a step the size of the correction's angle. Accepted; no slew mechanism. |
