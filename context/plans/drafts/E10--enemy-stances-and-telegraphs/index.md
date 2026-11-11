# E10--enemy-stances-and-telegraphs

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §7c · `context/lib/scripting.md` §1, §11 · `context/lib/networking.md` §Version gates, randomness posture · `context/lib/audio.md` §4 · read at 2cb88013

## Problem
The owner wants For Honor–style attacks, a requested capability. An enemy should telegraph *which* attack is coming by taking that attack's stance, while the player dreads not knowing *when* it lands.

Three things stand in the way today:
- **Timing is predictable.** A windup is a hand-written activity with a fixed duration, so once seen it is perfectly predictable: guards have no randomness. Every attack cycle is also rebuilt by hand.
- **Co-op clients lose the tell.** An activity's `sound` and a melee attack's `sound` play only on the host.
- **Damage can't cancel a windup cleanly.** It takes workarounds: a mod clock in an impact policy, a two-hits-in-one-tick race, and nothing at all for `applyDamage`.

When this is done:
- An author declares a stance in one call, and gets a fair, readable cycle from a pose and an attack alone.
- The hold length varies per entry within bounds the author sets, and replays deterministically on the host.
- A stance can release into movement: a roar before a fast charge.
- Every peer hears stance and melee sounds.
- Enough damage during a stance or another short activity staggers the enemy.

## Decisions
- **A stance is SDK vocabulary, not an engine noun.** `stance(name, options)` is an author-time builder like `activation.shot()` (`scripting.md` §11). It returns a fragment `{ entry, activities, transitions }` of plain activities named `<name>`, `<name>_release` and `<name>_recover`. The engine sees only activities. This spends the `stance` word reserved by `plans/done/E10--hierarchical-behavior-statecharts`.
- **`merge()` composes fragments.** It is an SDK helper, in TypeScript and Luau, that combines activity and transition maps. A duplicate key rejects load with the key named, so nothing overwrites silently. Two maps that both carry `"*"` collide too. Extra exits from a stance go in that level's `"*"` rows.
- **A stance can set the body's stance.** An optional `stance: "stand" | "crouch"` sets the activity property `stance` (`drafts/E10--enemy-cover`) on the hold and release; the recover stands unless it carries its own. A melee enemy crouches to coil a sweep. Whichever of the two briefs lands second wires the option.
- **One stance names one attack.** The pose is the tell. A stance with several follow-ups is hand-authored from the same parts.
- **Defaults for novices.** `holdMs` and `recover` are optional, and experts override them.
  - An omitted `holdMs` takes a seeded range above the co-op floor.
  - An omitted `recover` holds the release clip for a seeded duration.
- **A co-op floor on the tell.** A tell a remote player cannot see is no tell, so the floor is engine-owned; the hold above it is taste. This is the closet-reveal rule from `plans/done/E17--doors-as-occluders` research.
  - **The floor** is the remote interpolation buffer's ceiling (netcode's 250 ms clamp) plus a reaction budget.
  - **Where the constant lives.** The floor is an engine constant outside netcode, exported to both SDKs. A netcode test fails if the floor falls below the buffer ceiling, as with `SPAWN_ATTACK_WINDUP_MS`.
  - **What it doesn't cover.** Network transit has no load-time bound, so the floor leaves it out.
  - **Where it's checked.** `stance()` warns through the script log when a stance's minimum hold falls below the floor. The engine cannot tell a stance's hold from any other activity.
- **A release can move.** `release` takes an optional `motion`, any motion verb, and an optional `speed`; omitted, the release holds. A roar can release into a charge: a fast `chaseTarget` whose contact attack fires on reach.
- **Per-activity speed is an engine field.** Any activity takes an optional `speed`, a positive multiplier on the archetype's `moveSpeed` while it is the innermost activity that sets one. Omitted everywhere, it is 1. The builder writes `release.speed` onto the release only.
- **Stance cycles take an attack turn.** The builder marks all three activities `attackTurn` with the release's attack (`done/E10--enemy-combat-movement`), so one turn for that attack spans the cycle and holds its draws.
  - A tell must never promise an attack that cannot come. So the builder's hold also exits to `then` when `attackTurnHeld(attack)` for the release's attack is false one tick after entry, the facts' lag.
  - Authors gate stance entry on `attackTurnFree(attack)` with the release's attack.
  - **Self-chaining keeps the turn.** A `then` naming the stance's own hold is allowed: recover → hold stays on one mark, so the turn holds for as long as the chain runs and other enemies waiting on its tokens wait. `docs/` says so. Chaining into a stance for another attack releases the turn and waits in line.
