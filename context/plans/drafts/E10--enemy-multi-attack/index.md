# Enemy Multi-Attack

## Goal

A character descriptor carries a **list of attacks** — e.g. a melee swipe plus a ranged zap (Quake Ogre), or two distinct melee attacks (Fiend) — selected per engagement by range band, cooldown, and declaration priority. Attacks are references to weapon descriptors (the roadmap's "attacks are weapons/wieldables" model), so player and enemy attacks share one authoring substrate. The API is a list from day one: no privileged "primary" slot. Resolution stays hitscan/contact in this spec; a projectile attack later is a new `ResolutionMode` variant dropping into the same shape.

## Scope

### In scope

- `components.ai.attacks`: ordered list of attack entries. Each entry references a weapon descriptor by canonical name and adds AI-side fields (engage band, animation state).
- `ResolutionMode::Contact` — weapon-level variant for melee: cooldown-gated direct damage at range, no ray. Preserves today's enemy contact-damage behavior exactly.
- Per-attack cooldown state and deterministic selection in the AI tick: eligible = cooldown ready AND target XZ distance within `[minRange, maxRange]`; first eligible in declaration order wins.
- Enemy hitscan firing through the weapon module's existing ray + nearest-of resolution path, with shooter exclusion. A hitscan enemy attack is blocked by world geometry — resolution-level occlusion, not a perception model.
- Legacy compatibility: descriptors without `attacks` synthesize a single Contact attack from the existing `attackRange`/`attackDamage`/`attackCooldownMs` scalars, behavior-identical. Mixed authoring (both `attacks` and any legacy attack scalar) is a parse error.
- Per-attack animation: each entry may name an animation state; default is the existing `states.attack` mapping. Spawn-time validation extends the existing unmapped-state warning.
- Reference enemy archetype gains a second (hitscan) attack; agent diagnostics overlay labels the selected attack.
- Behavior-preserving split of `scripting/systems/ai.rs` (839 lines) before extension.

### Out of scope

- Projectile attacks (new `ResolutionMode` variant later; this spec only keeps the enum extensible).
- Runtime wieldable *entities* per attack (companion-entity equipping); stats resolve at spawn into brain-side tuning.
- Perception/LOS for target *selection* — the `visible` predicate stays `None`; occlusion here gates only hitscan attack *resolution*.
- Attack windups, telegraphs, damage synced to animation frames (damage stays cooldown-gated at fire time).
- Player weapon changes: `FireMode` is ignored on the enemy-driven fire path (the AI decides when to fire).
- Leash/pursuit policy, squad coordination, hostility/faction (other slices of the roadmap's behavior-descriptors bullet).
- Stagger/pain interrupts (`E10--enemy-stagger`).

## Acceptance criteria

- [ ] A descriptor with two attacks (contact melee + hitscan) parses and validates identically in QuickJS and Luau; rejections carry pathed errors in both: empty `attacks` list, `minRange > maxRange`, any legacy attack scalar alongside `attacks`. An unresolvable weapon name fails at spawn with the entity's descriptor name in the error.
- [ ] Descriptors without `attacks` behave bit-for-bit as today: the full existing AI test suite passes unchanged, and the legacy reference enemy's transition/damage cadence is unchanged.
- [ ] On a flat fixture with the two-attack reference enemy: at a distance inside only the hitscan band, the player takes that weapon's damage once per that attack's cooldown with that attack's animation state active; inside the melee band, the melee attack's damage and animation apply instead.
- [ ] Per-attack cooldowns are independent: firing one attack does not reset or delay the other's cooldown.
- [ ] An in-band enemy with a clear line to its target lands hitscan damage; the same enemy behind world geometry deals none. The firer can never hit itself.
- [ ] Selection is deterministic: when several attacks are eligible, declaration order wins; sim determinism tests stay green.
- [ ] The Attack logical state is entered when the target is inside any attack's band and exited when inside none; with no attack eligible but the target in band, the enemy holds in Attack facing the target (today's between-cooldowns behavior).
- [ ] SDK typedef drift tests pass with `attacks` present in both `postretro.d.ts` and `postretro.d.luau` committed fixtures.
- [ ] The agent diagnostics overlay state label shows the selected attack's name for agents in Attack state.
- [ ] On a connected client, a host enemy's attack switch shows as a change of replicated animation state name with no wire-format change.
- [ ] `scripting/systems/ai.rs` is split before extension; the split commit is behavior-preserving (full test suite green, no signature changes visible outside the module).

## Tasks

### Task 1: Split `ai.rs`

Behavior-preserving split of `crates/postretro/src/scripting/systems/ai.rs` (839 lines; `ai_tests.rs` at 2130 lines is `#[path]`-included at its tail). Natural seam: the pure FSM core (`evaluate_transition`, `select_target`, transition helpers, tuning types re-exports) versus the tick orchestration (`run_ai_tick`'s snapshot/compute/apply/despawn passes, facing, animation dispatch). Keep `pub(crate)` surfaces identical so no caller outside the module changes. Split the test file along the same line. No behavior change; full suite green is the gate.

### Task 2: Attack descriptor surface

Add `AttackDescriptor` and `attacks: Option<Vec<AttackDescriptor>>` to `AiDescriptor` in `crates/foundation/src/data_descriptors/types/combat.rs`. Fields per the boundary inventory: `name` (unique within the list), `weapon` (canonical weapon-descriptor name, resolved at spawn — the `default_weapon: Option<String>` reference precedent), `minRange` (optional, default 0), `maxRange` (optional, default = referenced weapon's `range`), `animationState` (optional, default = the `states.attack` mapping). Add `ResolutionMode::Contact` beside `Hitscan` (camelCase wire values, matching the enum's existing serde). Relax the legacy trio `attackRange`/`attackDamage`/`attackCooldownMs` to optional with cross-validation in `AiDescriptor::validate()`: exactly one of {legacy trio complete, `attacks` non-empty} must hold; per-entry checks (finite bands, `minRange <= maxRange`, non-empty names, unique names) carry wire-cased paths like `components.ai.attacks[1].maxRange`. Both script runtimes inherit parsing through the shared serde + `validate()` funnel (`js/entity.rs`, `lua/entity.rs` — verify no per-runtime shim needs the new field). Regenerate SDK typedefs and update the committed fixture files that pin them.

### Task 3: Spawn resolution into brain tuning

At archetype spawn, resolve each attack entry's weapon reference and materialize a per-attack tuning table on `AiTuning` (name, damage, range band, cooldown, resolution, animation state), mirroring how `AiTuning::from_descriptor` copies scalars today; unresolvable weapon names fail spawn validation with the entity's descriptor name in the error. Legacy descriptors synthesize one Contact attack from the trio here, so downstream code sees exactly one shape. `BrainComponent` gains per-attack cooldown state (`Vec<f32>`, serde-default, index-aligned with the tuning table) replacing the single `attack_cooldown_remaining_ms` read path — keep the old field serde-tolerated for existing saves but unused. Extend `validate_brain_animation_states` to cover per-attack animation names. Enumerate and update every reader of `tuning.attack_damage`, `tuning.attack_range`, and `tuning.attack_cooldown_ms` (FSM transitions, AI apply pass, tests).

### Task 4: Selection and firing in the AI tick

In the compute pass: derive eligibility per attack (cooldown ready, XZ distance in band), select the first eligible by declaration order, and carry the selection on `EnemyOutcome`. `evaluate_transition` gates Alert↔Attack on "distance inside any attack's band" instead of the single `attack_range`. In the apply pass: Contact attacks keep today's `apply_damage` + `enemyAttack` event + in-state clip restart; hitscan attacks synthesize origin (enemy eye) and direction (toward the selected target's hitbox center) and resolve through the weapon module's ray path — extract the world-ray + `nearest_entity_hit` + nearest-of resolution into a seam callable without a `WeaponComponent`, add an ignore-shooter parameter to `nearest_entity_hit` (enumerate its callers: the player fire path passes its firing pawn, tests updated), route the hit through the same zone-multiplier scaling and `apply_damage` the player path uses. Animation dispatch uses the selected attack's state name. Attack fires re-arm only the fired attack's cooldown.

### Task 5: Reference archetype, overlay, fixture verification

Give the reference enemy archetype a second attack: a hitscan zap weapon descriptor plus an `attacks` list ordering melee first. Add the attack clip/state mapping to its mesh animations. Extend the agent diagnostics overlay state label with the selected attack name. Verify the two-band behavior and occlusion ACs on the movement-feel fixture map, and confirm multiplayer clients show distinct attack animation states via the replicated state name (no wire change expected).

## Sequencing

**Phase 1 (concurrent):** Task 1 (ai.rs split — blocks 3, 4), Task 2 (descriptor surface, different crate).
**Phase 2 (sequential):** Task 3 — consumes Task 2's descriptor shape, lands in Task 1's split layout.
**Phase 3 (sequential):** Task 4 — consumes Task 3's tuning table and Task 1's module seams.
**Phase 4 (sequential):** Task 5 — exercises Task 4 end to end.

Cross-spec: run after `E10--enemy-combat-positioning` and `E10--enemy-facing-slew` land (both edit the same AI tick region; positioning also owns the destination the attack bands now inform). `E10--enemy-stagger` shares Task 1's split — whichever spec runs first executes it.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| attacks list | `AiDescriptor::attacks: Option<Vec<AttackDescriptor>>` | `"attacks"` | `attacks?: AttackDescriptor[]` | `attacks: {AttackDescriptor}?` | n/a |
| attack name | `AttackDescriptor::name` | `"name"` | `name` | `name` | n/a |
| weapon reference | `AttackDescriptor::weapon` | `"weapon"` | `weapon` | `weapon` | n/a |
| band floor | `AttackDescriptor::min_range` | `"minRange"` | `minRange?` | `minRange?` | n/a |
| band ceiling | `AttackDescriptor::max_range` | `"maxRange"` | `maxRange?` | `maxRange?` | n/a |
| attack animation | `AttackDescriptor::animation_state` | `"animationState"` | `animationState?` | `animationState?` | n/a |
| melee resolution | `ResolutionMode::Contact` | `"contact"` | `"contact"` | `"contact"` | n/a |

Legacy trio (`attackRange`, `attackDamage`, `attackCooldownMs`) keeps its existing casing, becomes optional.

## Script syntax examples

```ts
// Proposed design
export const grunt = defineEntityType({
  // ...
  components: {
    health: { max: 60 },
    ai: {
      detectionRange: 16, leashRange: 50, moveSpeed: 3.2, deathDespawnMs: 1200,
      attacks: [
        { name: "claw", weapon: "grunt_claw", maxRange: 1.8, animationState: "attack_claw" },
        { name: "zap",  weapon: "grunt_zap",  maxRange: 14,  animationState: "attack_zap" },
      ],
      states: { idle: "idle", alert: "walk", attack: "attack_claw", death: "death" },
    },
  },
});

export const gruntZap = defineEntityType({
  // ...
  components: { weapon: { damage: 7, range: 18, fireRateMs: 1600, fireMode: "semi", resolution: "hitscan" } },
});
```

## Open questions

- **Combat-positioning handoff.** Positioning's `engagement_radius` is a single scalar. Rule here: it reads the selected attack's `maxRange`, falling back to the largest band when nothing is eligible; an attack switch does not force a combat-slot re-score in v1. Revisit if playtests show band-thrash.
- **`minRange > 0` before positioning lands.** A far-band-only enemy chasing the target's center point can drive itself inside its own band floor and oscillate Alert↔Attack. Default `minRange` 0 avoids this; authors should not set band floors until combat positioning ships.
- **Repeat-attack replication.** Re-firing the same attack restarts the clip via `restart_animation_clip`, which changes no state name and so produces no wire delta — remote clients see the first swing clamp. Pre-existing gap (single-attack enemies have it today); distinct attacks mask it. A wire restart signal is future netcode work.
