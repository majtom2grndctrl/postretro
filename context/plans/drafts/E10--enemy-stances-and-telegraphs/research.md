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