- **Per-entry roll: `@brain.activityRoll`, uniform in [0, 1).**
  - It is scope-relative like `timeInActivityMs`, drawn on that level's entry and fixed until that level's activity is left.
  - It is a fact the engine computes, not an IR node.
  - Its reach is intended: any guard at that level reads it, including outgoing rows. So chance-based branching, random patrol and chance-based pain are authorable.
- **The roll is a second exception to the randomness posture** (`networking.md`), and the owner approved it.
  - Rolls are drawn on the host only. No roll or seed crosses the wire. Clients see only what follows from it: animation states and cues.
  - The seed survives a reversed spawn order of same-descriptor enemies, and two such enemies on one entry roll differently.
  - It lands in `networking.md` at promotion, described at its full reach.
- **Per-activity damage fact: `@brain.damageInActivity`.** It is scope-relative: damage drained from layers and health since that level's activity was entered, for every producer, `applyDamage` included.
- **Stagger is authored on that fact, for short activities.**
  - A stance's optional `breakOn` adds a row leaving the hold for a sibling activity. The same guard works on any short activity, such as an aim.
  - Release is committed: it has no break row, and only the level's `"*"` rows can leave it. Those rows outrank the stance's own rows, as they do every activity's.
  - On a long activity the sum grows without limit, so this brief does not offer stagger mid-chase.
  - This supersedes the deleted `E10--enemy-stagger` draft.
- **Telegraph sounds reach every peer.**
  - An activity's `sound` field and a contact attack's `sound` play once on each participating client, anchored to the enemy like the observer weapon cue. The host plays its own as today.
  - This takes the enemy slice of roadmap E12 step 4.
  - `onEnter` reactions are unchanged: a reaction's `playSound` already reaches every peer.
- **Clients match the host on clips.** The host restarts nothing when the next activity names the clip already playing, and neither does a client. No re-entry signal is replicated.
- **Shipped content adopts the vocabulary in this brief.**
  - The reference brute (`models/mutant`, library `mixamo_brute`) telegraphs every attack with a stance, one attack each:
    - **overhead:** holds `smash_windup`, releases `smash` for `slam`;
    - **lunge:** holds `swipe_windup`, releases `swipe` for a new lunge attack;
    - **jab:** holds `punch_windup`, releases `punch` for `jab`;
    - **roar:** holds `roar`, releases a fast `chaseTarget` charge for a new charge attack.
  - The limitator's roll picks each cycle's branch: today's strafe and fire, or reposition, stop and aim. The aim is a stance holding `idle_aiming` whose release is the `shoot` burst, with a `breakOn` into its unwired `hit_reaction` flinch.
  - `sdk/behaviors/reference` shows a stance.
  - `docs/` explains stances and the co-op floor. It also says a client sees a stance late by about half the round trip plus one snapshot, which the floor does not cover.
  - Tests pinned to the old hand-written cycle are updated.
- **Non-goals:**
  - **Compensating client latency.** It would mean predicting enemy behavior on clients, and graph evaluation is host-only (`entity_model.md` §7c). The floor and `docs/` cover authors instead.
  - **Stagger mid-chase.** A recency-windowed damage fact can be appended later.
  - **Player guard and parry.** "For Honor–style" here means the enemy's tell, not a directional player defense.
  - **An engine-drawn release indicator.** The release activity's own sound and animation are the last-instant cue.

