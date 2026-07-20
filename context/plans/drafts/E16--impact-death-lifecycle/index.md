# Impact / Death Lifecycle (E16)

## Goal

The engine owns IMPACT — the single point where damage is applied. Make DEATH a modder policy derived from impact, not an engine behavior. Ship the shared keystone (per-entity modder state, IR-bindable) that both the death lifecycle and the coming enemy-behavior-state system need. A mod authors what a hit *means* — kill, overkill, stagger, down-and-resurrect — as data over impact facts; the engine evaluates it.

## Scope

### In scope
- **Impact dispatch source.** A new §12 dispatch source fired at the damage chokepoint, publishing an ephemeral dispatch scope: the damaged entity (target), the damager (source), the damage amount, pre-impact health, **unfloored** post-impact health, and max health. This is the one net-new engine event.
- **0 HP made inert.** Reaching 0 HP no longer auto-removes an entity or forces a death state. Removal happens only via an explicit modder `despawn` effect. Kill *detection* (the latch / kill report) survives as a script-visible fact.
- **Per-entity modder state (the keystone).** A per-instance, modder-owned, name-keyed number state on an entity, exposed to the IR as a new binding scope: readable as an IR leaf, writable as an IR output. Host-authoritative.
- **New effects.** `despawn({afterMs?})`, `setHealth(value, {afterMs?})` (absolute; resurrect re-arms death detection), `playAnim(clip)`, `slot.add(delta)`.
- **Authoring surface.** `defineImpactEvent(filter, policy) → ImpactEvent` blessed handle; `handle.override(filter, policy)` cross-scope refinement merged by identity; the `NumberRef`/`BoolRef` IR skin over the shipped `runtime.*` builder with `.and/.or/.not/.select` sugar.

### Out of scope (the later behavior-states project)
- **Per-entity state replication.** Per-entity modder state is host-authoritative only. State that must reach clients uses a shared global store, as today. No per-entity/per-owner replication channel.
- **Opening the AI FSM state set.** The engine FSM keeps its closed locomotion states (idle/alert/attack). No author-declared AI states, no replacement of the transition core. Modder overlay states (stagger/downed) live in per-entity state, invisible to the FSM.
- **Fine-grained downed-enemy AI.** Coarse engagement disable for a downed/dead enemy reuses the existing aggro toggle; nuanced downed behavior is deferred.
- **General deferred-effect scheduler.** Only the per-effect `afterMs` countdowns for `despawn`/`setHealth`. No generic timer/queue for arbitrary effects.
- **Healing via the damage path** and non-impact health observation (regen, low-health UI) — unchanged, separate surfaces.

## Decisions

Pinned semantics (each forced by an acceptance fixture; no TBD).

- **Gated-group evaluation: INDEPENDENT.** Within one `defineImpactEvent` policy, every `{ when, do }` group whose `when` holds fires; effects within a group all fire. The author guarantees exclusivity for mutually-exclusive states (the lifecycle fixture does this with `.and(state.eq(...))`). Forced by the override fixture: a base group and an added group sharing the same `when` must BOTH fire (death effects + the arena style payout) — first-match would drop the second.
- **Base + override: whole-policy REPLACE, most-specific-wins.** For one `ImpactEvent` identity, an entity runs exactly one variant — the last-declared whose filter matches. An override for `zone: "arena_1"` replaces the base policy for those entities; the base still runs for entities the override doesn't match. Reuse is authoring-level: the override calls the base policy function and spreads its result. Base and override never both fire for the same entity.
- **FSM coexistence (v1).** (a) The FSM's 0-HP→Death transition and auto-despawn are removed; 0 HP is inert. (b) The FSM retains idle/alert/attack/death; the terminal death/steering-hold becomes reachable via a modder `despawn` effect (which drives the existing despawn timer), not via 0 HP. (c) Modder overlay states (stagger/downed) live in per-entity state and are invisible to the FSM. (d) The existing resurrect recovery (HP restored above 0 → idle) is preserved. (e) Coarse engagement disable for downed/dead reuses the existing aggro toggle.
- **Effect arms.** Consequential effects (health/state/store writes, despawn, grant) evaluate in-tick, host-authoritative. Presentation effects (`playAnim`) enqueue on the existing app-drain path (scripting §10.4). Cross-arm order is fixed (consequential before presentation) and not author-controlled.
- **`healthAfter` is unfloored.** The impact fact carries the true pre-`.max(0.0)` value (may be negative); the stored health component still floors at 0 for the HUD. This makes kill (`healthAfter.le(0)`) and gib (`healthAfter.le(-40)`) both expressible.

## Acceptance criteria

