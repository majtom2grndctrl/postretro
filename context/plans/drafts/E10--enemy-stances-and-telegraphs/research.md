# E10--enemy-stances-and-telegraphs — research

Read at 5932287f. Findings that inform the brief but do not decide it.

## Telegraphs today
- A windup is an authored activity left by a `timeInActivityMs` guard. Shipped examples:
  - `reference-enemy.ts`: slam, with windup 250 ms, commit 150 ms, recover 1450 ms.
  - `limitator.ts`: aim 550 ms, then fire.
- The attack fires once per activity entry, on the first tick that passes cooldown, range, aliveness, line of sight and facing (step 4 of `compute::evaluate`).
- The only engine delay is `SPAWN_ATTACK_WINDUP_MS` in `crates/sim/src/spawner.rs`.
- `IrNode` has no RNG. `BRAIN_INPUTS` holds only deterministic facts.
- **The randomness posture** (`networking.md`): engine randomness is host-only, load-time, consequences only. Weapon pellet spread is the one per-tick exception, seeded by `pellet_rng_seed(shells_fired, salt_name, slot)`.
  - Trigger pools roll once at load from `entropy_seed()`.
  - Chance-based pain and random patrol were both deferred for want of this story: `drafts/E10--enemy-stagger`, `done/E10--enemy-behavior-descriptor-syntax`.

## Replay-stable identity
- `EntityId` packs index and generation. Its index comes from a LIFO free list, so it depends on spawn order.
- The determinism gate (`crates/sim/src/sim/determinism_tests.rs`) runs `SpawnOrder::BetaThenAlpha` and compares by role label.
- `DescriptorProvenance` is per descriptor, not per instance.
- `BrainComponent` has no entry counter or entry tick. Its per-depth timers and attack counts zero on re-entry.

## Co-op delivery
- `SNAPSHOT_TICK_INTERVAL` = 2, about 30 Hz. `WireMeshAnimationState` carries the current state name only, latest value per snapshot. So a state shorter than one interval can be skipped, and a same-name re-entry is invisible.
- The client restarts the clip locally on a name change (`switch_animation_state`, local clock). No host clip time crosses the wire.
- **Behavior-entry sounds** play in the frame loop's Audio stage from `pending_ai_events`. That queue is always empty on a client, because clients never run `simulate_tick`.
- **Melee is silent on clients.** `publish_observer_weapon_cues` skips emissions without a `shot_id`, so a contact attack's sound never reaches a client. A weapon attack's sound rides `WeaponCue.additional_sound`.
- **Observer cues** (`freeze_observer_weapon_cue`) anchor to an entity `NetworkId` and go to all participating clients except the owner. An enemy has no owner, so every client receives them.
- **Version gates.** A new message bumps `PROTOCOL_ID` ("PRLC"). `WIRE_VERSION` (39) bumps only when an append-layout guard shows a shipped encoding changed. `SNAPSHOT_VERSION` (25) bumps for snapshot-record changes.

## Stagger without engine work
- **What works today:**
  - An impact policy (`defineImpactEvent`) can read `impact.amount` and `impact.target.state(n)`, and write `setState`.
  - Guards read `@state.*`.
  - Wildcard rows that target the current activity are filtered out.
- **What does not:**
  - No reaction writes per-entity state.
  - `onEnter` carries no subject.
  - `applyDamage` (`AppDrain`) runs no impact policy.
  - Policies have no clock, so authors would need a mod `accumulate` slot.
  - Two hits on one tick can erase a threshold pulse.
- So stagger is expressible, but fragile. One engine fact fed at the chokepoint removes the clock, the race and the `applyDamage` gap.
- **Stale wording, outside this brief:** `entity_model.md` §7c and the `state()` typedef say reactions write per-entity state. In production only impact policies and spawn seeding do.

## Stage order (`crates/sim/src/sim/mod.rs`)
- The stages run: Movers, Movement, Triggers, Projectiles, AI, Traps, Steering, Weapons.
- Projectile hits are seen by the brain the same tick.
- Player hitscan lands after AI, so the brain sees it next tick. Enemy melee lands in the AI apply pass, so its target also sees it next tick.

