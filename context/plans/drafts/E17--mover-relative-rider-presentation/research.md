# Mover-Relative Rider Presentation — Research

Read at 8489a1fe. Symbols, not lines; the executor re-verifies.

## Who reads which mover pose today

| Consumer | Pose it reads | Where |
|---|---|---|
| Mover beauty, shadow instances, occluder AABBs | `registry.interpolated_transform(id, alpha)` | `KinematicMoverRenderCollector::collect_with_mirror` (`crates/postretro/src/runtime_movers.rs`); visibility cull uses the current fixed-tick bounds |
| Mover-attached lights | registry interpolated transform | `crates/postretro/src/light_bridge.rs` |
| Foot probes, single-player and host own pawn | `MoverTickStateTable`, end of this tick, no alpha | `simulate_started_tick_with_presentation_aim` → `update_foot_ground_probes` (`crates/sim/src/sim/mod.rs`) |
| Foot probes, connected client | `App.kinematic_mover_tick_states`, no alpha | `App::update_client_presentation_pose_inputs` (`main.rs`) |
| Foot probes, host seat (client pawns) | live tick table ("deliberately live, not delayed") | `host_presentation::probe_presented_client_pawn_feet` |
| Local camera yaw | tick table + `tick_rotation_delta * alpha` residual | `effective_render_yaw`, `mover_yaw_render_residual` (`main.rs`) |
| Local camera position, local pawn mesh | world-space lerp of tick endpoints | `InterpolableState::lerp` (`crates/sim/src/sim/frame_timing.rs`); `MeshRenderCollector::collect_inner` |
| Remote riders, client view | delayed world-space sample, no ground | `ClientReplication::sample_into_registry` → `RemoteInterpolationBuffer::presented_pose` |
| Remote riders, host seat | delayed world-space sample, `newest_recorded_tick − PRESENTATION_DELAY_TICKS + alpha` | `host_presentation::present_client_pawns` |

`MoverPoseSource` (`crates/physics/src/collision/moving.rs`) is implemented by `MoverTickStateTable`, `MoverTickPoseSource`, `EmptyMoverPoseSource`, `MoverHistoryBuffer` (newest sample only), `ReplayCollisionSource` and `OneMoverPose`. No per-frame shared presentation pose exists.

## Remote riders

- `TransformSample` and `MovementOnlySample` carry no ground. The client reads `WireGroundRef::Mover` only to reject unknown movers (`unknown_ground_mover_id`) and to set the grounded stance bit (`wire_ground_ref_to_foundation_checked`).
- Movers on a client are predicted, not delayed. `prepare_mover_apply` fast-forwards the received phase to `floor(estimated_server_tick)`, and `client_predict_loaded_movers_tick` advances them each fixed tick. Remotes sample `estimated_server_tick − interpolation_delay`, so they run one delay behind their mover.
- The host seat has the same split at a smaller delay (`PRESENTATION_DELAY_TICKS` = `INPUT_BUFFER_TARGET`). Its movers are drawn at the registry blend of the current tick. Its client pawns sample the delayed buffer the host records each tick (`record_client_pawn_poses`). The host owns each client pawn's authoritative movement state, so ground is available where the pose is recorded.
- `MoverHistoryBuffer` (`crates/netcode/src/client.rs`) is keyed by mover id and capped at `MOVER_HISTORY_LIMIT`. It records the authoritative sample and one predicted sample at the fast-forward tick on each apply, not every predicted tick. It answers whole ticks only, through `resolve_replay_tick` / `replay_pose` (`client/replay_poses.rs`). A fractional read resolves the two whole-tick poses around it by replay and blends them.

### Rebase contract

```mermaid
sequenceDiagram
    participant Rec as "record (client apply / host tick)"
    participant Buf as "delayed rider buffer"
    participant Hist as "mover history at the rider's tick"
    participant Now as "render-time mover pose"
    participant Out as "presentation transform"
    Rec->>Buf: "pose + ground at tick t"
    Buf->>Buf: "sample at delayed fractional tick τ"
    Buf->>Hist: "same mover on both bracketing samples?"
    Hist-->>Buf: "mover frame at τ"
    Buf->>Now: "local = frame(τ)⁻¹ · pos"
    Now-->>Out: "pos' = frame(now) · local; yaw += Δyaw_up if carry_yaw"
```

The historical rider yaw already includes carry up to τ, so adding the world-up yaw from τ to now advances it once. Composing the full mover rotation into orientation would tilt the player.

## Local eye

