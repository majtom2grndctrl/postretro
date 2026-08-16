# Enemy Multi-Attack

## Goal

A behavior graph carries a **named map of attacks** — e.g. a melee swipe plus a ranged zap (Quake Ogre), or two distinct melee attacks (Fiend) — each with its own tuning, referenced by name from the `attack` action verb on whichever states fire it. A `weapon`-referencing entry resolves against a canonical weapon descriptor (the roadmap's "attacks are weapons/wieldables" model), so player and enemy attacks share one authoring substrate; a `weapon`-less entry is contact/melee tuned inline. The map is the API from day one — no privileged "primary" entry. Attacks stay **instantaneous**: damage is cooldown-gated at fire time, with no windup, commit, or recovery phase. That instantaneity is what keeps selection on the flat graph sufficient — band-based routing between attacks is ordered distance transitions, the same first-true-wins mechanism every other state choice already uses. This is action-vocabulary growth on the shipped behavior state graph, not a new selection engine.

## Scope

### In scope

- `components.behavior.attacks`: a named map (`BTreeMap<String, AttackParams>`) of attack entries. Each entry: optional `weapon` (canonical weapon-descriptor name), a `minRange`/`maxRange` distance band, an optional per-attack `engagementRadius` (standoff for the state that fires it), and — for a `weapon`-less contact entry — `damage` and `cooldownMs` inline. A `weapon` entry defaults `maxRange`, `cooldownMs`, and damage from the referenced weapon.
- The `attack` action verb takes a required `attack` parameter naming a map entry: `action: { attack: "claw" }`. A state fires exactly the named attack; nothing else about how action verbs run inside the tick changes.
- `ResolutionMode::Contact` — the melee resolution variant: cooldown-gated direct damage inside `maxRange`, no ray. Applies to any `weapon`-less entry, preserving today's enemy contact-damage behavior.
- Enemy hitscan firing for `weapon`-referencing entries, through the weapon module's ray + nearest-of resolution path, with shooter exclusion. A hitscan enemy attack is blocked by world geometry — resolution-level occlusion, not a perception model.
- Per-attack engagement radius: the state firing an attack takes its standoff from that attack's entry; every other state (and an attackless graph) takes the graph-level `engagementRadius` default.
- Per-attack cooldown state on `BrainComponent`, keyed by attack name, so each attack's cooldown counts independently.
- `@brain.attackCooldownMs` reports the current state's attack's remaining cooldown (0 when the current state fires no attack).
- Band-based attack selection as graph content: an archetype authors one attack-firing state per attack (or per attack cluster) with ordered `@brain.targetDistance` transitions routing into the state whose action names the attack that should fire. The graph's first-true-wins evaluation is the whole selection mechanism.
- Reference enemy archetype gains a second (hitscan) attack; the agent-diagnostics overlay state label names the firing state's attack.

### Out of scope, and who owns each

- **Windup / telegraph, commit (don't-bail-mid-attack), recovery.** These need a nested/scoped state layer so a brain can enter a phase it cannot be routed out of mid-swing. Owned by **Hierarchical behavior (statecharts)** (`roadmap.md`, Epic 10 — "a nested graph when a layer needs its own state (attack windup→commit→recover)").
- **Forced N>1 attack rotation** (fire attack A, then B, regardless of distance). A rotation counter is per-activity state the flat graph has no place to hold; it belongs to the same statechart layer and to **Epic 16 combat stances** (`roadmap.md`, Epic 10 statecharts entry — "the behavior substrate Epic 16's combat stances and planner build on").
- **Melee standoff with lunge** (park outside reach, then close under a committed impulse). The lunge is a combat↔movement impulse owned by **Epic 16 › Resolution Modes › melee** (`roadmap.md` — "melee and quick-melee with a lunge"); the commit that makes parking-then-closing safe is the statechart layer above.
- Projectile attacks (a later `ResolutionMode` variant; this spec only keeps the enum extensible).
- Runtime wieldable *entities* per attack (companion-entity equipping); stats resolve at spawn into brain-side tuning.
- Perception/LOS for target *selection* — the `visible` predicate stays `None`; occlusion here gates only hitscan attack *resolution*.
- Damage synced to animation frames (damage stays cooldown-gated at fire time).
- Player weapon changes: `FireMode` is ignored on the enemy-driven fire path (the graph decides when to fire).
- Leash/pursuit policy, squad coordination, faction (other slices of the behavior-descriptors thread).
- Stagger/pain interrupts (`E10--enemy-stagger`).

## Direction

**Problem.** The behavior graph can name only one attack (`components.behavior.attack`, a single `AttackParams` block), so an archetype cannot mix a melee swipe with a ranged shot the way the target enemies (Ogre, Fiend) demand. The graph already routes between states by distance; what it lacks is a way for different states to fire different attacks.

**Prior commitments.** The shipped graph makes `components.mesh.animations` a graph-wide named vocabulary that states reference by name; this spec models `attacks` the same way, so the two read alike. The statecharts successor (`roadmap.md`, Epic 10) commits to migrating graph-wide blocks (`patrol` → per-activity `route`, `moveSpeed` → per-activity `speed`) into per-activity groupings additively — a named `attacks` map is the endpoint that migration wants, not a shape it has to unwind: statecharts later group the same named entries per activity. Keeping attacks instantaneous is the load-bearing commitment: the roadmap places windup→commit→recover in the statechart layer, so an instantaneous attack that never needs commit is exactly the slice the flat graph can own. Combat-slot standoff (`E10--enemy-combat-positioning`, shipped) already resolves a per-agent `engagement_radius`; per-attack standoff feeds that seam rather than adding one.

**Alternatives rejected.** A privileged `primary` attack plus an optional `secondary` would carry a hidden default nobody authored and would not generalize past two attacks — the map costs nothing more and generalizes to N. A per-tick "eligible attacks" selection pass (engine ranks the fireable attacks and picks one) is the mechanism statecharts and combat stances exist to provide; adding it here would build the selection engine this spec's whole framing says is unnecessary while attacks are instantaneous. A graph-level engagement radius that the active attack cannot override (today's single resolver) produces the "enemy walks to the ring and never swings" failure once a graph mixes a long-reach shot with a short-reach swipe, because one standoff cannot suit both.

## Acceptance criteria

- [ ] A `components.behavior.attacks` map with two entries (a `weapon`-less contact entry and a `weapon` hitscan entry) parses and validates identically in QuickJS and Luau. Rejections carry pathed errors in both: an empty `attacks` map when any state's action references it; an entry with `minRange > maxRange`; a contact entry (no `weapon`) missing `damage` or `cooldownMs`, or with `maxRange` absent; a contact entry whose authored `engagementRadius > maxRange`; an `action.attack` naming no entry. An unresolvable `weapon` name, or a `weapon` entry whose engagement radius exceeds the resolved weapon range, fails at spawn with the entity's descriptor name in the error.
- [ ] The reference enemy, migrated to a single-entry `attacks` map with `action: { attack: "..." }`, holds its transition and damage cadence exactly: same distance at which its swing connects, same damage per swing, same cooldown interval, same attack-clip replay. The AI test suite passes once its fixtures are migrated to the map shape — semantic preservation, not a byte-identical descriptor.
- [ ] On a flat fixture with the two-attack reference enemy: at a distance inside only the hitscan band the player takes that attack's damage once per that attack's cooldown with the hosting state's animation active; inside the melee band the melee attack's damage and animation apply instead — routing driven entirely by the authored per-state distance guards.
- [ ] Per-attack cooldowns are independent: firing one attack neither resets nor delays another's cooldown, and switching between two attack-firing states mid-cooldown leaves each attack's remaining cooldown untouched.
- [ ] The state firing an attack takes its combat-slot standoff from that attack's engagement radius; a non-attack state (and an attackless graph) takes the graph-level `engagementRadius` default. An enemy whose active attack has a longer reach stands off at that reach rather than crowding to the melee ring.
- [ ] `@brain.attackCooldownMs` reads the current state's attack's remaining cooldown, and reads 0 while the current state fires no attack.
- [ ] An in-band enemy with a clear line to its target lands hitscan damage; the same enemy behind world geometry deals none. The firer can never hit itself.
- [ ] Selection is deterministic: when the graph's transitions could route to more than one attack state, declaration order wins (the first-true-wins evaluator guarantee); sim determinism tests stay green.
- [ ] With no attack's band currently satisfied but an attack-firing state still current, the enemy holds in that state facing the target (no authored transition guard is true yet).
- [ ] SDK typedef drift tests pass with `attacks`, the per-entry fields, and `action: { attack }` present in both `postretro.d.ts` and `postretro.d.luau` committed fixtures.
- [ ] The agent-diagnostics overlay state label shows the firing state's attack name for an enemy in an attack-firing state.
- [ ] On a connected client, a host enemy's attack switch shows as a change of replicated animation state name with no wire-format change.

## Tasks

### Task 1: Attack vocabulary and resolution descriptors

In `postretro-foundation`, `crates/foundation/src/data_descriptors/types/behavior.rs`: replace `BehaviorGraphDescriptor::attack: Option<AttackParams>` with `attacks: BTreeMap<String, AttackParams>`. Extend `AttackParams` with `weapon: Option<String>`, `min_range: f32` (serde-default 0), `max_range: Option<f32>`, and a per-attack `engagement_radius: Option<f32>`; make `damage: Option<f32>` and `cooldown_ms: Option<f32>` (a `weapon` entry inherits both from the weapon). Drop `#[derive(Copy)]` from `AttackParams` — `weapon: Option<String>` makes it non-`Copy`. Parameterize `ActionVerb::Attack` as `Attack { attack: String }`; the wire shape becomes the object `action: { attack: "<name>" }`, so the `ActionVerb` serde derive and the `action_verb_all_is_exhaustive` walk both carry the field, and the round-trip test's `"action": "attack"` string becomes `"action": { "attack": "..." }`. In `crates/foundation/src/data_descriptors/types/combat.rs`, add `ResolutionMode::Contact` beside `Hitscan` (camelCase serde, matching the enum). Rework the engagement-radius resolver: `BehaviorGraphDescriptor::engagement_radius()` loses its `self.attack.map(|a| a.range)` rung (there is no singular block, and non-`Copy` `AttackParams` cannot be mapped by value) and resolves the graph-level default only (`self.engagement_radius`, else `DEFAULT_ENGAGEMENT_RADIUS`); add `engagement_radius_for_state(&self, state_index: usize) -> f32`, which returns the active state's attack entry's engagement radius (authored, else its `maxRange`) when the state fires an attack, and the graph-level default otherwise. Add validation in `BehaviorGraphDescriptor::validate` (all pathed, wire-cased — `components.behavior.attacks.claw.maxRange`): `attacks` is non-empty when any state declares the attack action; every `action.attack` resolves to a map entry; each entry has `minRange <= maxRange` (both finite, `maxRange > 0`); a `weapon`-less entry requires `maxRange`, finite `damage >= 0`, and `cooldownMs > 0`; a `weapon`-less entry's authored `engagementRadius`, when present, is `<= maxRange` (the reach constraint — see Invariants). Both runtimes inherit parsing through the shared `behavior` serde funnel (`crates/scripting-core/src/data_descriptors/js/entity.rs`, `lua/entity.rs` — verify neither needs a per-runtime shim). Regenerate the SDK typedefs (`sdk/types/postretro.d.ts`, `.d.luau`, and the `sdk/lib` builders) and update the committed fixtures under `crates/postretro/src/scripting/typedef/tests/fixtures/`.

### Task 2: Spawn resolution and per-attack brain state

At archetype spawn, resolve each `attacks` entry's optional `weapon` reference and materialize a name-indexed per-attack tuning table alongside `BrainComponent` — for each attack: damage, `minRange`/`maxRange` band, engagement radius, cooldown, resolution mode. A `weapon` entry pulls damage, `maxRange`, and cooldown from the resolved weapon descriptor (an authored `maxRange`/`cooldownMs` on the entry overrides). An unresolvable `weapon` name fails spawn validation with the entity's descriptor name in the error; so does a `weapon` entry whose resolved engagement radius exceeds the weapon's range (the spawn-time twin of Task 1's parse-time contact reach check, since weapon range is unknown until spawn). Replace `BrainComponent::attack_cooldown_remaining_ms` (the single scalar) with a name-indexed cooldown map (`BTreeMap<String, f32>`, `#[serde(default)]`) — the cooldown is transient sim state re-armed on first fire, so a restored brain lacking the field loads empty (every attack ready), consistent with the existing `target_reachable`/anchor defaults. Update the per-tick cooldown countdown (currently the single-scalar decrement) to count each entry down. Enumerate and update every reader of the old single-attack tuning: the `attack`-verb cooldown check and damage/range reads in `crates/postretro/src/scripting/systems/ai/mod.rs`, and the `authored_graph()` fixture in `crates/entities/src/components/brain.rs`, keyed by the firing state's named attack.

