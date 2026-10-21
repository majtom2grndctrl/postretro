# viewmodel-motion-layer

Parked `/draft-session` handoff — not yet a brief. Resume with `/draft-brief` from this handoff. Recheck the verified facts if source has moved past `Read at`.

**Prerequisite:** `coop--host-switch-timing` (drafts) (host honours `lowerMs`/`raiseMs` for remote pawns, closing the drop-while-lowering gap) must land before this item flips the `lowerMs`/`raiseMs` default. That item has its own draft session.

## Handoff
```markdown
Problem: The first-person viewmodel never moves relative to the camera. `viewmodel_camera_space_transform` (main.rs) composes only `WeaponPlacementDescriptor` with the same view-feel offsets the camera gets, and `collect_viewmodel` (mesh_render.rs) draws it rigid. So nothing on screen shows weapon state, look, firing or landing — it reads as Doom II. The most visible case is switching: no shipped weapon authors `lowerMs`/`raiseMs` (both default 0), so a switch is a same-tick model swap. Even authored durations would be invisible, because the transform never reads `WieldableState::Lowering`/`Raising`.
Outcome: A render-side viewmodel motion layer with four channels — lower/raise (weapon switch, and vault while it blocks fire), look lag, fire kick, landing kick — tuned per weapon through a reusable profile, scaled by view-feel scale and reduced motion. Every weapon gets it with no art. Switching takes a visible non-zero time by default.
Read at: 3197d201
Verified facts: (by symbol)
  - No `lowerMs`/`raiseMs` under content/. `WeaponDescriptor.lower_ms/raise_ms` (foundation combat.rs) are `#[serde(default)] u32`. Both parser twins deserialize the whole weapon with serde (`weapon_descriptor_from_js`, `weapon_descriptor_from_lua`), so a serde default fn covers both. The host-resolved value reaches clients through `WieldableTuningPayload` (combat-model carried_loadout.rs), whose `weapon_descriptor()` is a struct literal.
  - Switch flow: `run_local_weapon_command_inner` (sim weapon_stage/commands/local.rs). BeginLower targets the active weapon. On `machine.lowered`, `finish_lowering`, `begin_raising(incoming)` and `active_slot = target` happen in one tick, and the incoming cooldown is clamped to >= raise_ms. (Raising, BeginLower) restarts the full lower. A third-slot press only retargets `switch_target`. Pressing the origin slot is rejected. With lower_ms 0 everything finishes in the select tick. A raise with raise_ms 0 has total 0 for one tick.
  - `refuse_local_switch` / `normalize_inventory_liveness` (commands.rs) snap Lowering/Raising to Idle. A client refusal can move the active slot back.
  - Spawn, respawn, level change, loadout restore, pickup auto-switch and `release_wieldable` (wieldable_inventory.rs) change the active weapon with no Raising. Only local.rs calls `begin_raising`.
  - Connected client: `materialize_net_local_wieldable_inventory_from_tuning` builds real WeaponComponents. The chain `simulate_client_wieldable_tick` → `run_client_weapon_equip` → `tick_weapon_equip_only` advances lower/raise locally. The switch is predicted, then `client_declare_switch` sends it (WIRE_VERSION 26 lane). The "intentionally has no weapon entity" comments on `collect_viewmodel` and `viewmodel_asset_for_archetype` are stale except during the archetype-fallback window before the inventory exists.
  - `local_viewmodel_asset` (main.rs) returns the active weapon EntityId where `collect_viewmodel` is called (frame_loop/mod.rs), so its WeaponComponent is readable. The archetype fallback has none.
  - Timers are tick-rate only (`advance_timer`, reload.rs). No previous-tick copy is kept.
  - `VaultViewInput::presented` presents progress `(1 − alpha)` of a tick behind. The `VaultViewState` integrator (postretro view_feel/vault.rs) provides a join ramp, rewind detection (>5% drop), release from the last presented weight, and max on overlap. It resets and outputs nothing when `viewFeel.vault` is absent.
  - movement.md §1: the viewmodel never receives the held vault pose.
  - `vault_blocks_fire` (sim weapon/pawn_gate.rs) gates fire without entering Lowering. `allowFire: true` never closes it. `vault_spares_activation` keeps an opted-out charge charging through the gate.
  - Frame inputs at eye assembly (`ViewFeelDriver`, `FrameEyeInputs`, frame_eye.rs): velocity, grounded, movement state, vault, `TimedMovementEdge`s, presented aim, frame_dt and scale. On a connected client, edges come from forward prediction only, and reconcile replay emits none.
  - No per-frame look delta reaches the viewmodel. `look.yaw_delta/pitch_delta` live only in the gameplay-snapshot block. `presented_aim_pose` yaw includes mover carry yaw.
  - Local fire signal: `"activate"` entries in `pending_weapon_script_events` (frame_loop/mod.rs) are filled before `collect_viewmodel`, and the fire sound already reads them. On a host they include every pawn's shots, so filter to the local pawn. On a client, `run_client_fire_path_post_loop` pushes one per predicted shot (`ClientPullPresentation::Fire` only).
  - No recoil or kick exists anywhere (bloom is accuracy only). `movement--view-feel` (done) scoped weapon sway out.
  - Placement rides `WieldableTuningPayload.placement` and feeds the fire origin and muzzle. The fire origin is "steady placement, no view-feel" (networking.md §Weapon placement is content). `TUNING_PAYLOAD_EPOCH` = 14 (was 13 at Read at; bumped by ad9dad20).
  - `defineWeaponPlacement` is a pure builder shared by weapon `placement` and manifest `defaultWeaponPlacement`. `resolve_weapon_placement` resolves whole values with fallbacks.
  - `presented_view_feel_scale` (options/resolved.rs) is 0 under reduce motion. player_options.md §5 enumerates the effects it covers. Viewmodel placement is never scaled today.
  - Drafts touched: `movement--state-view-pose` plans to fold `viewFeel.vault` into rate-based `pose` rows and still points at a separate weapon-lowering draft. `player-descriptor-composition` reserves a per-weapon `viewFeel` overlay that changes camera feel while wielded.
