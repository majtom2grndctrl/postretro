# Enemy Multi-Attack

## Goal

A character's behavior graph carries a **named map of attacks** — e.g. a melee swipe plus a ranged zap (Quake Ogre), or two distinct melee attacks (Fiend) — each with its own tuning, referenced by name from the `attack` action verb on whichever states use them. Attacks are references to weapon descriptors (the roadmap's "attacks are weapons/wieldables" model), so player and enemy attacks share one authoring substrate. The API is a map from day one: no privileged "primary" entry. Resolution stays hitscan/contact in this spec; a projectile attack later is a new `ResolutionMode` variant dropping into the same shape. This is action-vocabulary growth on `E10--behavior-state-graph`'s graph, not a new selection mechanism: band-based routing between attacks is authored transitions, same as any other state choice.

## Scope

### In scope

- `components.behavior.attacks`: a named map of attack entries, superseding the base spec's singular `attack` tuning block. Each entry: optional `weapon` (canonical weapon-descriptor name), `minRange`/`maxRange` band, and — for entries with no `weapon` (contact/melee) — `damage` and `cooldownMs` directly; entries with a `weapon` default range/cooldown from the referenced weapon.
- The `attack` action verb gains a required `attack` parameter naming an entry in the map: `action: { attack: "claw" }`. A state's action fires exactly the named attack; nothing else changes about how action verbs run inside the tick.
- `ResolutionMode::Contact` — weapon-level variant for melee: cooldown-gated direct damage at range, no ray. Preserves today's enemy contact-damage behavior exactly. Applies to any attack entry without a `weapon` reference.
- Per-attack cooldown state on `BrainComponent`, keyed by attack name (replaces the base spec's single cooldown scalar once an archetype's graph uses `attacks`).
- Enemy hitscan firing (for `weapon`-referencing entries) through the weapon module's existing ray + nearest-of resolution path, with shooter exclusion. A hitscan enemy attack is blocked by world geometry — resolution-level occlusion, not a perception model.
- Band-based attack selection is graph content, not engine logic: an archetype authors one state per attack (or per attack cluster) with transitions gated on `@brain.targetDistance`, routing into whichever state's action names the attack that should fire. The graph's existing first-true-wins evaluation is the whole selection mechanism — no separate "eligible attacks" computation.
- Reference enemy archetype gains a second (hitscan) attack state; agent diagnostics overlay labels the selected attack.

### Out of scope

- Projectile attacks (new `ResolutionMode` variant later; this spec only keeps the enum extensible).
- Runtime wieldable *entities* per attack (companion-entity equipping); stats resolve at spawn into brain-side tuning.
- Perception/LOS for target *selection* — the `visible` predicate stays `None`; occlusion here gates only hitscan attack *resolution*.
- Attack windups, telegraphs, damage synced to animation frames (damage stays cooldown-gated at fire time).
- Player weapon changes: `FireMode` is ignored on the enemy-driven fire path (the graph decides when to fire).
- Leash/pursuit policy, squad coordination, hostility/faction (other slices of the roadmap's behavior-descriptors bullet).
- Stagger/pain interrupts (`E10--enemy-stagger`).
- Splitting `scripting/systems/ai.rs` — owned by `E10--behavior-state-graph` Task 1. This spec lands after that split.

## Acceptance criteria

- [ ] A `components.behavior.attacks` map with two entries (contact melee + hitscan) parses and validates identically in QuickJS and Luau; rejections carry pathed errors in both: empty map when any state's action references it, `minRange > maxRange`, a contact entry (no `weapon`) missing `damage` or `cooldownMs`, an `action.attack` naming no entry in the map. An unresolvable weapon name fails at spawn with the entity's descriptor name in the error.
- [ ] Archetypes using the base spec's singular `attack` block behave bit-for-bit as today: the full existing AI test suite passes unchanged, and the legacy reference enemy's transition/damage cadence is unchanged.
- [ ] On a flat fixture with the two-attack reference enemy: at a distance inside only the hitscan band, the player takes that attack's damage once per that attack's cooldown with the hosting state's animation active; inside the melee band, the melee attack's damage and animation apply instead — driven entirely by the authored per-state distance guards.
- [ ] Per-attack cooldowns are independent: firing one attack does not reset or delay the other's cooldown.
- [ ] An in-band enemy with a clear line to its target lands hitscan damage; the same enemy behind world geometry deals none. The firer can never hit itself.
- [ ] Selection is deterministic: when the graph's transitions could route to more than one attack state, declaration order wins (the standing first-true-wins evaluator guarantee); sim determinism tests stay green.
- [ ] With no attack's band currently satisfied but a state whose action fires an attack still current, the enemy holds in that state facing the target (today's between-cooldowns behavior, now just "no authored transition guard is true yet").
- [ ] SDK typedef drift tests pass with `attacks` present in both `postretro.d.ts` and `postretro.d.luau` committed fixtures.
- [ ] The agent diagnostics overlay state label shows the selected attack's name for agents in an attack-firing state.
- [ ] On a connected client, a host enemy's attack switch shows as a change of replicated animation state name with no wire-format change.

