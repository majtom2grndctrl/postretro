# E10--enemy-stances-and-telegraphs

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §7c · `context/lib/scripting.md` §1, §11 · `context/lib/networking.md` §Version gates, randomness posture · `context/lib/audio.md` §4 · read at 2cb88013

## Problem
The owner wants For Honor–style attacks, a requested capability. An enemy telegraphs an attack by taking its stance. A stance may lead to more than one attack, so the player reads the threat from the pose but dreads not knowing the exact strike or *when* it lands.

These stand in the way today:
- **Timing is predictable.** A windup is a hand-written activity with a fixed duration, so once seen it is perfectly predictable: guards have no randomness. Every attack cycle is also rebuilt by hand.
- **Co-op clients lose the tell.** An activity's `sound` and a melee attack's `sound` play only on the host.
- **Attacks are strings.** An attack is a key in the behavior's `attacks` map, and every reference to it (`action`, `attackTurn`, `attackTurnFree`, `attackTurnHeld`) repeats the string, so a typo surfaces at load, not at compile. The owner prefers typed handles in SDK APIs.
- **Damage can't cancel a windup cleanly.** It takes workarounds: a mod clock in an impact policy, a two-hits-in-one-tick race, and nothing at all for `applyDamage`.

When this is done:
- An author declares a stance and its attacks as typed handles, and gets a fair, readable cycle from a pose and an attack alone.
- The hold length varies per entry within bounds the author sets, and replays deterministically on the host.
- A stance can release into movement: a roar before a fast charge.
- Every peer hears stance and melee sounds.
- Enough damage during a stance or another short activity staggers the enemy.

## Decisions
- **Attacks are typed handles.** `defineAttack(params)` declares an attack; in TypeScript the binding names it, as `defineAttackToken` and `defineDamageType` do (`scripting.md` §2), and Luau passes the name explicitly. A behavior lists its attacks, `attacks: [jab, slam]`, and every reference takes the handle: `action: { attack }`, `attackTurn`, `attackTurnFree`, `attackTurnHeld`, and an attack's `stance`.
  - The string form retires. Shipped content, `sdk/behaviors/reference` and `docs/` move to handles in this brief.
  - A handle crosses as its name. The SDK builds the descriptor's name-keyed `attacks` map, so the wire and the runtime are unchanged; only the SDK, its generated types and its Rust-side type registry change.
  - Handles lower to names wherever an entity enters the mod: `defineEntity` and `defineMod`'s `entities` both lower them, and both reject a bare string with a pathed error, in TypeScript as in Luau.
  - One handle may serve several archetypes. Two handles of one name in one archetype load only if their declarations are equal, since a Luau re-`require` makes copies; unequal declarations reject load.
  - A misspelled handle must be observable to agents. In Luau a misspelled global reads `nil`, so a nil where a handle is expected rejects with a pathed error, and a nil table field, which reads as absent, is caught by a check agents run.
- **A stance is SDK vocabulary, not an engine noun.** `defineStance(options)` declares a pose: its animation state, its sound, its hold range and its `breakOn`. It is named by its binding like `defineAttack`. The engine sees only activities. This spends the `stance` word reserved by `plans/done/E10--hierarchical-behavior-statecharts`.
- **An attack names its stance.** A stance is a prerequisite of an attack. An attack's optional `stance` names the pose it starts from, and the attack owns its strike: `release`, `recover` and `after`. `release` is required exactly when `stance` is set; either alone rejects load. Several attacks may share one stance, so the pose names the threat and the strike stays a mix-up.
- **An attack may land in another stance.** An attack's optional `after` names where it lands when its recovery ends: a sibling activity, or a `Stance`, whose pose it then rests in. So a slam can land coiled, ready for a lunge. Omitted, the attack returns to the activity that started it.
- **Rows target attacks and stances directly.** A row's `to` takes an `Attack` or a `Stance` as well as an activity name: `{ to: slam }` strikes, and `{ to: overhead }` rests in the pose without committing to an attack. `defineEntity` and `defineMod` expand each targeted attack and stance into activities in the level that targets it; authors never write or name them.
- **Resting in a stance.** A stance's rest is an unmarked activity holding the pose. Rows leaving it are authored under the stance's name, as any activity's are, and a row from it to an attack on that stance commits from the same pose. A higher-ranked enemy rests to wait for its turn, so the release stays only partly predictable; a basic enemy strikes straight from its approach. A rest with no way out (no row of its own, and no `"*"` or ancestor row) rejects load.
- **The enemy commits when it strikes.** An attack's turn must be held through its tell (below), so a strike commits on entry. Each struck attack gets its own hold, `<attack>_<stance>` (so `slam_overhead` names the tell in traces), posed identically to that stance's rest and every other attack on it, then `<attack>_release` and `<attack>_recover`. A pose plays its clip and sound once: moving from a rest into an attack on the same stance restarts neither. Two enemies racing for a shared stance can drop the pose for a tick and re-enter through the sibling attack, playing its sound twice; that is accepted (pin P24).
- **Return is per start.** An attack with no `after` returns to the activity that started it. When several activities strike it, the SDK generates one cycle per starting activity; traces still read `<attack>_<stance>`.
- **A stance can set the body's stance.** An optional `stance: "stand" | "crouch"` on `defineStance` sets the activity property `stance` (`drafts/E10--enemy-cover`) on the rest, the holds and the releases; a recover always stands. Whichever of the two briefs lands second wires the option.
- **Defaults for novices.** `holdMs` and `recover` are optional, and experts override them.
  - An omitted `holdMs` takes a seeded range above the co-op floor.
  - An omitted `recover` holds the release clip for a seeded duration.