## Industry reference (owner conversation)
- An attack has three phases: anticipation, active, recovery. The tell lasts longer the more an attack hurts.
- Tells use several channels: pose, a unique sound, VFX, barks.
- For Honor's stances name the attack while the release timing varies. Doom Eternal's Marauder flashes before its axe.
- Unreal, Unity and Godot all leave this to animation notifies. Postretro times it in logic, which suits determinism and co-op.

## Review round 1 corrections
- The host restarts nothing on same-name re-entry. `switch_animation_state` returns already-in-state, and `restart_animation_clip` has no production caller. So a replicated entry sequence would have made clients exceed the host, and it was dropped.
- Luau has no object spread, and the SDK has no merge helper. So `merge()` is what makes the fragment approach mirror in Luau.
- An `onEnter` `playSound` already reaches every peer. A host-only reaction with no audience resolves to Everyone (`system_commands.rs`). Only the descriptor `sound` field was host-only.
- `drafts/E10--enemy-stagger` was deleted on this branch. Its accumulator design is superseded by `damageInActivity`.

## Pin table

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Release row and `breakOn` row both pass | The hold timer reaches its threshold on the tick the damage sum reaches `breakOn.damage`. | Break wins; release is never entered. The builder emits the break row first. |
| P2 | Damage around a transition tick | On tick N a hit lands before the AI pass, the brain leaves A for B, then a second hit lands after it. | The first counts toward A only. The second counts toward B and is read on B's first evaluation. |
| P3 | Activity entered and left on one tick | A reseat seats the initial activity and its own row passes in the same pass. | Its rows read its own fresh roll. Only the final leaf announces; nobody plays the skipped sound. |
| P4 | Nested levels entered by one descent | A composite and its nested initial seat on one tick; later the nested level transitions. | Independent rolls per level. A nested transition redraws and zeroes only the nested level. |
| P5 | Selector guard reads a scope-relative fact | A move or offense selector row guards on the roll or damage sum. | It reads its own level's values, matching a transition guard there. |
| P7 | A → B → A inside one snapshot interval | Activity A, with a sound, is entered, left and re-entered between two snapshots. | Each peer plays A's sound twice. The client's clip follows the latest replicated state. |
| P9 | Cue without a live client entity | A cue reaches a client before the enemy's snapshot, or after it despawned. | Plays once at the host-captured position and never replays. |
| P10 | Weapon attack sound | An enemy weapon attack carries a sound. | It rides `WeaponCue.additional_sound` and plays once. The enemy cue never duplicates it. |
| P11 | Killed in the pass that enters a stance | An earlier outcome in the apply pass kills the enemy that compute moved into a stance. | No entry sound and no cue on any peer. |
| P12 | Hold at zero or equal bounds | `holdMs: 0`, `[0, 0]`, `[x, x]`. | All load. Zero leaves on the first evaluation; `[x, x]` matches a fixed `x`. |
| P13 | Initial activity at spawn | A fresh brain seats its initial activity before per-instance spawn state is set. | The first evaluation reads this enemy's own salted roll. Two same-tick spawns differ, and replay reproduces both. |
| P14 | Stance entered on a client's join tick | On tick N an enemy enters its stance; the client begins participating that frame, before or after the host publishes tick N's cue. | A cue published after participation began plays once, at the captured position if the enemy is not yet materialized (P9). One published before is never delivered. Materializing the enemy mid-stance plays no sound. |

## Review round 2 corrections
- `WeaponCuesMessage` sends one cue per message and carries a `participation_epoch`. `MAX_WEAPON_CUES` is 64, and an invalid (over-cap) message is silently not sent (`crates/net/src/transport.rs`). So the enemy cue goes one per message.
- `MAX_DELAY_MICROS` (250 ms) is `pub(crate)` in `crates/netcode/src/interpolation.rs`, and netcode depends on sim. So the floor constant lives outside netcode, guarded by a netcode test (precedent: `SPAWN_ATTACK_WINDUP_MS`, `enemy_replication_harness_test.rs`).
- `"*"` rows evaluate before every activity's own rows at that level (`graph_eval.rs`). So they can leave a release and win a tie with the break row.