## Tasks

### Task 1: Attack tuning map

In `postretro-foundation` (`data_descriptors/types/`), change `BehaviorGraphDescriptor`'s attack tuning from the base spec's singular `attack: Option<AttackTuning>` to `attacks: BTreeMap<String, AttackTuning>`; extend `AttackTuning` with `weapon: Option<String>` and `minRange`/`maxRange` (default `minRange` 0, `maxRange` defaults from the referenced weapon's `range` when `weapon` is present). Add `ResolutionMode::Contact` beside `Hitscan` in the weapon module (camelCase wire values, matching the enum's existing serde). Parameterize `ActionVerb::Attack` with a required `attack: String` naming a map key; wire shape `action: { attack: "<name>" }`. Validation in `BehaviorGraphDescriptor`'s structural checks: every `action.attack` resolves to a map entry; `minRange <= maxRange`; entries without `weapon` require finite `damage` (≥ 0) and `cooldownMs` (> 0); wire-cased paths (`components.behavior.attacks.claw.maxRange`). Both script runtimes inherit parsing through the shared `behavior` funnel from `E10--behavior-state-graph` Task 2 (verify `js/entity.rs`, `lua/entity.rs` need no per-runtime shim). Regenerate SDK typedefs and update the committed fixture files that pin them.

### Task 2: Spawn resolution into brain tuning

At archetype spawn, resolve each `attacks` entry's optional weapon reference and materialize a name-indexed per-attack tuning table alongside `BrainComponent` (damage, range band, cooldown, resolution mode) — the same resolve-at-spawn shape the base spec's Task 4 uses for the singular block, now keyed by name; unresolvable weapon names fail spawn validation with the entity's descriptor name in the error. `BrainComponent` gains per-attack cooldown state (a name-indexed map, serde-default) replacing the base spec's single cooldown scalar; keep that scalar field serde-tolerated for existing saves but unused once an archetype's graph carries `attacks`. Enumerate and update every reader of the base spec's single-attack tuning (the `attack` action verb's cooldown check, tests) to key by the firing state's named attack instead.

### Task 3: Attack action verb firing

Extend the `attack` action verb's execution inside the tick (the seam `E10--behavior-state-graph` Task 5 establishes) to resolve its `attack` parameter against the per-attack tuning table, check that attack's own cooldown, and fire: `Contact` entries keep today's `apply_damage` + `enemyAttack` event + in-state clip restart; `weapon`-referencing entries synthesize origin (enemy eye) and direction (toward the target's hitbox center) and resolve through the weapon module's ray path — extract the world-ray + `nearest_entity_hit` + nearest-of resolution into a seam callable without a `WeaponComponent`, add an ignore-shooter parameter to `nearest_entity_hit` (enumerate its callers: the player fire path passes its firing pawn, tests updated), route the hit through the same zone-multiplier scaling and `apply_damage` the player path uses. A fired attack re-arms only its own cooldown entry. Band-based routing between attack-firing states needs no engine code beyond this — it's authored transitions on `@brain.targetDistance`, already covered by the base spec's evaluator.

### Task 4: Reference archetype, overlay, fixture verification

Give the reference enemy archetype a second attack entry: a hitscan zap weapon descriptor plus an `attacks` map with the existing melee entry. Author a second attack-firing state (zap) with a distance-guard transition from the melee state's band, and back. Add the zap clip/state mapping to the reference enemy's mesh animations. Extend the agent diagnostics overlay state label with the firing state's named attack. Verify the two-band behavior and occlusion ACs on the movement-feel fixture map, and confirm multiplayer clients show distinct attack animation states via the replicated state name (no wire change expected).