- [ ] A damaging hit on an entity matching a `defineImpactEvent` filter runs its policy; IR facts evaluate against the real pre/post health of that hit. A policy granting `source` an amount computed from `target.level` credits the damager.
- [ ] An entity reduced to 0 HP by a policy that omits `despawn` **remains present** (not auto-removed); a later frame can still observe and act on it.
- [ ] A `despawn()` effect removes the entity; `despawn({ afterMs: N })` removes it approximately N ms later, not the same tick.
- [ ] Zombie fixture: an entity whose policy, at `healthAfter.le(0)` but above the gib threshold, plays a "down" clip and `setHealth(x, { afterMs })` is not removed and regains health after the delay (resurrect); an entity below the gib threshold despawns.
- [ ] Per-entity state: a policy calling `setState("stagger", 1)` causes a subsequent impact on the *same instance* to read `state("stagger") == 1` and branch; a second instance of the same type reads its own independent `0`.
- [ ] Override: a `zone: "arena_1"` override changes the policy for arena_1 entities only; entities of the same tag outside the zone run the base policy. No entity runs both.
- [ ] Independent gated groups: a single kill fires all matching groups (e.g. xp grant + death count + despawn together).
- [ ] Boolean composition: a policy using `.and(...)` type-checks and evaluates as the shipped `select` composition (no `and`/`or` opcode introduced).
- [ ] Scoring, HUD health, and replication behave unchanged for a normal kill+despawn (kill counted once, HUD reads 0, remote entity pruned on despawn).
- [ ] The three spike files (`arena-death.spike.ts`, `lifecycle.spike.ts`, `proposed.d.ts`) type-check against the *shipped* SDK after the authoring surface lands (WALL imports resolve to real `postretro` exports).

## Tasks

### Task 1: Impact dispatch source
Fire a new dispatch source at the damage chokepoint (`apply_damage_with_context`), after the decrement, publishing an ephemeral dispatch scope with these inputs: target-entity token, source-entity token, damage amount, pre-impact health, unfloored post-impact health (the `current - amount` value *before* the `.max(0.0)` floor — split the existing single statement so the unfloored value is captured, then floored), and max health. Register the scope in the reaction dispatch model (scripting §11-12) so a policy program binds these by name via `resolve_input`, exactly as existing dispatch sources do. The chokepoint has one caller path today (all producers route through it); the fire site sees `payload` (amount, source/last_attacker) and the health component (pre/post/max) — enumerate no new plumbing beyond reading those already-in-scope values. Emit host-authoritatively in the game-logic stage. Do not add policy evaluation here — this task only makes impact an observable, bindable event.

### Task 2: 0 HP inert + resurrect re-arm
Remove auto-removal at 0 HP. In `sweep_deaths`, keep the plain-non-player branch's tag/ledger/kill-report capture but delete its immediate `registry.despawn`. Gate the AI-tick auto-despawn: the FSM no longer transitions to Death from 0 HP, and `death_despawn_remaining_ms` is seeded only by a modder despawn effect (Task 4), not by reaching 0 HP. Preserve the existing resurrect recovery (HP restored above 0 clears the countdown and returns to idle); ensure an absolute health write (Task 4) resets the `death_handled` latch so a resurrected entity can die again. Reconcile the single `alive_players` occupancy predicate: decide and implement whether a latched-but-present 0-HP player counts as present (recommend: presence follows despawn, not HP — a 0-HP undespawned player is present). Verify scoring, HUD, and replication are untouched (they key off the kill report and despawn event). `ai.rs` and `health.rs` are large — make surgical edits along the existing seams; do not restructure.

### Task 3: Per-entity state component + EntityScope (keystone)
Add a per-instance, name-keyed number state carried on entities, modeled on the store `SlotValue`/`SlotSchema` vocabulary but keyed per `(entity, name)` rather than by global dotted name. Storage: a new closed component holding a small name→number map (the `HealthComponent.zone_multipliers` string-keyed-number-map is the structural precedent), materialized at spawn with author-declared defaults and reseeded on hot reload like the health pattern. Expose it to the IR as a new `BindingScope` (`EntityScope`) bound to a specific entity, so a program reading `state(name)` resolves to an IR input leaf and writing `setState(name, value)` resolves to an IR output — reusing the bind-once/eval-per-tick machinery (`StoreScope` is the template). Host-authoritative; no replication. Plumbing: the EntityScope needs the target entity id from the impact dispatch (Task 1 provides the target token) to resolve reads/writes against that entity's component.

### Task 4: Effects (despawn, setHealth, playAnim, slot.add)
Implement the four consequential/presentation effects as command-buffer instructions. `despawn({ afterMs? })`: immediate `registry.despawn` when no delay; with `afterMs`, seed the deferred-despawn countdown (copy the `death_despawn_remaining_ms` component-countdown idiom, ms-based, ticked in the game-logic stage) so removal fires after the delay independent of animation. `setHealth(value, { afterMs? })`: a new absolute-set chokepoint on the health component mirroring `apply_damage_with_context` (write `current`, clamp to `[0, max]`), resetting `death_handled = false`; with `afterMs`, defer via the same countdown idiom. `playAnim(clip)`: route through the existing `setAnimationState` primitive path (presentation, app-drain). `slot.add(delta)`: an SDK-side builder that lowers to a self-referential IR output `Add{ read(slot), delta }` on the same store slot — no new evaluator opcode; document the read-modify-write-races-LWW caveat. Deferred writes must be host-authoritative and deterministic in the game-logic stage.