- **A co-op default for the tell.** A tell a remote player cannot see is no tell, so the seeded default hold starts above a co-op floor; an authored hold is taste. This follows the closet-reveal rule from `plans/done/E17--doors-as-occluders` research, as a default rather than a guarantee: the engine cannot tell a stance's hold from any other activity, so it cannot enforce one.
  - **The floor** is the remote interpolation buffer's ceiling (netcode's 250 ms clamp) plus a reaction budget.
  - **Where the constant lives.** The floor is an engine constant outside netcode. A netcode test fails if the floor falls below the buffer ceiling, as with `SPAWN_ATTACK_WINDUP_MS`.
  - **What it bounds.** The seeded default hold never starts below it. The builder's default is an SDK literal, so a test evaluates `defineStance` without `holdMs` in both runtimes and fails if its minimum falls below the engine constant. An authored `holdMs` may go lower: the SDK carries no exported constant and no warning, and `docs/` states the number.
  - **What it doesn't cover.** Network transit has no load-time bound, so the floor leaves it out.
- **The pose is planted.** The SDK writes `motion: "hold"` on every rest, hold, release and recover it generates, so a stance never inherits its level's motion.
- **A release can move.** `release` takes an optional `motion` (`hold`, `chaseTarget`, `strafe` or `reposition`; a position goal cannot carry an attack) and an optional `speed`; they replace the release's planted hold. A roar can release into a charge: a fast `chaseTarget` whose contact attack fires on reach.
  - A moving release requires `recover`: the default recovery holds the release's state, which would freeze a stride.
  - A moving release's state rate-scales against its own `travelSpeed`, as a locomotion state does, so a charge's stride matches its speed.
- **Per-activity speed is an engine field.** Any activity takes an optional `speed`, a positive multiplier on the archetype's `moveSpeed` while it is the innermost activity that sets one. Omitted everywhere, it is 1. The builder writes `release.speed` onto the release only.
  - **Witting divergence.** `done/E10--hierarchical-behavior-statecharts` and `done/E10--enemy-behavior-descriptor-syntax/sample.ts` placed an absolute per-activity `speed` that graph-wide `moveSpeed` would migrate into. A multiplier lets one attack's release serve archetypes of different speeds, so the owner chose it (2026-11-11) and gave up that endpoint.
- **A strike takes an attack turn.** The SDK marks an attack's hold, release and recover `attackTurn` with that attack (`done/E10--enemy-combat-movement`), so one turn spans the cycle and holds its draws. A rest is unmarked and holds no turn.
  - A strike never releases without its turn. So each hold also leaves when `attackTurnHeld` for its attack is false one tick after entry, the facts' lag, back to where the strike started: the activity or the rest. The turn is the whole promise: cooldown, range, sight and facing still gate the strike, and `docs/` says a release can whiff.
  - Authors gate each `{ to: attack }` row on `attackTurnFree(attack)`.
  - **A strike works wherever it sits.** While an activity marked `attackTurn` is on the path, the enemy keeps its target and committed aim as it does under an engaging ancestor, so a strike outside `engage` still releases (`index.md` §2, Designers over structure).
  - **Chains land in a rest.** An `after` may name the attack's own stance or another; the enemy lands in that rest, unmarked, so no turn is held across a chain. An `after` naming an activity the SDK generates rejects load.
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
  - A stance's optional `breakOn` adds a row leaving its rest and each of its holds for a sibling activity. The same guard works on any short activity, such as an aim.
  - Release is committed: it has no break row. Only its own timer, the level's `"*"` rows and ancestor levels' rows leave it; the latter two outrank the generated rows, as they do every activity's.
  - This supersedes the deleted `E10--enemy-stagger` draft.