- `InterpolableState` holds only `position`. `lerp` is `Vec3::lerp`. Every `push_state` site pushes the eye after `follow_camera_to_local_pawn`.
- `interpolated_transform` lerps position and slerps rotation, so a point fixed to the mover sweeps an arc while the eye sweeps the chord. The gap at mid-tick is r·θ²/8 for a turn of θ per tick at radius r. On the carousel (45°/s, 60 Hz, θ ≈ 0.013 rad) it is about 0.1 mm at 4–6 m. At 360°/s it is about 8 mm at 6 m. The gap is not what play-tests saw. The owner kept the arc because one rule, placing the attached eye through the mover's presented pose, also carries a smoothed correction to the eye. Without it, the client needs a separate rule that applies the correction at the eye's point. NPC riders on single-player and the host keep the straight blend at the same millimetre scale.
- The local pawn mesh is `shadowOnly` in first person (`content/dev/scripts/player.ts`).
- `ClientPrediction::presentation_offset` (`crates/netcode/src/prediction.rs`) is added inside `pawn_eye_position` before `push_state`, so both endpoints already include it.
- `apply_authoritative_mover_corrections` turns a carried rider's camera yaw by the correction's yaw at apply. Under smoothing, the view yaw has to take the presented correction's world-up yaw, or the view turns while the deck holds still.

## Where the pose lives

`interpolated_transform` is the render-time accessor every per-entity visual pass reads (`entity_model.md` §5). Presentation writers that resolve a final pose, such as the remote buffer and the host seat, write it through `set_presentation_transform`. A mover's current `Transform` is simulation truth, so smoothing cannot use that writer. It has to be a correction the accessor applies on read.

A per-frame mover pose table was the earlier shape. Whenever smoothing runs, every reader not handed the table draws a different mover than the beauty pass:
- ghosts (`ghost_presented_transform`)
- sound anchors (`sound_events/anchors.rs`)
- E21 attachments and the `E22--kinematic-assemblies` draft
- `mover-aware-reflection-captures`

The registry correction costs one sparse lookup per accessor call and nothing for an entity without a correction. The table cached poses once per frame, but each consumer had to be wired to it.

## Mover corrections

- `ClientReplication::apply_components_to` writes phase and `Transform` with `set_component`, which writes the current slot only. `previous_transforms` keeps the predicted pre-snapshot pose. `run_snapshot_apply_stage` runs before the tick loop, and the client refreshes previous only in `client_predict_loaded_movers_tick`. A zero-tick frame therefore blends an unmatched pair. A frame that ticks still jumps by about the correction.
- `MoverCorrection` records position magnitude only. `apply_authoritative_mover_corrections` uses it to adjust a carried rider's yaw and to publish to `MoverTickStateTable`.
- No test measures the drawn step. `moving_platform_reconciles_under_mandated_profile_without_accumulating_drift` allows corrections up to 0.16 m, eighty times the 0.002 m step the earlier draft would have tolerated. This led the owner to build smoothing outright and use measurement as proof.

### Own-pawn share of a mover correction

The ordinary arm of `reconcile.rs` seeds `presentation_offset` with predicted-minus-reconciled pawn position. Replay carries the pawn on the corrected mover history, so a mover correction d under a standing pawn produces a pawn correction of about d. Suppose the eye stayed on today's path while the drawn deck held its old pose. The eye would glide at the pawn offset's rate and the deck at the mover's, and the rider would slide across the deck in between. The brief keeps the mover-explained share out of the pawn offset, and applies the mover's presented correction at the eye instead. On leaving the mover, whatever correction remains at the eye moves into the world offset, so nothing steps.

## Commitments touched

- `networking.md` §Host-side remote-pawn presentation: host-seat foot probes use the live mover pose. This brief moves them to the render-time pose. Update the section at promotion.
- `movement.md` §6, Moving bases (Agents): "co-op presentation of NPCs on moving geometry is not built". It stays unbuilt, and the roadmap item owns it.
- `plans/done/E17--rotating-movers/`: intrinsic velocity drives remote locomotion, fixed-tick yaw stays authoritative with an alpha residual, and `carry_yaw` never crosses the wire. All are kept.
- `entity_model.md` §5: the registry accessor stays the one presented pose. It gains a render-only correction term, which goes into §5 at promotion.
- `E22--kinematic-assemblies` (draft): members render at their holder's interpolated transform, so they follow smoothing with no change.

## Oversized sources

| Source | Lines at read | Action |
|---|---|---|
| `crates/netcode/src/client.rs` | ~8,700 | Split remote presentation and mover history out first |
| `crates/postretro/src/main.rs` | ~13,100 | Split camera follow and yaw carry helpers out first |
| `crates/sim/src/sim/mod.rs` | ~4,700 | Split foot probing out first |
| `crates/netcode/src/interpolation.rs`, `scripting/frame_systems/mesh_render.rs`, `runtime_movers.rs` | 2,100–2,800, mostly tests | Extend at the boundary |