### Task 3: Attack action verb firing and brain-fact feed

Extend the attack seam in `crates/postretro/src/scripting/systems/ai/mod.rs` — the cooldown/`action_for_state` gate in the compute pass and the apply-pass `apply_damage_with_context` + `ENEMY_ATTACK_EVENT`/`ENEMY_ATTACK_SOURCE_ID` + `restart_animation_clip`. Resolve the current state's named attack against Task 2's per-attack tuning table, gate on that attack's own cooldown-map entry and its `maxRange`, and fire: a `Contact` entry keeps the `apply_damage_with_context` contact-damage path (unchanged `DamageContext`); a `weapon` entry synthesizes an origin (enemy eye) and a direction (toward the target's hitbox center) and resolves through the weapon ray path. Extract the world-ray + `nearest_entity_hit` + nearest-of resolution (`resolve_nearest_hit` in `crates/postretro/src/weapon/mod.rs`) into a seam callable without a `WeaponComponent`, and add an ignore-shooter parameter to `nearest_entity_hit` so the ray excludes the firing enemy — enumerate and update its callers (`weapon/mod.rs` player fire path, `impact_policy.rs`, and the `hit_zones.rs` tests). Route the weapon hit through the same zone-multiplier scaling and `apply_damage_with_context` the player path uses. A fired attack re-arms only its own cooldown-map entry (Invariant: independent cooldowns). Feed `@brain.attackCooldownMs` from the current state's attack's remaining cooldown at the scope refresh (`BrainFacts.attack_cooldown_ms`), reading 0 when the current state fires no attack — the fact's single-number shape is unchanged; only which attack's timer it reports generalizes across states. In `crates/postretro/src/scripting/systems/ai/combat_slots.rs`, resolve each engaged agent's standoff via `engagement_radius_for_state(outcome.brain.state_index)` instead of the graph-wide `engagement_radius()` (D2).