## Sequencing

**Depends on:** `E10--behavior-state-graph` — needs `components.behavior`, the `attack` action verb, and the split `ai.rs` layout in place first.

**Phase 1 (sequential):** Task 1 — the tuning map shape everything else consumes.
**Phase 2 (sequential):** Task 2 — consumes Task 1's descriptor shape.
**Phase 3 (sequential):** Task 3 — consumes Task 2's tuning table.
**Phase 4 (sequential):** Task 4 — exercises Task 3 end to end.

Cross-spec: run after `E10--enemy-combat-positioning` and `E10--enemy-facing-slew` land (both edit the same AI tick region; positioning also owns the destination the attack bands now inform).

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| attacks map | `BehaviorGraphDescriptor::attacks: BTreeMap<String, AttackTuning>` | `"attacks"` | `attacks: Record<string, AttackTuning>` | `attacks: {[string]: AttackTuning}` | n/a |
| weapon reference | `AttackTuning::weapon: Option<String>` | `"weapon"` | `weapon?` | `weapon?` | n/a |
| band floor | `AttackTuning::min_range` | `"minRange"` | `minRange?` | `minRange?` | n/a |
| band ceiling | `AttackTuning::max_range` | `"maxRange"` | `maxRange?` | `maxRange?` | n/a |
| contact damage | `AttackTuning::damage` | `"damage"` | `damage?` | `damage?` | n/a |
| cooldown | `AttackTuning::cooldown_ms` | `"cooldownMs"` | `cooldownMs?` | `cooldownMs?` | n/a |
| action parameter | `ActionVerb::Attack { attack: String }` | `action: { "attack": "<name>" }` | `action: { attack: string }` | same | n/a |
| melee resolution | `ResolutionMode::Contact` | `"contact"` | `"contact"` | `"contact"` | n/a |

## Script syntax examples

```ts
// Proposed design
import { defineEntity, runtime } from "postretro";

const dist = runtime.read("@brain.targetDistance");

export const grunt = defineEntity({
  canonicalName: "grunt",
  components: {
    health: { max: 60 },
    mesh: { /* model, animation states */ },
    behavior: {
      initial: "idle",
      attacks: {
        claw: { damage: 8, minRange: 0, maxRange: 1.8, cooldownMs: 1200 },
        zap:  { weapon: "grunt_zap", maxRange: 14, cooldownMs: 1600 },
      },
      states: {
        idle: {
          animation: "idle", motion: "hold",
          transitions: [{ to: "chase", when: runtime.le(dist, 16) }],
        },
        chase: {
          animation: "walk", motion: "chaseTarget",
          transitions: [
            { to: "attack_claw", when: runtime.le(dist, 1.8) },
            { to: "attack_zap", when: runtime.le(dist, 14) },
          ],
        },
        attack_claw: {
          animation: "attack_claw", motion: "chaseTarget", action: { attack: "claw" },
          transitions: [{ to: "chase", when: runtime.gt(dist, 1.8) }],
        },
        attack_zap: {
          animation: "attack_zap", motion: "chaseTarget", action: { attack: "zap" },
          transitions: [
            { to: "attack_claw", when: runtime.le(dist, 1.8) },
            { to: "chase", when: runtime.gt(dist, 14) },
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

- **Combat-positioning handoff.** Positioning's `engagement_radius` is a single scalar. Rule here: it reads the current attack-firing state's `maxRange`, falling back to the largest band when the enemy is between attack states; a state switch does not force a combat-slot re-score in v1. Revisit if playtests show band-thrash.
- **`minRange > 0` before positioning lands.** A far-band-only enemy chasing the target's center point can drive itself inside its own band floor and oscillate between attack states. Default `minRange` 0 avoids this; authors should not set band floors until combat positioning ships.
- **Repeat-attack replication.** Re-firing the same attack restarts the clip via `restart_animation_clip`, which changes no state name and so produces no wire delta — remote clients see the first swing clamp. Pre-existing gap (single-attack enemies have it today); distinct attacks mask it. A wire restart signal is future netcode work.
