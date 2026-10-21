# player-animation-layers

Brief · resumable · reads: `context/lib/entity_model.md` §7b · `context/lib/networking.md` §Host-side remote-pawn presentation, §Presentation events vs. replicated state, §Version gates · `context/lib/resource_management.md` §7 · `context/lib/scripting.md` §10.3 · read at 9a440e8

## Problem
The owner, playtesting co-op, sees a third-person player body that uses a fraction of its model's clips. Strafing turns the legs about 90° onto a forward cycle. Turning in place snaps the legs to aim. Walk clips play at several times their authored pace. Nothing on the body answers a shot or a reload. Cause: the runtime blends at most two whole-skeleton poses and restarts every incoming clip at zero, the lower body keeps no yaw of its own, and no peer but the host learns that another player is reloading. When this is done:
- A moving body faces its legs at aim and blends its directional clips by travel angle, feet in phase.
- A standing body holds its legs until the waist twist passes a threshold, then turns them with a turn clip.
- Fire and reload clips play on the upper body over any lower-body state, on every peer.
- The dev player maps clip families whose pace suits each speed tier.

## Decisions
- **One pose description, one sampling function.** A frame's pose is:
  - a weighted, phase-shared set of locomotion clips;
  - an optional crossfade out of the previous set;
  - at most one masked upper-body layer.

  The renderer, posed hit-zone queries and socket attachments share one function below the pose-modifier stack.
  - Mesh and attachments sample the full pose, then modifiers, so the gun stays in the drawn hand.
  - Hit zones sample the locomotion set and crossfade only. The weapon layer joins the modifiers as presentation, keeping the E21 rule that hit-zone authority ignores presentation (`plans/done/E21--pose-modifier-stack`).
  - Lifts the M10 ceiling ("crossfade between two whole-skeleton poses", `plans/done/M10--skinned-animation-runtime`).
- **Directional names, per tier.** The engine recognizes `<tier>_<direction>` for tiers `walk`, `run` and `crouch` and directions `forward`, `backward`, `left`, `right`, `forward_left`, `forward_right`, `backward_left`, `backward_right`. Naming follows the shipped scheme (`walk_forward`, `crouch_backward`); no descriptor field is added.
- **A tier with side clips faces aim.** A tier that declares usable `_left` and `_right` states turns its legs to aim and weights the two directional states adjacent to the travel angle. A missing diagonal blends from its cardinal neighbours; a missing backward follows the existing forward fallback. A tier without side states keeps the §7b fold and hysteresis unchanged.
- **A blended tier is one state to selection, correction and the wire.** Its identity is the tier's forward state name. Direction weights are per-peer presentation, derived from presented velocity and aim, and never replicate, so host corrections compare tiers rather than sectors. No snapshot change.
- **Locomotion loops share phase.** Entering a looping locomotion state inherits the outgoing state's normalized phase; directional clips within a tier share one phase. Every other state keeps today's restart and per-entity de-sync.
- **Turn in place.** Each peer keeps a lower-body yaw on the mesh animation, beside the locomotion direction and jump phase, and never replicates it. Rules:
  - Standing, the yaw holds while the waist twist stays inside a threshold. Past it, `turn_left` or `turn_right` (crouched: `crouch_turn_left`, `crouch_turn_right`) plays and drives the yaw by the clip's own root rotation.
  - A twist past a hard cap drags the yaw, so the upper body never exceeds the cap.
  - Moving resets the yaw to the tier's facing.
  - Without a usable turn state, the standing body snaps to aim as today.
  - Only turn states neutralize root yaw; every other clip renders its hips as authored.
  - Turn states are one-shots admitted like jump one-shots (`admits_state`).
- **Weapon layer: fire and reload, upper body only.** The layer plays `weapon_fire` and `weapon_reload` on the `upperBody` pose mask over any lower-body state, including jump, crouch and turns.
  - **Who gets it:** any mesh that declares either state, whoever wields the weapon. There is no branch on actor kind (`entity_model.md` §Components, Weapon vocabulary). An enemy whose brain plays whole-body fire states declares neither.
  - **Fire:** enters on a shot and holds while shots keep arriving within one clip length, so automatic fire does not stutter. A shot after release re-enters from the clip start.
  - **Reload:** spans `reloadMs` for a magazine reload, loops at rate 1 for a per-shell reload, and ends on completion or cancel.
  - **Selection:** host-free and per peer.
    - The host uses its weapon and AI emissions.
    - The shooter's client uses its predicted shot and its reload edges.
    - Other clients use the observer cues, which already carry enemy shots.
  - **Aim:** not a layer. The player always aims, so aiming idles serve as rest poses.