- **Telegraph sounds reach every peer.**
  - An activity's `sound` field and a contact attack's `sound` reach each participating client as a positioned `PlaySound` presentation command at the enemy's host-captured origin: the route an `onEnter` `playSound` already takes. The host plays its own once, as today.
  - Presentation is unreliable, so a dropped packet can lose a sound. The pose replicates as animation state and still tells. No new message kind and no version bump.
  - This takes the enemy slice of roadmap E12 step 4, on the payload that step extends.
  - `onEnter` reactions are unchanged.
- **Release clips wait on Clip-timed attacks.** A release must play the rest of its attack clip, not replay the windup from frame 0, and strike on the clip's moment rather than on entry. Both belong to the roadmap's Clip-timed attacks, which builds first. The slam, lunge and jab releases wait on it; the builder, the facts, the sounds, the limitator's aim and the charge do not, since their holds and releases are separate clips.
- **Clients match the host on clips.** The host restarts nothing when the next activity names the clip already playing, and neither does a client. No re-entry signal is replicated.
- **Shipped content adopts the vocabulary in this brief.**
  - Characters use each clip for what it depicts (`resource_management.md` §7). The brute declares an animation state per stance pose and release, each playing its namesake clip. Its locomotion state today plays `run` at every pace; instead it walks on a `walk` state under a `speed` below 1 where it strolls, and runs on a `run` state where it chases and charges.
  - The reference brute (`models/mutant`, library `mixamo_brute`) telegraphs every attack with a stance. Its tells are **overhead** (`smash_windup`), **coil** (`swipe_windup`), **cock** (`punch_windup`) and **bellow** (`roar`); its strikes are `slam` (`smash`), a new lunge (`swipe`), `jab` (`punch`) and a new charge (a fast `chaseTarget` on `run`). At least one tell leads to two strikes, proving the mix-up; the pairing is tuned at build.
  - The limitator's roll picks each cycle's branch: today's strafe and fire, or reposition, stop and aim. The aim is a stance holding `idle_aiming` that the `shoot` attack names; its release is the burst, with a `breakOn` into its unwired `hit_reaction` flinch.
  - `sdk/behaviors/reference` shows typed attacks and a shared stance.
  - `docs/` explains stances, the co-op floor's number and that a release can whiff. It also says a client sees a stance late by about half the round trip plus one snapshot, which the floor does not cover.
  - Tests pinned to the old hand-written cycle are updated.
- **Non-goals:**
  - **Compensating client latency.** It would mean predicting enemy behavior on clients, and graph evaluation is host-only (`entity_model.md` §7c). The floor and `docs/` cover authors instead.
  - **Stagger mid-chase.** On a long activity the sum grows without limit; a recency-windowed damage fact can be appended later.
  - **Player guard and parry.** "For Honor–style" here means the enemy's tell, not a directional player defense.
  - **An engine-drawn release indicator.** The release activity's own sound and animation are the last-instant cue.
  - **Typed activity, state and sound names.** `after`, `breakOn.to`, `animation` and `sound` stay strings: activities are keys in the same file's maps, and states and sounds resolve against content. Attacks cross files and archetypes, which is where a typo hid.