Not verified:
  - Fire kick fires exactly once per predicted shot through every reconcile path. Traced only partially.
  - How look lag excludes mover carry yaw: input deltas, or presented aim minus carry.
  - Engine default values for the lowered pose and channel strengths. Tune by playtest.
Decisions (owner-ruled 2026-10-14 unless noted):
  - Scope: one viewmodel motion layer, four channels — lower/raise, look lag, fire kick, landing kick. A separate weapon bob is deferred (the weapon already follows camera bob).
  - Composition: viewmodel = camera-follow view feel (as today) ∘ placement ∘ motion-layer offsets. The layer never feeds the fire origin, muzzle terms, aim, the camera or the third-person mount. With all channels at rest the transform is bit-equal to today's.
  - Per-weapon motion profile, set up like placement: a pure reusable builder, a weapon-level field, a mod-wide manifest default, and an engine fallback, so a value is defined once in script and reused. It holds the lowered pose (a placement-relative position/rotation delta) and the per-weapon lag and kick tuning. It is host-resolved into the tuning payload with a `TUNING_PAYLOAD_EPOCH` bump, under the "small host-resolvable values are replicated" contract. It must not be named `viewFeel`, which is reserved by player-descriptor-composition.
  - `lowerMs`/`raiseMs` get a non-zero engine default (proposed 150/200 ms, tuned in playtest). An explicit 0 still means instant. Update the SDK .d.ts/.d.luau, the goldens and primitives/mod.rs docs. Gated on the host switch-timing prerequisite.
  - Scaling: every channel follows the presented view-feel scale and reduced motion, and integrators keep advancing at 0. Accepted consequence: with reduced motion on, a switch has a lockout with no visual cue.
  - Lowered weight: render-side, 0..1, presented with frame alpha. It never steps: a restart mid-raise continues from the current weight; a refusal or a snap to Idle releases from the last presented weight; overlapping sources take the max.
  - Vault: the weapon lowers while `vault_blocks_fire` holds. `allowFire: true` never lowers. This runs whether or not `viewFeel.vault` is authored.
  - Refactor: extract the vault integrator's machinery into one shared presented-weight helper, used by the vault camera pose (no behavior change), the switch lower and the vault lower. Amend `movement--state-view-pose` so `pose.vault` keeps progress weighting through the helper, and drop its pointer to a separate weapon-lowering draft.
  - Look lag is driven by player look, never by mover carry. Standing on a turntable produces no lag.
  - Fire kick reads the local pawn's `"activate"` emissions, once per predicted shot, and never on reconcile replay.
  - Landing kick reads movement edges, the same as camera impulses.
  - Fix the stale "no weapon entity" comments.
  - Authored lower/raise clips are a follow-up. The layer keeps the lowered weight and the lowering/raising phase readable for it.
