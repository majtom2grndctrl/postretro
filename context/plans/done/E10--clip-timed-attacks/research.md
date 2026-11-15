# E10--clip-timed-attacks — research

Read at 919d34a05. Facts cited by symbol; the executor re-verifies.

## Firing today
- An attack fires on the first tick after entry on which every gate passes: one fire per entry (`activity_attack_count`, reset on entry), target in range and alive, the perception fire gate, heading within tolerance, cooldown at or below zero. `compute::evaluate` (Attack block) builds a `PendingAttack` from `graph_eval::action_for_path`.
- `attack_turns::lifecycle::arbitrate_attack_turns` drops a proposal with no turn held; the latch stays open and it retries next tick. This is why `at` implies a mark: an unmarked attack's request forms with its proposal, which `at` would delay to the moment.
- Contact attacks resolve once, at fire (`apply::apply_outcomes` → `AiHost::apply_damage`). Weapon attacks become a `WeaponAttackRequest` advanced by `AiWeaponControllers::advance_actor`.
- `action_for_path` finds the action at any depth, on an activity or a selector row (`selector_action`). `animation_for_path` picks one state for the path: a nested leaf's own animation, else the nearest composite's, else the root leaf's, with locomotion leaves yielding to the rest animation at a standstill.

## Animation on the host
- `MeshAnimation` (on `MeshComponent`) stores the current and previous state, `entered_at` stamps in animation-clock seconds, and runtime-only `rate` and rebase fields. `switch_animation_state` returns `AlreadyInState` for the playing state, so activities sharing a state share its clip.
- The animation clock is per frame: `advance_anim_clock` adds frame dt × `anim_time_scale`; every tick of a multi-tick frame shares one value; dev freeze holds it. Enemy switches leave `entered_at` pending until `resolve_pending_animation_stamps` before render.
- Enemy rate is runtime-only, set for travel states by `update_brain_animation_playback_rates`. Authored states have `clip`, `loop`, `crossfadeMs`, `travelSpeed`, `interrupt`; no rate.
- Every loaded model's clips sit on the host in `HitZoneStore` (`ModelHitZones::from_loaded`), zones or none. `clip_duration` (sim) reads a clip's duration for player jump one-shots. `mesh_anim::state_elapsed` computes elapsed and completion but only tests call it.
- Only the state name replicates (`WireMeshAnimationState`); clients time clips on their own clock.

## Clips and the converter
- `AnimationClip` holds name, duration, joints and travel speed. `parse_clip` reads no animation extras. `append_library_clips` skips library clips the character already names and copies no metadata.
- `mixamo_to_gltf.py` writes node extras (`tag_skeleton`) and document extras (`stamp_clip_library`), nothing per animation. `trim_clip` builds a fresh animation dict, so it would drop source extras unless it copies events.
- No clip-event concept exists in source. Engine precedent elsewhere: Unreal Anim Notifies, Unity Animation Events, Godot method tracks, Source QC events, Quake's per-frame `ai_melee`; neither Mixamo FBX nor glTF carries them.

## The brute's timing defect
- `punch_windup` cuts `punch` at 467 ms, where the fist is drawn back high and stops moving (`mixamo_brute/license.txt`), so contact comes later. `reference-enemy.ts` says the strike lands about 250 ms in; the jab fires on entry and the slam's commit at 250 ms, both ahead of the fist, which reaches full reach at 767 ms.
- Moments measured by hand tracking on the source FBXs (2026-11-14): punch 767 ms, smash 1667 ms, swipe 1300 ms, firing_rifle kick 0 ms. Each library's `license.txt` records them, its full re-export command and its sources. A rifle rebuild differs from the committed bake only on `mixamorig:Throat`, as `done/character-clip-library/plan.md` records.

## Stage order
- `simulate_tick`: projectile impacts → AI → steering, then `update_brain_animation_playback_rates` → weapon stage (hitscan). Projectile flinches land before AI, hitscan flinches after it; the rate the AI reads was set by the previous tick's pose stage.
- Inside AI: activity clocks tick at the top of `compute::evaluate`, then transitions, then the Attack block on the new path; arbitration's hard gate runs after compute (`attack_turns::lifecycle`); apply fires, then switches animation only when a latch flips, and itself plays `jump_loop` while launched.
- Enemy playback rate clamps to 0.5–1.5 and is scaled only for the graph's locomotion state; a switch resets it to 1.
- A looping state samples at clip time plus `sample_params::instance_phase`, seeded by `EntityId`; one-shots take no phase.

## Ordering pins
| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Clip time sampling | Entry: the entered state's clip time is its start (0, or the drawn phase on a loop). Each later AI tick adds dt × rate before guards and the `at` check; compared on whole ticks, not raw `f32` sums | Fires on the first tick whose clip time is at or past the event; a moment on a tick boundary fires on that tick |
| P2 | Rate source | Rate is written in the pose stage after AI; a switch resets it to 1 | Tick N advances by the rate tick N−1's pose stage set |
| P3 | Exit row on the moment tick | Transitions run before the Attack block, as today | Path leaves; nothing fires |
| P4 | Moment on the clip's last tick | `animationDone` turns true on the tick after the clip reaches its duration | Fires on the moment tick; an `animationDone` exit leaves on the next |
| P5 | Entry onto a shared non-looping clip around the moment | The entered activity is checked on its entry tick, after transitions | Clip already past the moment on the previous tick: nothing fires. Clip reaching it on the entry tick: fires that tick |
| P6 | Impact switch before AI on the entry tick | Projectile impact before AI; the entry's own switch follows in apply | Entry not cancelled |
| P7 | Impact switch after compute on the moment tick | Hitscan in the weapon stage; an earlier slot's contact hit in the apply batch | That tick's fire stands |
| P8 | Switch off and back between AI ticks | Two reactions or effects before the next AI tick | Moment cancelled for that entry |
| P9 | Turn gate at the moment | Hard gate in arbitration, after compute; grants precede it | No turn: whiff, nothing later in that entry. Turn granted in the moment tick's arbitration: fires |
| P10 | Non-looping → non-looping entry | Accumulator resets before the entered activity's first guard evaluation | `animationDone` false; the entered clip's moment fires |
| P11 | Cancelled moment and clip end | Clip time keeps accumulating for the path's state while the mesh is off it | `animationDone` true at the duration; the activity can leave |
| P12 | Held across the moment | No AI tick while held; turns cleared on readmission | Clip time freezes with the activity clocks; on release the moment fires on clip time if the re-requested turn is held, else whiffs |
| P13 | Save and load mid-entry | Clip time serializes with the activity clocks | Fires on the same tick as without the save |
| P14 | Travel variant switch mid-entry | Apply switches the variant with the activity unchanged; the drawn clip restarts | Clip time restarts; the variant clip's event times the moment |
| P15 | Knockback launch mid-entry | The launch begins; apply may show `jump_loop` only after the fall delay and only where the mesh declares it | Moment cancelled from the launch's first tick, whatever clip shows |