### Scripting surface
```ts
// Proposed design. Illustrative: a brute-like offense; the shipped brute's graph is the executor's.
// Each `animation` names a state the archetype declares, here each playing its namesake clip.
import { attackTurnFree, brain, defineAttack, defineStance } from "postretro";
import { heavy, melee } from "./attack-tokens";

// Poses. Each is named by its binding: "overhead", "coil", "bellow".
const overhead = defineStance({
  animation: "smash_windup",             // the held pose names the threat
  sound: "brute/stance_overhead",        // heard by every peer, once per pose
  holdMs: [500, 1200],                   // optional; release moment re-rolled each strike
  breakOn: { damage: 25, to: "flinch" }, // optional; rest and holds
});
const coil = defineStance({ animation: "swipe_windup" });
const bellow = defineStance({ animation: "roar" });

// Strikes. Two start from `overhead`: same pose, different hit.
const slam = defineAttack({
  draws: [melee.with({ cost: 2 }), heavy], damage: 14, maxRange: 3.5, cooldownMs: 1800,
  stance: overhead,
  release: { animation: "smash", ms: 150 },
  recover: { animation: "idle", ms: 300 },
  after: coil,                           // lands coiled, ready to lunge
});
const sweep = defineAttack({
  draws: [melee], damage: 8, maxRange: 3.5, cooldownMs: 1200,
  stance: overhead,
  release: { animation: "swipe", ms: 120 }, // after omitted: back where it started
});
const lunge = defineAttack({
  draws: [melee], damage: 10, maxRange: 3.5, cooldownMs: 1500,
  stance: coil,
  release: { animation: "swipe", ms: 120 },
});
const charge = defineAttack({
  draws: [heavy], damage: 18, maxRange: 1.5, cooldownMs: 4000,
  stance: bellow,
  release: { animation: "run", motion: "chaseTarget", speed: 2, ms: 1500 },
  recover: { animation: "idle", ms: 600 }, // required after a moving release
});

// behavior: { attacks: [slam, sweep, lunge, charge], ... }
// behavior.activities.engage.layers.offense
offense: {
  initial: "approach",
  activities: {
    approach: { animation: "walk" },
    flinch: { animation: "flinch", motion: "hold" },
  },
  transitions: {
    approach: [
      // one roll per entry of `approach`
      { to: slam, when: attackTurnFree(slam).and(brain.targetDistance.le(3.5)).and(brain.activityRoll.lt(0.4)) },
      { to: overhead, when: brain.targetDistance.le(3.5) },  // rest in the pose: dread
      { to: charge, when: attackTurnFree(charge).and(brain.targetDistance.ge(8)) },
    ],
    overhead: [                          // rows out of the rest, under the stance's name
      { to: slam, when: attackTurnFree(slam).and(brain.activityRoll.lt(0.5)) },
      { to: sweep, when: attackTurnFree(sweep) },
      { to: "approach", when: brain.targetDistance.gt(4.5) },
    ],
    coil: [
      { to: lunge, when: attackTurnFree(lunge) },
      { to: "approach", when: brain.timeInActivityMs.ge(800) },
    ],
    flinch: [{ to: "approach", when: brain.timeInActivityMs.ge(450) }],
  },
},
```
The Luau twin passes names explicitly (`local overhead = defineStance("overhead", { ... })`, `defineAttack("slam", { ... })`) and writes guards with Luau syntax (`brain.targetDistance:le(3.5)`). A stance's rest is keyed by its name in `transitions`, as any activity is. `after` and `breakOn.to` take sibling activity names, and `after` also a `Stance`; `then` is avoided because it is a Luau keyword.