### Task 4: Reference archetype, overlay, fixture verification

In `sdk/behaviors/reference/entities.{ts,luau}`, migrate the reference enemy and the pose-fixture enemy from the singular `attack` block + `action: "attack"` to a single-entry `attacks` map + `action: { attack: "<name>" }`, holding each one's cadence (Task 2/3 preserve the contact path; this is a shape change, not a tuning change). Give the reference enemy a second attack: a hitscan zap weapon descriptor, a second `attacks` entry referencing it, an attack-firing zap state with distance-guard routing to and from the melee state, and the zap clip/state added to its `mesh.animations`. Migrate every AI test fixture to the new shape (`ai/mod.rs` tests, `brain_scope.rs`, `crates/entities/src/components/brain.rs` `authored_graph()`, and the `behavior.rs` descriptor tests). Extend the agent-diagnostics overlay state label with the firing state's attack name. On the movement-feel fixture, verify the two-band behavior (melee band vs. hitscan band) and the occlusion AC, and confirm co-op clients show the distinct attack animation states via the replicated state name (no wire change expected).

## Sequencing

**Builds on shipped code:** the behavior state graph (`components.behavior`, the `attack` action verb, the split `crates/postretro/src/scripting/systems/ai/` layout), combat positioning (`CombatQuery::engagement_radius` in `combat_slots.rs`), and facing slew (`FACING_TURN_RATE`) — all in `context/plans/done/`.