### Scripting surface
```ts
// Proposed design
import { attackTurnFree, brain, merge, stance } from "postretro";

const overhead = stance("overhead", {
  animation: "stance_overhead",          // the held pose names the attack
  sound: "brute/stance_overhead",        // heard by every peer
  holdMs: [500, 1200],                   // optional; release moment re-rolled each entry
  release: { animation: "attack_overhead", attack: "slam", ms: 150 },
  recover: { animation: "recover", ms: 900 }, // optional
  breakOn: { damage: 25, to: "flinch" }, // optional; hold only
  then: "approach",
});
const sweep = stance("sweep", {
  animation: "stance_low",
  stance: "crouch",                      // optional body stance on hold and release
  release: { animation: "attack_sweep", attack: "jab", ms: 120 },
  then: "approach",
});
const roar = stance("roar", {
  animation: "roar",
  release: { animation: "run", attack: "charge", motion: "chaseTarget", speed: 2, ms: 1500 },
  then: "approach",
});

// behavior.activities.engage.layers.offense
offense: {
  initial: "approach",
  activities: merge(
    { approach: { animation: "walk" }, flinch: { animation: "pain", motion: "hold" } },
    overhead.activities,
    sweep.activities,
    roar.activities,
  ),
  transitions: merge(
    {
      approach: [
        // a slam also waits on `heavy`; the sweep's jab only on `melee`
        { to: overhead.entry, when: attackTurnFree("slam").and(brain.targetDistance.le(3.5)).and(brain.activityRoll.lt(0.5)) },
        { to: sweep.entry, when: attackTurnFree("jab").and(brain.targetDistance.le(3.5)) },
        { to: roar.entry, when: attackTurnFree("charge").and(brain.targetDistance.ge(8)) },
      ],
      flinch: [{ to: "approach", when: brain.timeInActivityMs.ge(450) }],
    },
    overhead.transitions,
    sweep.transitions,
    roar.transitions,
  ),
},
```
The Luau twin calls the same builders with Luau syntax (`brain.targetDistance:le(3.5)`). `then` and `breakOn.to` name same-level siblings.

## Acceptance
### Automated
**Builder**
- [ ] `stance()` and `merge()` emit identical canonical descriptors in TypeScript and Luau, and the Scripting surface example runs as `content/dev` content.
- [ ] With a fixed `holdMs` and a `breakOn`, `stance()` output equals a checked-in hand-written fixture of the three activities and their rows, row order included.
- [ ] `stance: "crouch"` sets `stance` on the hold and release only. The recover stands even under an outer crouch, and a recover given its own `stance` keeps it.
- [ ] `merge()` with a duplicate activity or transition key rejects load naming the key.
- [ ] These reject load with a pathed error:
  - a `then` or `breakOn.to` naming no sibling, or naming the stance's own `_release`;
  - a `breakOn.to` naming the stance's own hold;
  - `holdMs` with min > max, or a negative bound;
  - a `breakOn.damage` of zero or below;
  - a negative `release.ms` or `recover.ms`;
  - an activity or `release` `speed` of zero or below;
  - any non-finite value.
- [ ] A stance whose minimum hold is below the co-op floor loads with one warning naming it, `holdMs: 0` included. One at or above loads silently.
- [ ] A `then` naming the stance's own hold loads and chains back-to-back stances on one turn: a waiter for the same token is not granted while the chain runs, and is granted after the chain leaves for an unmarked activity.
- [ ] `release.motion` and `release.speed` land on the release only; omitted, the release holds at speed 1.
- [ ] A test fails if the co-op floor falls below the remote interpolation buffer ceiling, and passes when the floor is above it.
- [ ] A stance given only `animation`, `release` and `then` loads silently. It gets the seeded hold range, above the floor, and a recovery that holds the release clip for the seeded duration.
- [ ] All three emitted activities carry `attackTurn`, naming the release's attack. A stance entered without a turn, having lost a same-tick grant, leaves its hold for `then` on the first evaluation that can see the missing turn. Its release never fires.

**Roll**
- [ ] Across many entries, the hold's threshold varies within [min, max], and the hold ends on the first tick at or past it. A fixed `holdMs` never varies.
- [ ] `holdMs: 0` leaves on the first evaluation after entry, and `[x, x]` holds like a fixed `x` (pin P12).
- [ ] The roll is fixed for one entry, and a re-entry draws again. An activity reseated and left on one tick reads its own fresh roll (pin P3).
- [ ] Levels seated by one descent draw independent rolls. A nested transition redraws and zeroes only the nested level (pin P4).
- [ ] Selector guards read their own level's roll and damage sum (pin P5).
- [ ] On one entry of `approach`, both branches of the example read the same roll.
- [ ] The determinism gate covers two same-descriptor enemies under role labels. It reverses their spawn order, and every roll matches by role.
- [ ] Two same-descriptor enemies spawned on one tick read different rolls on their first evaluation (pin P13).
- [ ] Grep gate: no wire record or message carries a roll or its seed.