## Acceptance
### Automated
**Typed attacks**
- [ ] In TypeScript, `const slam = defineAttack({ ... })` compiles to `defineAttack("slam", { ... })`, and an explicit name is kept; `defineStance` compiles the same way, and its name appears in its holds' keys. A `defineAttack` or `defineStance` call outside a direct top-level binding throws the binding-name diagnostic. Luau passes the name and produces the identical descriptor.
- [ ] A behavior listing `attacks: [jab, slam]` serializes to the same name-keyed `attacks` map a hand-written descriptor carries today; the engine-side descriptor is unchanged.
- [ ] A string where an attack handle is expected (the behavior's `attacks` list or a name-keyed `attacks` map, `action.attack`, `attackTurn`, `attackTurnFree`, `attackTurnHeld`, an attack's `stance`, a row's `to` naming an attack) fails type-checking in TypeScript and, in both runtimes, rejects load with a pathed error, through `defineEntity` and through a bare entity in `defineMod`. A handle not listed in the archetype's `attacks` rejects load naming it. Two listed handles sharing a name load when their declarations are equal and reject otherwise.
- [ ] A `nil` passed where a handle is expected (`attackTurnFree`, `attackTurnHeld`) rejects with a pathed error, and a misspelled Luau handle in a table field is reported by the check agents run.
- [ ] One handle listed by two archetypes loads in both, and each resolves it by name.
- [ ] Every shipped enemy, `sdk/behaviors/reference` and `docs/` use handles; grep gate: no shipped script, `docs/` page or generated typedef comment passes or shows an attack as a string.

**Expansion**
- [ ] `defineStance`, `defineAttack` and the expanded behavior emit identical canonical descriptors in TypeScript and Luau, and the Scripting surface example runs as `content/dev` content.
- [ ] With a fixed `holdMs` and a `breakOn`, a level whose row targets one attack expands to a checked-in hand-written fixture of its rest-free cycle (hold, release, recover) and their rows, row order included; a level whose row targets a stance adds that stance's rest.
- [ ] `{ to: slam }` enters `slam`'s hold; `{ to: overhead }` enters `overhead`'s rest, which holds no turn; a row from the rest to an attack on that stance enters its hold with no clip restart and no second play of the stance sound.
- [ ] A level gains only the rests and cycles its own rows (and its attacks' `after`s) reach; a level that targets nothing gains nothing. An authored activity whose name collides with a generated one rejects load naming both.
- [ ] Two attacks sharing a stance get separate holds with the same animation state, sound, hold range, `breakOn` row and body `stance` as the stance's rest; each hold carries its own attack's `attackTurn`.
- [ ] Two enemies racing for a shared stance under one token: the loser leaves its hold, re-enters through the sibling attack when that turn is free, and plays the stance sound once per entry (pin P24).
- [ ] An attack with `after: coil` lands in `coil`'s rest when its recovery ends, holding no turn; one with `after: "approach"` lands there; one with no `after` returns to the activity that started it. Struck from two activities, it gets one cycle per start and each returns to its own.
- [ ] An attack whose `after` names its own stance lands in that rest, and its turn ends there: a waiter for the same token is granted once the refresh lapses, though the enemy re-strikes from the rest.
- [ ] A rest with no row of its own and no `"*"` or ancestor row rejects load naming the stance.
- [ ] Once `E10--enemy-cover` has landed, `stance: "crouch"` sets `stance` on the rest, the holds and the releases. Recovers stand even under an outer crouch. Before then, the option rejects load with a pathed error.
- [ ] These reject load with a pathed error:
  - an `after` or `breakOn.to` naming no sibling activity, or naming an activity the SDK generates;
  - a row targeting an attack with no `stance`, or an attack or stance not declared by the archetype;
  - `holdMs` with min > max, or a negative bound;
  - a `breakOn.damage` of zero or below;
  - a negative `release.ms` or `recover.ms`;
  - an activity or `release` `speed` of zero or below;
  - a `release.motion` other than `hold`, `chaseTarget`, `strafe` or `reposition`, naming `release.motion`;
  - a moving `release` without `recover`;
  - an attack with `stance` and no `release`, or `release` and no `stance`;
  - any non-finite value.
- [ ] An attack whose `after` lands in another stance's rest ends its turn on that transition's tick, or when its weapon execution ends if still running; a strike from that rest requests the second attack's turn, and without a grant its hold returns to the rest and its release never fires.
- [ ] A strike at the root level beside a chase, outside any engaging ancestor, keeps its target and committed aim through a co-op target re-rank and one tick out of sight, and releases (pin P23).
- [ ] Every generated rest, hold, release and recover carries `motion: "hold"`. `release.motion` and `release.speed` replace it on the release only; omitted, the release holds at speed 1. A strike entered from a chasing level stays planted.
- [ ] A test fails if the co-op floor falls below the remote interpolation buffer ceiling, and passes when the floor is above it.
- [ ] A test fails if the builder's default hold minimum, read from `defineStance` in each runtime, falls below the co-op floor.
- [ ] A stance given only `animation`, and an attack given only its engine fields, `stance` and `release`, load silently. The hold gets the seeded range, above the floor, and the recover holds the release's state for the seeded duration.
- [ ] A strike's hold, release and recover carry `attackTurn`, naming their attack; a rest carries none. A strike entered without a turn, having lost a same-tick grant, leaves its hold for where it started (the activity or the rest) on the first evaluation that can see the missing turn. Its release never fires.
- [ ] When the hold's timer and its missing turn pass on one evaluation, the hold leaves for where it started and the release is never entered (pin P16).
- [ ] When the break threshold and the missing turn pass on one evaluation, the hold leaves for `breakOn.to` (pin P22).
- [ ] A strike hold seated by a spawn or a reseat, with a hold threshold of 0, does not enter its release before its turn reads held (pin P21).
- [ ] `release.ms: 0` and `recover.ms: 0` load. The release fires at most once, on its entry tick, and each leaves on its first evaluation after entry (pin P20).

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
- [ ] During release, the same damage does not break. A level `"*"` row that passes during release or recover leaves it, outranking the cycle's own rows. Damage taken in `flinch` does not carry into the activity it resumes.
- [ ] Enough damage during the limitator's aim enters `hit_reaction`, and its flinch resumes on its timer. Within one engagement of one limitator, consecutive cycles take both the strafe-and-fire and the reposition-and-aim branch, each matching the roll its deciding activity drew on that cycle's entry.

**Speed**
- [ ] On two archetypes whose `moveSpeed` differ and neither is 1, the same `speed: 2` activity steers each at twice its own `moveSpeed`, and at `moveSpeed` once left. An inner activity's `speed` overrides an outer one's; an inner activity without one inherits it.
- [ ] An activity's `speed` governs steering from the tick it is entered through the tick before it is left, and an activity entered and left on one tick never moves at its speed (pin P17).
- [ ] A charge release's `run` state plays at the rate its steered speed implies against its `travelSpeed`, on host and loopback client.
- [ ] `speed` scales only the agent's own steered travel. A knockback slide is the same under `speed: 2` as under none.
- [ ] An activity with `speed: 2` that plays the graph's locomotion clip presents the same playback rate on host and loopback client, with and without a calibrated clip stride.
- [ ] A charge release under the bellow stance chases at its speed and lands its contact attack on reach, once per entry: a target already in reach is hit on the release's entry tick, and a charge that does not reach within `release.ms` enters its recover with no hit and never fires there (pin P18).
- [ ] A brain serialized before these fields deserializes. Its sums read 0, and its next entry draws a roll.

**Co-op**
- [ ] On a loopback client, an activity's `sound` and a contact attack's `sound` play exactly once per host event, and the host plays each exactly once, with or without a server (pin P15).
- [ ] An enemy weapon attack's sound still plays exactly once (pin P10).
- [ ] An A → B → A sequence inside one snapshot interval plays A's sound twice on each peer (pin P7).
- [ ] A stance or contact sound plays once at the host-captured position whether or not the client holds the enemy, and never replays (pin P9).
- [ ] An enemy killed in the apply pass that enters its stance plays no stance sound on any peer (pin P11).
- [ ] An enemy that enters its stance and is killed after the AI pass on the same tick plays the stance sound once on every peer (pin P19).
- [ ] A client receiving the sound runs no `onEnter` reaction.
- [ ] A client that begins participating on the tick an enemy enters its stance plays that stance's sound at most once, and only from its command. Materializing an enemy mid-stance plays no sound (pin P14).
- [ ] Grep gate: no server message variant is added, and `PROTOCOL_ID`, `WIRE_VERSION` and `SNAPSHOT_VERSION` are unchanged.
- [ ] Entering an activity whose clip is already playing restarts nothing on host or client.

### Manual
- [ ] On `movement-feel`, the reference brute with its stances: a player reads the threat from the pose, cannot tell which of a shared stance's attacks is coming or predict the release across several cycles, and breaks a stance with heavy fire. The bellow reads as a charge coming.
- [ ] On `movement-feel`, the limitator sometimes strafes and fires, sometimes repositions, stops and aims; heavy fire during the aim flinches it.
- [ ] Co-op loopback: the client hears each stance sound and sees the stance before the hit lands.

### Docs
- [ ] `docs/` explains typed attacks, stances shared by attacks, the co-op floor's number, that a release can whiff on cooldown, range, sight or facing, and that a client sees a stance about half a round trip plus one snapshot late. `sdk/behaviors/reference` carries typed attacks and a shared stance that load.

## Path
- **Body stance.** A crouching stance needs the cover brief's `crouchHeight` and `crouch_` states on the archetype, which load enforces. Under innermost-wins resolution the builder emits `stance: "stand"` on the recover when the option is set, so an outer crouch never leaks into it.
- **Assets.** Today the brute's jab and slam both play `punch`; `reference-enemy.ts` nests the jab in a `jabs` subgraph under `hunt`/`press` and marks the slam's windup, commit and recover `attackTurn: "slam"`. The brute uses its authored hitbox: its model carries no hit zones (the converter zones only head and legs, which would leave the torso unhittable). Its library carries `smash` (a jump attack pinned in place), `swipe`, `punch`, `roar`, `hit_reaction`, and the holds `smash_windup` (400 ms, the crouch before the leap) and `swipe_windup` (900 ms, the lean back before the lunge). Holds are cut by `mixamo_to_gltf.py --trim`, and a non-looping clip clamps on its last frame (`resolve_time`, `crates/model/src/anim/types.rs`), so the pose holds with no engine change. `punch_windup` does not exist yet, and the Mixamo sources are not in the repo (see Open questions). The limitator's rifle library carries `idle_aiming`, and its `hit_reaction` is bound but unwired. Stance sounds have no assets yet; the dev fixture sounds stand in. An engine `untilMs`-style clip end is a follow-up, not this brief.
- **Seams.**
  - **Brain facts.** `ScopeRelativeValues` / `SCOPE_RELATIVE_SLOTS` (`crates/ai/src/brain_scope.rs`): one field and one registration per fact. `repoint_scope_relative` re-points them per depth in `select_transition_path` and in both selectors, each building the struct literally. `BrainScope::refresh` and the typedef templates spell `BRAIN_INPUTS` out positionally too.
  - **Brain arrays.** The per-depth arrays on `BrainComponent` zero on re-entry. `BRAIN_INPUTS` is append-only.
  - **Damage.** `apply_damage_with_context` already writes the brain for every producer, through `record_damage_perception` on a live target. Its `drain.points_dealt` is layer-absorbed plus health loss (`drain_layers`). A same-level row naming the current activity is skipped (`first_enabled_target`), so an activity re-enters only through a sibling or an ancestor.
  - **Seeding.** `pellet_rng_seed` / `SplitMix64` salt with a per-instance counter rather than `EntityId`. The initial activity is seated before spawn state is set (pin P13).
  - **Melee is silent on clients** because `publish_observer_weapon_cues` skips emissions without a `shot_id`.
  - **Sound route.** `SystemReactionCommand::PlaySound` maps to `PresentationCommand::PlaySound` (`crates/netcode/src/presentation_commands.rs`); `route_player_presentation` sends it per client through `send_presentation` and also queues it locally (P15). `resolve_recipients` sends a host-only sound with no audience to Everyone. The AI emission's `Emitter::Entity` origin is the captured position.
  - **The determinism gate's** single enemy spawns after both pawns, so today's reversal cannot catch an `EntityId` seed.
- **Shape.** Declarations (`defineStance`, `defineAttack`) that rows target directly, expanded by `defineEntity` and `defineMod` (which already walk the descriptor to lower handles). A builder the author calls (`stanceCycles()` plus `merge()`) was drafted and dropped: three moving parts for one idea, and a cycle-wide `after`. The rival, a composite activity with a nested graph, could hold its turn across its children, but nothing signals that the nested graph finished, and a `breakOn` inside it cannot reach the outer level's sibling while the release stays committed. A primitives-only rival (roll, damage fact, `speed`, sounds, typed attacks, and a docs recipe for hand-written cycles) leaves every cycle rebuilt by hand, the first obstacle in Problem. An engine-side stance state that picks the attack at release was weighed and declined: it makes stance an engine noun, and the turn must be held before the tell starts.
- **Typed attacks.** Binding-name sugar is `apply_binding_name_sugar` (`crates/script-compiler/src/lib.rs`); `defineStance` and `defineAttack` join it, keyed on an object-literal first argument like `defineAttackToken`. Attack-token references cross as `{ kind, name }` and reject bare strings (`crates/foundation/src/attack_budget/references.rs`); attack handles reject strings the same way but lower to the bare name, since every engine reference to an attack is already a name. Turn facts resolve per attack at load (`BrainInputRef::AttackTurnHeld` / `AttackTurnFree`, `crates/foundation/src/brain.rs`), so `attackTurnFree(slam)` still emits `"@attackTurnFree.slam"`. The TS and Luau types for `attacks`, `action.attack` and `attackTurn` are generated from a Rust registry in `crates/sim`, the turn-fact signatures live in `crates/scripting-core` typedef templates, and new Luau exports join `luau_prelude.rs`'s lists; drift tests guard all three. Today only `defineEntity` lowers references, so `defineMod` gains lowering for bare entities.
- **First slice.** The roll fact, its seed, and the extended determinism gate: the one-way door, falsified fastest.
- **Attack turns.** A turn is requested and granted in tick N's arbitration and reads held on N+1 (`arbitrate_attack_turns`, `TurnFactReader::read`). A drawn attack under a mark never fires without the turn (the hard gate in `arbitrate_attack_turns`). A turn's refresh starts when the turn ends, fired or not, so a broken hold spends its tokens' refresh.
- **Speed.** Steering reads `move_speed` on the agent, set once at spawn (`attach_agent`, `crates/sim/src/scripting/builtins/data_archetype.rs`; `agent_steering.rs`).

## Open questions
- Seed salt, a per-instance replay-stable identity — **delegated**: meet both seed requirements in Decisions and report the choice.
- The seeded hold and recovery defaults, and the floor's reaction budget — **delegated**: tune on `movement-feel` under the lag profile and report.
- The brute's new lunge and charge attacks (damage, draws), which stance two of its attacks share, the charge speed, its walk and run `speed`s, and how its offense graph nests the stances — **delegated**: tune on `movement-feel` and report.
- `punch_windup` — **blocks build** of the `cock` stance only: the owner re-exports `mixamo_brute` from the Mixamo sources with a `--trim punch_windup=punch:<ms>` cut at the cocked fist. The other stances build without it.
- How a misspelled Luau handle in a table field reaches agents (strict globals in data scripts, `luau-analyze` in the scripts check, or a language server) — **delegated**: pick the one an agent's normal loop runs, and report.
- Clip-timed attacks — **blocks build** of the slam, lunge and jab releases: that brief's clip ranges and event-timed strike land first. The owner may fold the strike cuts into the same `mixamo_brute` re-export as `punch_windup`.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| attack declaration | binding-name sugar list (`crates/script-compiler`); Luau export list (`luau_prelude.rs`); serializes to `BehaviorGraphDescriptor::attacks` by name | `"attacks": { "<name>": { ... } }` unchanged | `defineAttack(params): Attack`, binding-named; `defineAttack(name, params)` | `defineAttack(name, params)` | n/a |
| attack fields (SDK only) | none | stripped before the descriptor | `stance: Stance`, `release { animation, ms, motion, speed }` (motion: `hold` \| `chaseTarget` \| `strafe` \| `reposition`), `recover { animation, ms }`, `after: string \| Stance` | same | n/a |
| row targets | none (expanded to activity names) | the generated activity's name | `to: string \| Attack \| Stance` | same | n/a |
| attack references | SDK type registry (`crates/sim`) and typedef templates (`crates/scripting-core`) take handle types; runtime unchanged | the attack's name | `attacks: Attack[]`; `action: { attack: Attack }`; `attackTurn: Attack`; `attackTurnFree(a)`, `attackTurnHeld(a)` take a handle | same | n/a |
| stance declaration | binding-name sugar list; Luau export list | — | `defineStance(options): Stance`, binding-named; `defineStance(name, options)` | `defineStance(name, options)` | n/a |
| stance options | none | — | `animation`, `sound`, `stance`, `holdMs`, `breakOn { damage, to }` | same | n/a |
| generated names | none | activity keys `<stance>` (rest), `<attack>_<stance>`, `<attack>_release`, `<attack>_recover`; per-start copies are the executor's to name | same | same | n/a |
| activity speed | `BehaviorActivityDescriptor::speed: Option<f32>` | `"speed"` | `speed?: number` | `speed` | n/a |
| roll fact | `BRAIN_INPUTS` tail | `"@brain.activityRoll"` | `brain.activityRoll` | `brain.activityRoll` | n/a |
| damage fact | `BRAIN_INPUTS` tail, after the roll | `"@brain.damageInActivity"` | `brain.damageInActivity` | `brain.damageInActivity` | n/a |

These facts append after `hasAttackTurn` (slot 20).

## Wire format
**No new message.** Telegraph and melee sounds ride the existing `PresentationCommand::PlaySound { sound, bus, at }` on the Presentation channel, `at` set to the host-captured origin, to every participating client. Weapon attack sounds stay on `WeaponCue`. No `PROTOCOL_ID`, `WIRE_VERSION` or `SNAPSHOT_VERSION` change.