**Phase 1 (sequential):** Task 1 — the descriptor shape and validation everything else consumes; it also falsifies the wire-shape and resolver assumptions the later tasks rest on.
**Phase 2 (sequential):** Task 2 — consumes Task 1's descriptor shape; materializes the per-attack tuning table and cooldown map.
**Phase 3 (sequential):** Task 3 — consumes Task 2's tuning table and cooldown map; wires firing, the brain fact, and the standoff resolver.
**Phase 4 (sequential):** Task 4 — exercises Task 3 end to end and migrates the reference content and fixtures.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| attacks map | `BehaviorGraphDescriptor::attacks: BTreeMap<String, AttackParams>` | `"attacks"` | `attacks: Record<string, AttackParams>` | `attacks: {[string]: AttackParams}` | n/a |
| weapon reference | `AttackParams::weapon: Option<String>` | `"weapon"` | `weapon?` | `weapon?` | n/a |
| band floor | `AttackParams::min_range: f32` (serde-default 0) | `"minRange"` | `minRange?` | `minRange?` | n/a |
| band ceiling / reach | `AttackParams::max_range: Option<f32>` | `"maxRange"` | `maxRange?` | `maxRange?` | n/a |
| contact damage | `AttackParams::damage: Option<f32>` | `"damage"` | `damage?` | `damage?` | n/a |
| cooldown | `AttackParams::cooldown_ms: Option<f32>` | `"cooldownMs"` | `cooldownMs?` | `cooldownMs?` | n/a |
| per-attack standoff | `AttackParams::engagement_radius: Option<f32>` | `"engagementRadius"` | `engagementRadius?` | `engagementRadius?` | n/a |
| graph default standoff | `BehaviorGraphDescriptor::engagement_radius: Option<f32>` | `"engagementRadius"` | `engagementRadius?` | `engagementRadius?` | n/a |
| action parameter | `ActionVerb::Attack { attack: String }` | `action: { "attack": "<name>" }` | `action: { attack: string }` | `action: { attack: string }` | n/a |
| contact resolution | `ResolutionMode::Contact` | `"contact"` | `"contact"` | `"contact"` | n/a |