- **Missing weapon states warn once, quietly.** The first shot or reload by a skinned shooter whose descriptor lacks a usable `weapon_fire` or `weapon_reload` logs one warning per descriptor and state. The body keeps its lower-body pose. Owner decision: support ships before the dev model has the clips.
- **Reload reaches observers on the weapon-cue lane.** The host forwards each pawn's reload started, shell loaded, completed and cancelled to observers beside the fire cue, carrying that address's sound, on the reliable lane. The reloading owner is excluded, as for fire. This replaces host-local-only reload audio for remote observers. Events, not replicated reload state, keep a reload and the shot that interrupts it ordered on one lane. It changes the cue layout, so it gates per `networking.md` §Version gates.
- **Dev content.**
  - Walk tier plays exo_red's run family, run tier plays its sprint family, crouch keeps crouch-walk.
  - Derived clip speeds replace the `travelSpeed` overrides. The playback clamp keeps a little foot slide at full speed.
  - `idle_aiming` and `idle_crouching_aiming` become the rest poses.
  - exo_red has no fire or reload clips, so the dev player declares neither until the model is re-exported (owner content task).
  - `walk_crouching_backward_right`'s duplicated final keyframe is an owner content fix, not engine work.
- **Non-goals.**
  - Player deaths wait for a player death substrate: a dead pawn still walks, and locomotion overwrites any set state.
  - A cross-file clip library is deferred. Re-export is the content fix, and any later library binds strictly by joint name and rest pose.
  - Hand IK and left-hand grip are an E21 non-goal.
  - Per-weapon body clips are deferred: there is one fire and reload pair per body. A later per-class lookup that falls back to the pair is additive.
  - First-person viewmodel animation, weapon switch clips, and hit reactions are out of scope.
  - Enemy strafe and turn stay deferred by `plans/done/E10--enemy-facing-slew`, though this substrate serves them.

### Scripting surface
```typescript
mesh: {
  model: "models/exo_red/model.gltf",
  animations: {
    idle: { clip: "idle_aiming", loop: true, crossfadeMs: 50 },
    walk_forward: { clip: "run_forward", loop: true, crossfadeMs: 50 },
    walk_backward: { clip: "run_backward", loop: true, crossfadeMs: 50 },
    walk_left: { clip: "run_left", loop: true, crossfadeMs: 50 },
    walk_right: { clip: "run_right", loop: true, crossfadeMs: 50 },
    walk_forward_left: { clip: "run_forward_left", loop: true, crossfadeMs: 50 },
    // …remaining diagonals, then run_* (sprint clips) and crouch_* likewise
    turn_left: { clip: "turn_90_left", loop: false, crossfadeMs: 80 },
    turn_right: { clip: "turn_90_right", loop: false, crossfadeMs: 80 },
    crouch_turn_left: { clip: "crouching_turn_90_left", loop: false, crossfadeMs: 80 },
    crouch_turn_right: { clip: "crouching_turn_90_right", loop: false, crossfadeMs: 80 },
    weapon_fire: { clip: "firing_rifle", loop: true, crossfadeMs: 40 },
    weapon_reload: { clip: "reloading", loop: false, crossfadeMs: 80 },
  },
  defaultState: "idle",
  locomotion: { speedScale: true },
}
```
Names are the surface; state fields are unchanged. Luau authors the same table, and both ship.

