# E10--enemy-stances-and-telegraphs

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §7c · `context/lib/scripting.md` §11 · `context/lib/networking.md` §Version gates, randomness posture · `context/lib/audio.md` §4 · read at 5932287f

## Problem
The owner wants For Honor–style attacks, a requested capability. An enemy should telegraph *which* attack is coming by taking that attack's stance, while the player dreads not knowing *when* it lands. Today a windup is a hand-written activity with a fixed duration: once seen, it is perfectly predictable, because guards have no randomness. Every attack cycle is rebuilt by hand. On a co-op client the tell is weaker still: behavior-entry and melee attack sounds play only on the host, and re-entering a same-named state shows nothing. Damage cannot cancel a windup without fiddly workarounds: a mod clock in an impact policy, a two-hits-in-one-tick race, and nothing at all for `applyDamage`. When this is done:
- An author declares a stance in one call, naming its pose, hold range, attack and recovery.
- The hold length varies per entry within the author's bounds and replays deterministically on the host.
- Every peer hears stance and melee sounds and sees each re-entry.
- Enough damage during a stance, or during any authored activity, can stagger the enemy.

## Decisions
- **A stance is SDK vocabulary, not an engine noun.** `stance(name, options)` is an author-time builder, as `activation.shot()` is (`scripting.md` §11). It returns a graph fragment `{ entry, activities, transitions }` of plain activities: hold, then release, then recover. The engine sees only activities. This spends the `stance` word that `plans/done/E10--hierarchical-behavior-statecharts` reserved. The builder is the first SDK helper to emit graph structure, and ships in TypeScript and Luau with byte-equal output.
- **One stance names one attack.** The pose is the tell. A stance with several follow-ups is hand-authored from the same parts.
- **Stance cycles take an attack turn.** The builder marks all three activities `attackTurn` (`drafts/E10--enemy-combat-movement`), so one turn spans the cycle. That brief lands first, and this one depends on it.
- **Per-entry roll: `@brain.activityRoll`, uniform in [0, 1).**
  - It is scope-relative like `timeInActivityMs`, drawn when that level's activity is entered and fixed until it is left.
  - The IR stays RNG-free: the roll is a fact the engine computes, not a node.
  - A hold given as `[min, max]` compiles to `timeInActivityMs ≥ lerp(min, max, activityRoll)`. A single number is a fixed hold.
- **The roll is a second carve-out from the randomness posture** (`networking.md`: host-only, load-time, consequences only), and the owner approved it.
  - Brain activity entry rolls on the host only. Only the resulting activities replicate.
  - The seed must survive the determinism gate's spawn-order reversal. Two enemies of one descriptor on the same entry count must not roll alike.
  - `EntityId` fails the first requirement, and descriptor name alone fails the second.
  - This lands in `networking.md` at promotion.
- **Per-activity damage fact: `@brain.damageInActivity`.**
  - It is scope-relative: damage drained from layers and health since that level's activity was entered.
  - It is fed at the damage chokepoint for every producer, `applyDamage` included.
  - Hits that land after the AI pass count on the next evaluation, which is the shipped stage order.
- **Stagger is authored on that fact.** A stance's optional `breakOn` adds the row that leaves the hold for a sibling activity when the fact crosses a threshold. A general stagger is an ordinary scoped `"*"` row. Release is committed and has no break row. This retires `drafts/E10--enemy-stagger`: its tuning block, accumulator and pulse are superseded. Chain-stagger guarding is authored, because the flinch activity resets the fact on entry.
- **Telegraph sounds reach every peer.**
  - An activity's `sound` and a contact attack's `sound` play on each participating client, anchored to the enemy as the observer weapon cue is. The host plays its own as today.
  - This takes the enemy slice of roadmap E12 step 4. Door, crusher and other world sounds stay there.
  - `onEnter` reactions stay host-only. Light and emitter writes already replicate as state.
  - A new wire message bumps `PROTOCOL_ID` (`networking.md` §Version gates).
- **Re-entry is visible.** The replicated animation state carries an entry sequence, and a client restarts the clip whenever the sequence advances, even under the same name. This is a snapshot record change, so `SNAPSHOT_VERSION` is bumped.
- **Non-goals:**
  - **Compensating client latency.** A client sees a stance late by about half the round trip plus one snapshot. Author guidance in `docs/` covers it, and authors set hold minimums with it in mind.
  - **Chance-based entry.** The roll fixes how long, never whether: a roll on entry cannot gate that entry.
  - **An engine-drawn release indicator.** The release activity's own sound and animation are the last-instant cue.