Two distinct `engagementRadius` keys exist: one on each `attacks` entry (the firing state's standoff) and one on the graph (the default for non-attack states). They never collide — one is nested under `attacks.<name>`, the other is top-level under `behavior`.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Each attack's cooldown counts independently; firing or switching attacks touches only the fired attack's entry | Task 2 (name-indexed cooldown map), Task 3 (re-arm only the fired entry) | A shared or mis-keyed decrement would couple two attacks' timers | AC 4 |
| A contact attack's engagement radius never exceeds its reach (`engagementRadius <= maxRange`) | Task 1 (parse-time check for `weapon`-less entries) | A contact attack parked beyond `maxRange` can never land — closing the gap needs commit, which the flat graph cannot express; this is the statecharts boundary | AC 1, AC 5 |
| A `weapon` attack's engagement radius never exceeds the weapon's range | Task 2 (spawn-time check, once the weapon resolves) | Weapon range is unknown at parse; the check must run at spawn | AC 1 |
| An enemy's hitscan ray never resolves against its own hitbox | Task 3 (ignore-shooter parameter on `nearest_entity_hit`) | Every `nearest_entity_hit` caller must pass the exclusion consistently | AC 7 |
| Attack selection is declaration-order deterministic | shipped first-true-wins evaluator; Task 4 authors ordered guards | Adding attack-firing states must not introduce a non-deterministic tiebreak | AC 8 |

## Script syntax examples

```ts
// Proposed design
import { defineEntity, brain, runtime } from "postretro";

export const grunt = defineEntity({
  canonicalName: "grunt",
  components: {
    health: { max: 60 },
    mesh: { /* model, animation states incl. attack_claw / attack_zap */ },
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      // Graph-wide attack vocabulary; states reference entries by name, the way
      // they already reference `mesh.animations` keys.
      attacks: {
        // Contact entry: inline damage/cooldown, standoff defaults to maxRange.
        claw: { damage: 8, minRange: 0, maxRange: 1.8, cooldownMs: 1200 },
        // Weapon entry: damage/cooldown/reach inherit from `grunt_zap`; it stands
        // off at 12 m and fires a ray, bounded by the weapon's range.
        zap:  { weapon: "grunt_zap", maxRange: 14, engagementRadius: 12, cooldownMs: 1600 },
      },
      // Default standoff for non-attack states (e.g. `chase`).
      engagementRadius: 12,
      states: {
        idle: {
          animation: "idle", motion: "hold",
          transitions: [{ to: "chase", when: runtime.le(brain.targetDistance, 16) }],
        },
        chase: {
          animation: "walk", motion: "chaseTarget",
          transitions: [
            { to: "attack_claw", when: runtime.le(brain.targetDistance, 1.8) },
            { to: "attack_zap", when: runtime.le(brain.targetDistance, 14) },
          ],
        },
        attack_claw: {
          animation: "attack_claw", motion: "chaseTarget", action: { attack: "claw" },
          transitions: [{ to: "chase", when: runtime.gt(brain.targetDistance, 1.8) }],
        },
        attack_zap: {
          animation: "attack_zap", motion: "chaseTarget", action: { attack: "zap" },
          transitions: [
            { to: "attack_claw", when: runtime.le(brain.targetDistance, 1.8) },
            { to: "chase", when: runtime.gt(brain.targetDistance, 14) },
          ],
        },
      },
    },
  },
});

export const gruntZap = defineEntity({
  canonicalName: "grunt_zap",
  components: { weapon: { damage: 7, range: 18, fireRateMs: 1600, fireMode: "semi", resolution: "hitscan" } },
});
```

## Open questions

- **Repeat-attack replication.** Re-firing the same attack restarts the clip via `restart_animation_clip`, which changes no state name and so produces no wire delta — remote clients see the first swing clamp. Pre-existing gap (single-attack enemies have it today); distinct attacks mask it, since routing between them does change the replicated state name. A wire restart signal is future netcode work. **Owner:** netcode.
- **`minRange > 0` band-floor thrash.** A far-band-only enemy chasing the target's center point can drive itself inside its own band floor and oscillate between attack states. Per-attack engagement radius (D2) mitigates this — the enemy stands off at the firing attack's radius rather than closing to center — but a graph that authors a `minRange` shorter than the neighbouring attack's standoff can still chatter. Playtest whether authored band floors need a hysteresis guard; do not add one ahead of an observed thrash. **Owner:** this spec's playtest, escalating to combat-positioning if it recurs.