### Task 5: Authoring surface (defineImpactEvent + override + IR skin)
Promote the WALL (`proposed.d.ts`) into the real `postretro` SDK. `defineImpactEvent(filter, policy)` is a pure builder returning a branded `ImpactEvent` handle (mirror `defineStore`: pure, no FFI, registered only by returning it through a manifest `events` child); `handle.override(filter, policy)` returns a linked variant carrying the base's identity. Lower a returned `ImpactEvent` (base + its overrides) into the manifest as a descriptor the engine merges by identity with most-specific-wins replace semantics (Decisions). The policy builder receives an `Impact` occurrence whose accessors return IR refs (`NumberRef`/`BoolRef`) and whose methods return effect descriptors; add the `NumberRef` op-set mirroring `runtime.*` and `BoolRef` `.and/.or/.not/.select` desugaring to `select`. A policy returns `EffectOrGroup[]`; groups evaluate independently (Decisions). Ensure the three spike files type-check against the shipped exports.

## Sequencing

**Phase 1 (concurrent):** Task 1 (impact dispatch source), Task 3 (per-entity state + EntityScope) — independent subsystems (chokepoint/dispatch vs. entity component/IR scope).
**Phase 2 (sequential):** Task 2 (0 HP inert) — shares `health.rs`/`ai.rs` with Task 1; sequence after to avoid conflicting edits at the chokepoint.
**Phase 3 (sequential):** Task 4 (effects) — consumes Task 3's EntityScope write-binding and Task 2's inert/despawn semantics.
**Phase 4 (sequential):** Task 5 (authoring surface) — consumes Task 1's dispatch scope, Task 3's state binding, and Task 4's effect contract (pinned in the boundary inventory).

## Boundary inventory

Casing per scripting §4 (primitives camelCase, types PascalCase, `@` reserved for ephemeral dispatch inputs §5).

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| define impact event | new `ImpactEventDescriptor` builder | manifest `events[]` entry | `defineImpactEvent(filter, policy)` | `defineImpactEvent(...)` |
| event handle type | descriptor + identity | — | `ImpactEvent` | (table) |
| override | linked descriptor (base identity) | `events[]` entry w/ base id | `handle.override(filter, policy)` | `:override(...)` |
| impact occurrence | dispatch scope | — | `Impact` (param) | (table) |
| dispatch inputs | `resolve_input` names | `{op:"input", name:"@…"}` | `impact.target/source/amount/healthBefore/healthAfter/maxHealth` | same |
| per-entity state read | `EntityScope` input leaf | `{op:"input", name}` | `target.state(name)` | `target:state(name)` |
| per-entity state write | `EntityScope` output | command-buffer output | `target.setState(name, value)` | `target:setState(...)` |
| absolute health write | new health chokepoint | command output | `target.setHealth(value, opts?)` | `target:setHealth(...)` |
| deferred despawn | `*_despawn_remaining_ms` idiom | command output | `target.despawn(opts?)` | `target:despawn(...)` |
| play animation | `setAnimationState` path | reaction command | `target.playAnim(clip)` | `target:playAnim(...)` |
| additive slot write | self-referential IR `Add` | command output | `slot.add(delta)` | `slot:add(...)` |

## Script syntax examples

See the fixtures: `arena-death.spike.ts` (grunt baseline, arena cross-scope override, zombie), `lifecycle.spike.ts` (Doom-2016 stagger/glory-kill). Canonical shape:

```ts
// Proposed design — the death policy is data over impact facts.
const gruntImpactEvent = defineImpactEvent({ tag: "grunt" }, (impact) => [
  { when: impact.target.healthAfter.le(0), do: [           // author-defined death, in IR
      impact.target.playAnim("death"),
      impact.source.grant("xp", impact.target.level.times(200)),
      impact.target.despawn({ afterMs: 1500 }),            // engine does not auto-remove at 0 HP
  ]},
]);
// A map refines the same handle in a different scope; merged by identity, most-specific-wins.
gruntImpactEvent.override({ zone: "arena_1" }, (impact) => [ /* reuse base + style payout */ ]);
```

## Open questions

- **`afterMs` ownership on non-brain entities.** The deferred-despawn idiom lives on `BrainComponent` today. A general `despawn({afterMs})`/`setHealth({afterMs})` on a non-brain entity needs a countdown field with an owner. Decide during Task 4: a small dedicated deferred-write component vs. widening an existing one. Constraint: ticked host-authoritatively in the game-logic stage, deterministic.
- **Per-entity state schema declaration.** Where the author declares an entity's state fields + defaults (a `components.state` block on the entity descriptor, paralleling `components.health`) vs. implicit-on-first-write. Recommend explicit declaration for validation and spawn-seeding; settle in Task 3.