## Acceptance
### Automated
**Directional blend**
- [ ] A full 360° sweep of travel angle against aim gives weights that sum to one and change continuously, with exactly two non-zero weights inside each sector, checked on both sides of every sector boundary.
- [ ] A tier missing one diagonal blends its two cardinal neighbours at that angle; a tier without side states selects exactly as before (existing selector tests pass unchanged).
- [ ] Backpedalling straight back on a side-capable tier plays the backward state with legs at aim, and the upper/lower twist stays near zero.
- [ ] The host and a remote client presenting the same strafing pawn agree on the replicated state name across a full sweep; no correction is queued for a sector change.
**Phase**
- [ ] Walk→run, forward→backward and walk→crouch crossfades start the incoming loop at the outgoing loop's normalized phase; a non-locomotion state still starts at zero.
**Sampler parity**
- [ ] For a pose with a directional blend, a crossfade and an active weapon layer, renderer and attachment sampling return identical joint matrices.
- [ ] Hit-zone sampling of the same pose equals that pose with the layer and modifiers removed.
**Turn in place**
- [ ] A standing twist just past the threshold starts a turn; just inside it does not.
- [ ] A turn clip ends with the lower-body yaw advanced by the clip's root rotation and no pose discontinuity on its last frame.
- [ ] A flick past the cap during a turn keeps the twist at the cap.
- [ ] Moving during a turn resets the yaw to the tier facing.
- [ ] Without turn states, a standing body faces aim each frame as today.
- [ ] A clip not bound to a turn state keeps its authored hips yaw.
**Weapon layer**
- [ ] Shots on consecutive ticks hold one fire clip without restarting.
- [ ] A shot after the layer released restarts the clip.
- [ ] Lower-body joints are unchanged by the layer.
- [ ] A magazine reload spans its `reloadMs`, a per-shell reload loops until completion, and a cancel releases the layer on the next frame.
- [ ] A shooter without `weapon_fire` warns exactly once across repeated shots; same for `weapon_reload`; none warns when both exist.
- [ ] An enemy declaring `weapon_fire` plays the layer on its shots on the host and on an observer client; an enemy declaring neither keeps its brain's state untouched.
**Reload cue**
- [ ] An observer client receives started, shell-loaded, completed and cancelled for another player's reload, plays each sound and drives the layer; the reloading owner receives none.
- [ ] Two reload addresses from one pawn on one tick arrive in host order.
- [ ] A peer on the previous wire version is refused at handshake.
**Content**
- [ ] The dev player loads with every declared state usable and its walk and run tiers rate-scaled from derived clip speeds.
- [ ] The Scripting surface example, authored in TypeScript and in Luau against a fixture model that has every clip, resolves every state it names.
### Manual
- [ ] Co-op at simulated lag, watched from the peer: circle-strafe, diagonal runs, backpedal, crouch-strafe, standing turns, a 180° flick, jumps while strafing.
- [ ] Fire and reload seen from a teammate on each reference weapon, after the exo_red re-export; record whether `reloading` keeps the rigid weapon in hand.
- [ ] A connected client hears a teammate's reload.

## Path
- Sampling seam: `MeshSampleParams` (`crates/model/src/sample_params.rs`) built by `animate_entity` (`crates/sim/src/scripting_systems/mesh_anim.rs`). Consumers are `sample_instance` (renderer `mesh_pass.rs`), `sample_blended_world` (`hit_zones.rs`) and `sample_blended_world_modified` (`attachments.rs`). Blend math: `blend_local`, `resolve_blend_into`, `capture_blend` (`crates/model/src/anim/blend.rs`).
- Phase: `reset_incoming_playback_time`, `snapshot_previous_playback_time`, `instance_phase` in `crates/entities/src/components/animation/`.
- Facing:
  - `player_travel_heading_yaw` and `classify_locomotion_direction`.
  - `presented_player_heading` and `update_pose_inputs` (`crates/sim/src/sim/mod.rs`).
  - `present_avatar_pose_inputs` (`host_presentation.rs`).
  - `sample_into_registry` (`client.rs`): today it emits a heading only while moving.
- Turn memory sits beside `LocomotionJumpPhase`; `jump.rs` is the one-shot precedent, which suggests a sibling `turn.rs`.
- Root yaw: `derive_and_neutralize_root_motion` handles XZ only. Store a per-clip yaw curve at load and subtract it only when a turn state samples it.
- Weapon signals:
  - Host: emissions from `weapon_stage/commands/{local,remote}.rs`.
  - Shooter client: `run_client_fire_path_post_loop` and `client_reload.rs` edges.
  - Observers: `publish_observer_weapon_cues` (`weapon_observers.rs`) and `materialize_observer_weapon_cue`.
  - The cue is shot-keyed today; a reload cue needs a pawn-keyed shape.
- Shape chosen: a flat weighted set with one phase per tier. Rival: a general blend tree, more than one tier needs, rejected in `research.md`.
- First slice: the shared sampler with a two-clip directional blend and phase inheritance, proven by the parity row. It falsifies the riskiest assumption, that three sites can share one sampling function without a frame-cost regression.
- `crates/netcode/src/client.rs`, `crates/sim/src/sim/mod.rs` and `pose_modifier.rs` exceed ~800 lines. Split before extending, behavior-preserving, in their own commits.

## Open questions
- Turn threshold and cap values, and the turn-clip crossfade; the cap stays within the 105° twist §7b already tolerates. **delegated**
- Whether a turn chains when the twist still exceeds the threshold at clip end. **delegated**
- Fire and reload cue timing against the observer's presented pose (follow the fire cue). **delegated**