Non-goals:
  - Authored clips and viewmodel clip sampling.
  - A separate weapon bob.
  - Third-person or observer motion.
  - A raise on spawn, pickup or drop (those stay instant).
  - Switch rule changes: no reverse or cancel, no proportional restart.
  - Host switch timing (`coop--host-switch-timing`).
  - HUD equip-state slots.
  - Camera recoil or aim change.
Proof:
  Automated:
    - Weight function: Idle 0, Lowering 0→1, Raising 1→0. Durations of 0 give no NaN and no flicker frame.
    - Continuity: a switch mid-raise continues from the current weight. A third-slot retarget leaves the trajectory unchanged. A refusal releases with no step.
    - The presented weight is monotonic within a lower at 30/60/240 Hz and with frame_dt 0.
    - The vault camera pose matches today's `view_feel::vault::tests` after the helper extraction.
    - A vault with `allowFire` false lowers, and with it true does not. Entry and exit tick edges. Works with no `viewFeel.vault` block.
    - Look lag is zero on a turntable with no input, and present with input.
    - Fire kick fires once per local shot. Another pawn's shot and a reconcile replay produce no kick.
    - A landing edge kicks.
    - All channels at rest give a transform bit-equal to today's `viewmodel_world_transform`. The fire origin and `client_fire_muzzle_terms` are unchanged under any channel.
    - Scale and reduced motion pin both sides: 0 gives none, 1 gives full, and the integrator resumes from its live state.
    - JS/Luau parser twins handle the profile and the builder (accept and reject cases). Resolution order: weapon > mod default > engine.
    - Tuning golden carries the epoch bump; the client reads the host value.
    - The `lowerMs`/`raiseMs` default applies when omitted, and an explicit 0 still means instant. Fix the tests that assumed a same-tick switch.
    - Connected-client harness: a predicted switch shows the lower. The archetype-fallback window gives weight 0.
  Manual:
    - Playtest dev weapons: switching (rapid, mid-raise, retarget), look lag, fire kick, landing.
    - A co-op client at realistic latency, including a refused switch.
    - Reduced motion on and off.
    - A vault with `allowFire` false and true.
Open owner questions:
  1. The profile's name and field shape.
  2. Default durations (150/200 proposed) and whether channel strengths default to a subtle non-zero.
  3. Whether the lowered pose's engine default needs per-mod art guidance in docs/.
Route: brief (likely resumable)
Why this route: It adds a modder-facing descriptor surface, a tuning-epoch bump, a gameplay default change gated on a prerequisite, a cross-draft amendment, and four channels with one shared helper. The decisions need review before code, and the work spans ordered phases.
Next action: draft (`/draft-brief` from this handoff; the build waits on the host switch-timing prerequisite)
```