**Damage fact and stagger**
- [ ] `damageInActivity` sums every hit since entry, across armor and health. The covered sources are hitscan, projectile, melee, splash and `applyDamage`. Two hits on one tick both count.
- [ ] A hit after the AI pass on the tick an activity is entered counts toward that activity on its first evaluation. A hit before the AI pass that tick counts only toward the activity left (pin P2).
- [ ] `breakOn` leaves the hold on the first evaluation at or past the threshold, never below it. When the timer and threshold pass together, the break wins and the release never fires (pin P1).
- [ ] During release, the same damage does not break. A level `"*"` row that passes during release or recover leaves it, outranking the stance's own rows. Damage taken in `flinch` does not carry into the activity it resumes.
- [ ] Enough damage during the limitator's aim enters `hit_reaction`, and its flinch resumes on its timer. Across many cycles its roll takes both the strafe-and-fire and the reposition-and-aim branch.

**Speed**
- [ ] An activity with `speed: 2` steers its agent at twice `moveSpeed` while active and at `moveSpeed` once left. An inner activity's `speed` overrides an outer one's; an inner activity without one inherits it.
- [ ] A roar stance's charge release chases at its speed and lands its contact attack on reach, once per entry.
- [ ] A brain serialized before these fields deserializes. Its sums read 0, and its next entry draws a roll.

**Co-op**
- [ ] On a loopback client, an activity's `sound` and a contact attack's `sound` play exactly once per host event, and the host plays each exactly once.
- [ ] An enemy weapon attack's sound still plays exactly once (pin P10).
- [ ] An A → B → A sequence inside one snapshot interval plays A's sound twice on each peer (pin P7).
- [ ] A cue for an entity the client lacks plays once at the host-captured position and never replays (pin P9).
- [ ] An enemy killed in the apply pass that enters its stance plays no stance sound on any peer (pin P11).
- [ ] A client receiving a cue runs no `onEnter` reaction.
- [ ] A client that begins participating on the tick an enemy enters its stance plays that stance's sound at most once, and only from a cue. Materializing an enemy mid-stance plays no sound (pin P14).
- [ ] A previous-protocol client is refused.
- [ ] Entering an activity whose clip is already playing restarts nothing on host or client.

### Manual
- [ ] On `movement-feel`, the reference brute with four stances: a player names the coming attack from the pose, cannot predict the release across several cycles, and breaks a stance with heavy fire. The roar reads as a charge coming.
- [ ] On `movement-feel`, the limitator sometimes strafes and fires, sometimes repositions, stops and aims; heavy fire during the aim flinches it.
- [ ] Co-op loopback: the client hears each stance sound and sees the stance before the hit lands.

## Path
- **Body stance.** A crouching stance needs the cover brief's `crouchHeight` and `crouch_` states on the archetype, which load enforces. Under innermost-wins resolution the builder emits `stance: "stand"` on the recover when the option is set, so an outer crouch never leaks into it.
- **Assets.** Today the brute's jab and slam both play `punch`; `reference-enemy.ts` nests the jab in a `jabs` subgraph under `hunt`/`press` and marks the slam's windup, commit and recover `attackTurn: "slam"`. The brute uses its authored hitbox: its model carries no hit zones (the converter zones only head and legs, which would leave the torso unhittable). Its library carries `smash` (a jump attack pinned in place), `swipe`, `punch`, `roar`, `hit_reaction`, and the holds `smash_windup` (400 ms, the crouch before the leap) and `swipe_windup` (900 ms, the lean back before the lunge). Holds are cut by `mixamo_to_gltf.py --trim`, and a non-looping clip clamps on its last frame (`resolve_time`, `crates/model/src/anim/types.rs`), so the pose holds with no engine change. `punch_windup` does not exist yet, and the Mixamo sources are not in the repo (see Open questions). The limitator's rifle library carries `idle_aiming`, and its `hit_reaction` is bound but unwired. Stance sounds have no assets yet; the dev fixture sounds stand in. An engine `untilMs`-style clip end is a follow-up, not this brief.
- **Seams.**
  - **Brain facts.** `ScopeRelativeValues` / `SCOPE_RELATIVE_SLOTS` (`crates/ai/src/brain_scope.rs`): one field and one registration per fact. `repoint_scope_relative` re-points them per depth in `select_transition_path` and in both selectors, each building the struct literally. `BrainScope::refresh` and the typedef templates spell `BRAIN_INPUTS` out positionally too.
  - **Brain arrays.** The per-depth arrays on `BrainComponent` zero on re-entry. `BRAIN_INPUTS` is append-only.
  - **Damage.** `apply_damage_with_context` already writes the brain for every producer, through `record_damage_perception` on a live target. Its `drain.points_dealt` is layer-absorbed plus health loss (`drain_layers`). A same-level row naming the current activity is skipped (`first_enabled_target`), so an activity re-enters only through a sibling or an ancestor.
  - **Seeding.** `pellet_rng_seed` / `SplitMix64` salt with a per-instance counter rather than `EntityId`. The initial activity is seated before spawn state is set (pin P13).
  - **Melee is silent on clients** because `publish_observer_weapon_cues` skips emissions without a `shot_id`.
  - **The determinism gate's** single enemy spawns after both pawns, so today's reversal cannot catch an `EntityId` seed.
