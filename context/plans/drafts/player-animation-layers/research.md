# player-animation-layers — research

Read at 9a440e8. Findings that inform the brief without deciding it.

## exo_red clip inventory

Skeleton root `mixamorig:Hips`; the model faces −Z, right is +X. Every clip animates all 66 joints.

| Family | Duration (s) | Natural speed (m/s) | Root |
|---|---|---|---|
| idle, idle_aiming, idle_crouching | 2.133 | — | fixed |
| idle_crouching_aiming | 2.033 | — | fixed |
| walk ×8 directions | 1.033 | 1.78 | translates along its named direction |
| run ×8 | 0.533 | 4.32 | same |
| sprint ×8 | 0.533 | 6.48 | same |
| walk_crouching ×8 | 1.033 | 1.89 | same; `backward_right` has a duplicated last key |
| turn_90_left / right | 1.033 | — | hips yaw +90° / −90°, root fixed |
| crouching_turn_90_left / right | 1.300 / 1.267 | — | hips yaw ±90° |
| jump_up / loop / down | 0.567 / 0.967 / 0.700 | — | in place |
| death_* (6) | 1.9–3.7 | — | fall travel 0.35–1.09 m; names do not match fall direction |

- Diagonals travel exactly ±speed/√2 per axis, so each family is equal-speed in every direction.
- `idle_aiming` differs from `idle` by about 16° on average across the arm, shoulder and spine joints, with the hands about 0.19 m higher. It is the weapon-mount solver's reference pose.
- limitator shares the Mixamo skeleton (it adds end bones and Throat) and carries `firing_rifle`, `reloading` and `hit_reaction`. exo_red has no weapon-action clips.

## Tier pace versus clip pace

Dev tiers: walk 7, run 11, crouch 3 m/s. The old mapping played the walk family at 7 m/s, about 4× natural pace, with a `travelSpeed: 7` override that hid it. With the shift-up mapping:

| Tier | Family | Ratio | Rate after clamp (0.5–1.5) |
|---|---|---|---|
| walk 7 | run 4.32 | 1.62 | 1.5 |
| run 11 | sprint 6.48 | 1.70 | 1.5 |
| crouch 3 | crouch-walk 1.89 | 1.59 | 1.5 |

The playback clamp is `RATE_MIN`/`RATE_MAX` in `playback.rs`. Raising it is not in scope.

## Runtime today

- Two sources and one weight (`BlendSource` is `Clip | Snapshot | Rest`). An interrupted fade collapses to a snapshot, so no more than two legs contribute live.
- Incoming clips start at elapsed zero. Loops add a fixed per-entity `instance_phase` de-sync, and there is no normalized-time concept.
- `UpperLowerSplit` is a procedural yaw after sampling. Its world factors are 1 / 0.5 / 0 by mask, with one step at the seam joint. No clip is masked.
- Root motion: XZ drift is measured as `travel_speed` and subtracted at load. Root yaw stays in the joint track.
- Players have one `current_state`. The host replicates it as `WireMeshAnimationState`, and each peer re-derives locomotion and treats the host name as a correction (`queue_correction`, `admits_state`).

## Weapon signals per peer

| Signal | Host | Shooter's client | Other clients |
|---|---|---|---|
| Shot | `activate` emission | predicted `activate` | `WeaponCue::Activate` (owner excluded) |
| Reload | `reload_*` emissions | edges from owner-private slots, one RTT late | none today |
| Aim / ADS | none — always aiming | — | — |

The weapon state machine (`WieldableState`) is never replicated. The active weapon archetype is, through the entity record.

## Rejected shapes

- **General blend tree / blend-space asset.** It would add an authored graph the selector does not need. Tiers are equal-speed rings of eight clips sharing a duration, so a flat adjacent-pair weight with one phase covers them. A blend tree stays open if an archetype ever needs non-ring blends.
- **Restart the fire clip on every shot.** At automatic rates (about 10 Hz) against a 0.5 s clip it would stutter. Holding while shots keep arriving reads as continuous fire.
- **Replicate directional weights or the dominant sector.** The client derives them from the same presented velocity and aim. Replicating them would make host corrections flicker at sector boundaries.
- **Aim as its own layer.** Nothing toggles aim, so a layer would always be on. Rest-pose remapping gives the same pose.
- **Players-only weapon layer.** Rejected for the no-actor-kind rule. Mainstream engines gate upper-body action layers on the body's animation setup, not on its controller: Unreal (Lyra) bots and players share one character and anim graph, and a fire montage plays in an upper-body slot for both; Unity layers and avatar masks belong to the Animator. Source's split between player and NPC anim code is the outlier. Gating on declared states also avoids a fight with enemy brains that play whole-body fire states.
- **Per-weapon body clips (`weapon_fire_<class>` with fallback).** Deferred, additive. Until then every weapon plays the body's one pair, and a per-shell reload loops the magazine clip.
- **Reload as replicated shared-visible state.** Observers would derive edges as the owner does, and a late joiner would arrive mid-reload. Rejected: cue events keep a reload and the shot that cancels it ordered on the one reliable lane the fire cue already uses. The cost is that a joiner mid-reload misses that one reload's layer.
- **Hit zones on the full pose.** Rejected: E21 keeps hit-zone authority on the unmodified pose, and the weapon layer is presentation selected per peer.
- **Cross-file clip library now.** It is a standard engine feature (shared-skeleton clips), but the owner chose re-export for exo_red. A future library must bind strictly by joint name and rest pose and refuse mismatches; retargeting is where low-quality poses come from.

## Turn-in-place notes

- Heading is recomputed per frame from velocity, and standing falls back to transform yaw (aim). There is no clamp or slew.
- The read point for a stored yaw is the standing `fallback` in `presented_player_heading`, plus `present_avatar_pose_inputs` (the host drawing a client's pawn) and `sample_into_registry` (remote clients emit no heading while still).
- The turn clips rotate the hips a full ±90° with a fixed root. Unless that yaw is extracted, the body pops back by 90° at clip end.