### Scripting surface
```ts
// Proposed design
import { brain, stance } from "postretro";

const overhead = stance("overhead", {
  animation: "stance_overhead",          // the held pose names the attack
  sound: "knight/stance_overhead",       // heard by every peer
  holdMs: [400, 1200],                   // release moment re-rolled each entry
  release: { animation: "attack_overhead", attack: "slam", ms: 150 },
  recover: { animation: "recover", ms: 900 },
  breakOn: { damage: 25, to: "flinch" }, // optional; hold only
  then: "approach",
});

// behavior.activities.engage.layers.offense
offense: {
  initial: "approach",
  activities: {
    approach: { animation: "walk" },
    flinch: { animation: "pain", motion: "hold" },
    ...overhead.activities,
  },
  transitions: {
    "*": [{ to: "flinch", when: brain.damageInActivity.ge(40) }],
    approach: [{ to: overhead.entry, when: brain.targetDistance.le(3.5) }],
    flinch: [{ to: "approach", when: brain.timeInActivityMs.ge(450) }],
    ...overhead.transitions,
  },
},
```
The Luau twin mirrors it field for field. `then` and `breakOn.to` name same-level siblings.

## Acceptance
### Automated
**Stance builder**
- [ ] `stance()` emits identical canonical descriptors in TypeScript and Luau.
- [ ] Its output equals the hand-written hold, release and recover graph it replaces.
- [ ] A `then` or `breakOn.to` naming no sibling rejects load with a pathed error.
- [ ] Bad timings reject load: `holdMs` with min > max, a negative bound, or non-finite values.

**Roll**
- [ ] Across many entries of one enemy, the hold varies and stays within `[min, max]`.
- [ ] A fixed `holdMs` never varies.
- [ ] The roll stays fixed for the life of one entry, and a re-entry draws again.
- [ ] Replaying a run reproduces every roll. The determinism gate's spawn-order reversal reproduces them by role.
- [ ] Two same-descriptor enemies entering on one tick roll differently.

**Damage fact**
- [ ] `damageInActivity` sums every hit since entry, from hitscan, projectile, melee, splash and `applyDamage`, across armor and health. It reads 0 on entry and after a re-entry.
- [ ] Two hits on one tick are both counted.
- [ ] A hit landing after the AI pass is read on the next evaluation, never lost.
- [ ] `breakOn` leaves the hold on the first evaluation at or past the threshold, and never below it.
- [ ] During release, the same damage does not break.
- [ ] Damage taken during `flinch` does not re-trigger the `"*"` stagger until the flinch resets.

**Co-op**
- [ ] On a loopback client, an enemy's activity `sound` and a contact attack's `sound` each play exactly once per host event. The host plays each exactly once.
- [ ] Re-entering a same-named stance restarts the clip on the client.
- [ ] A previous-protocol client is refused.
- [ ] Without stances, every existing AI, replication and determinism test passes unchanged.

### Manual
- [ ] On `movement-feel`, the reference knight with two stances: a player can name the coming attack from the pose, cannot predict the release moment across several cycles, and can break a stance with heavy fire.
- [ ] Co-op loopback: the client hears the stance sound and sees each stance on screen before the hit lands.

## Path
- **Seams.**
  - **Brain facts.** `ScopeRelativeValues` and `SCOPE_RELATIVE_SLOTS` (`crates/ai/src/brain_scope.rs`): a scope-relative fact appends one field and one registration. The per-depth arrays `time_in_activity_ms` and `attacks_fired_in_activity` on `BrainComponent` zero on re-entry. `BRAIN_INPUTS` in foundation `brain.rs` is append-only.
  - **Damage.** The chokepoint is `apply_damage_with_context` (`crates/entities/src/components/health.rs`).
  - **Seeding.** `pellet_rng_seed` and `SplitMix64` (`crates/sim/src/weapon/spread.rs`) avoid `EntityId` by salting with a per-instance counter. The brain needs a per-instance, replay-stable salt.
  - **Observer cues.** `freeze_observer_weapon_cue` and `publish_observer_weapon_cues` skip emissions without a `shot_id`, which is why melee is silent on clients. `AiEmission` has `AiCue::Entered`.
  - **Animation.** `WireMeshAnimationState` and `apply_mesh_animation_state` → `switch_animation_state`.
- **Shape.** Chosen: a fragment builder spread into the author's maps. Rival: a composite activity with a nested graph. That needs an engine "nested graph finished" fact to exit, and it can't hold an attack turn across siblings.
- **First slice.** The roll fact plus its seed. It is the one-way door, and the determinism gate falsifies it fastest.
- **Verify the host first:** check whether a same-named re-entry restarts the clip there. The client must match the host, not exceed it.
- **Wire the limitator too.** It declares `hit_reaction` but leaves it unwired, waiting for "stagger N ms then resume". `damageInActivity` with a flinch activity delivers that. Wire it as a second consumer.

## Open questions
- Seed salt, a per-instance replay-stable identity — **delegated**: must meet both seed requirements in Decisions. Report the choice.
- The new peer-cue message, whether it is a `WeaponCue` sibling or a broader enemy-cue message — **delegated**: one entity-anchored message for both sound kinds. Report it.