- **Shape.** A fragment builder plus `merge()`. The rival, a composite activity with a nested graph, needs an engine "nested graph finished" fact to exit, and can't hold a turn across siblings.
- **First slice.** The roll fact, its seed, and the extended determinism gate: the one-way door, falsified fastest.
- **Attack turns.** A turn is requested and granted in tick N's arbitration and reads held on N+1 (`arbitrate_attack_turns`, `TurnFactReader::read`). A drawn attack under a mark never fires without the turn (the hard gate in `arbitrate_attack_turns`). A turn's refresh starts when the turn ends, fired or not, so a broken hold spends its tokens' refresh.
- **Speed.** Steering reads `move_speed` on the agent, set once at spawn (`attach_agent`, `crates/sim/src/scripting/builtins/data_archetype.rs`; `agent_steering.rs`).

## Open questions
- Seed salt, a per-instance replay-stable identity — **delegated**: meet both seed requirements in Decisions and report the choice.
- The seeded hold and recovery defaults, and the floor's reaction budget — **delegated**: tune on `movement-feel` under the lag profile and report.
- The brute's new lunge and charge attacks (names, damage, draws), the charge speed, and how its offense graph nests the four stances — **delegated**: tune on `movement-feel` and report.
- `punch_windup` — **blocks build** of the jab stance only: the owner re-exports `mixamo_brute` from the Mixamo sources with a `--trim punch_windup=punch:<ms>` cut at the cocked fist. The other stances build without it.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| stance builder | none (SDK only) | — | `stance(name, options)` | `stance(name, options)` | n/a |
| fragment keys | none | — | `entry`, `activities`, `transitions` | same | n/a |
| stance options | none | — | `animation`, `sound`, `stance`, `holdMs`, `release { animation, attack, ms, motion, speed }`, `recover { animation, ms }`, `breakOn { damage, to }`, `then` | same | n/a |
| generated names | none | activity keys `<name>`, `<name>_release`, `<name>_recover` | same | same | n/a |
| activity speed | `BehaviorActivityDescriptor::speed: Option<f32>` | `"speed"` | `speed?: number` | `speed` | n/a |
| merge helper | none | — | `merge(...maps)` | `merge(...)` | n/a |
| roll fact | `BRAIN_INPUTS` tail | `"@brain.activityRoll"` | `brain.activityRoll` | `brain.activityRoll` | n/a |
| damage fact | `BRAIN_INPUTS` tail, after the roll | `"@brain.damageInActivity"` | `brain.damageInActivity` | `brain.damageInActivity` | n/a |

These facts append after `hasAttackTurn` (slot 20).

## Wire format
**Enemy cue message, host to clients.** It follows `WeaponCuesMessage`:
- the same codec and the same reliable-ordered channel;
- one cue per message, sent when the host event fires, so no batch cap can drop one;
- sent to every participating client.

Fields, in order:
- **participation epoch:** as `WeaponCuesMessage` carries it;
- **anchor:** the enemy's `NetworkId` with the captured origin, or the captured origin alone when the enemy has no network id. No contact list;
- **sound:** required, encoded as `WeaponCue.sound`'s string.

Weapon attack sounds stay on `WeaponCue`. The new message is appended to the server message enum under an append-layout guard, and bumps `PROTOCOL_ID`. No snapshot record changes.
