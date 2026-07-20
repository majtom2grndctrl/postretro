# Impact / Death Lifecycle (E16)

## Goal

The engine owns IMPACT — the single point where damage is applied. Make DEATH a modder policy derived from impact, not an engine behavior. Ship the shared keystone (per-entity modder state, IR-bindable) that both the death lifecycle and the coming enemy-behavior-state system need. A mod authors what a hit *means* — kill, overkill, stagger, down-and-resurrect — as data over impact facts; the engine evaluates it. This re-thinks the roadmap's Combat Feedback & Economy `onImpact` / `CombatScope` substrate around impact-derived death.

## Scope

### In scope
- **Impact dispatch source.** A new §12 dispatch source fired at the damage chokepoint, publishing an ephemeral dispatch scope: number/bool IR facts (damage amount, pre-impact health, **unfloored** post-impact health, max health) plus two **command-target tokens** (target = damaged entity, source = damager). The one net-new engine event.
- **0 HP made inert.** Reaching 0 HP no longer auto-removes an entity or forces a death state. Removal happens only via an explicit modder `despawn` effect. Kill *detection* (the `death_handled` latch / kill report) survives as a script-visible fact.
- **Per-entity modder state (the keystone).** A per-instance, modder-owned, name-keyed number state on an entity, exposed to the IR as a new binding scope: readable as an IR leaf, writable as an IR output. Host-authoritative.
- **New effects.** `despawn({afterMs?})`, `setHealth(value, {afterMs?})` (absolute; resurrect re-arms kill detection), `setState(name, value)`, `playAnim(clip)`, `slot.add(delta)`.
- **Authoring surface.** `defineImpactEvent(filter, policy) → ImpactEvent` blessed handle; `handle.override(filter, policy)` cross-scope refinement; the `NumberRef`/`BoolRef` IR skin over the shipped `runtime.*` builder with `.and/.or/.not/.select` sugar.
- **Engine-side policy evaluator.** The runtime that binds each policy's gated-group IR to an impact fire, evaluates it, resolves base+override, and dispatches effects (Task 6).

### Out of scope
- **Crediting the source (rewards).** `grant(...)` — awarding xp / score / ammo / health to the damager — defers to Epic 16's **`resource-grant chokepoint + dev-mod reference`** item (Combat Feedback & Economy). Per-player resources also need a replication story (below). The impact scope still publishes `source` as a command-target token so the seam is charted; no v1 effect targets it.
- **Per-entity state replication.** Per-entity modder state is host-authoritative only. State that must reach clients uses a shared global store, as today. No per-entity/per-owner replication channel.
- **Opening the AI FSM state set.** The engine FSM keeps its closed locomotion states (idle/alert/attack/death). No author-declared AI states, no replacement of the transition core. Modder overlay states (stagger/downed) live in per-entity state, invisible to the FSM.
- **Fine-grained downed-enemy AI.** Coarse engagement disable for a downed/dead enemy reuses the existing aggro toggle; nuanced downed behavior is deferred.
- **General deferred-effect scheduler.** Only the per-effect `afterMs` countdowns for `despawn`/`setHealth`. No generic timer/queue for arbitrary effects.
- **Spatial `zone` filtering.** Overrides narrow by an additional tag, not by a spatial region. No zone-membership subsystem.
- **Healing via the damage path** and non-impact health observation (regen, low-health UI) — unchanged, separate surfaces.

## Decisions

Pinned semantics (no TBD).

- **Entity references are command-target tokens, not IR leaves.** The IR carries only number/bool. `target`/`source` cross as opaque command-target tokens in the dispatch scope — mirroring trigger events' `on.activators`/`on.trigger` — consumed by EntityScope binding and by every effect writer, never as `resolve_input` number leaves.
- **Gated-group evaluation: INDEPENDENT.** Within one policy, every `{ when, do }` group whose `when` holds fires; effects within a group all fire. The author guarantees exclusivity for mutually-exclusive states (the lifecycle fixture uses `.and(state.eq(...))`). Forced by the override fixture: a base group and an added group sharing the same `when` must BOTH fire — first-match would drop the second.
- **Base + override: whole-policy REPLACE, MOST-RECENTLY-EXECUTED wins.** For one `ImpactEvent` identity, an entity runs exactly one variant — the last override to execute whose filter matches. Filters narrow by an additional tag (an entity may carry several tags); authors scope overrides so it is clear where a modified rule applies and where it does not. Reuse is authoring-level: the override calls the base policy function and spreads its result. Base and override never both fire for the same entity.
- **Cross-FFI identity.** `defineImpactEvent` takes no name, but base↔override linkage must survive the VM drop — JS object identity does not cross the FFI. The engine assigns each `defineImpactEvent` a stable derived id (as `defineReaction` derives reaction identity); `override` carries its base's derived id in the returned descriptor.
- **Producer contexts (both charted).** Damage reaches the chokepoint from two producers: in-tick weapon fire and the app-drain `applyDamage` reaction. v1 implements impact dispatch + in-tick consequential evaluation for the in-tick producer, and **stubs the app-drain producer at the same seam** (the dispatch fires; its consequential effects route to the next tick's game-logic stage). Charting both now avoids an API footgun when DoT/environmental damage (app-drain-sourced) lands.
- **FSM coexistence (v1).** (a) The FSM's 0-HP→Death transition and auto-despawn are removed; 0 HP is inert. (b) The FSM retains idle/alert/attack/death; terminal death/steering-hold becomes reachable via a modder `despawn` effect (which drives the existing despawn timer), not via 0 HP. (c) Modder overlay states (stagger/downed) live in per-entity state, invisible to the FSM. (d) Coarse engagement disable for downed/dead reuses the existing aggro toggle.
- **Resurrect & kill re-arm.** The engine's resurrect recovery keys off `brain.state == Death`, not the `death_handled` latch; `death_handled` is set only by the death sweep and is never cleared today. Once 0-HP→Death is removed, the `brain.state` recovery no longer fires from HP restoration — so the load-bearing re-arm is `setHealth` resetting `death_handled`, which re-enables kill detection for a resurrected entity. "Preserve resurrect recovery" means preserve the `death_handled` re-arm, not the now-vestigial FSM-state recovery.
- **Effect arms.** Consequential effects (health/state/store writes, despawn) evaluate in-tick, host-authoritative. Presentation effects (`playAnim`) enqueue on the existing app-drain path (scripting §10.4). Cross-arm order is fixed (consequential before presentation) and not author-controlled.
- **Despawn ordering.** A `despawn` (even with no `afterMs`) removes the entity at end-of-frame, after the presentation drain — so a same-group `playAnim("death"/"gib")` still targets a live entity and plays. `afterMs` extends the delay further.
- **`healthAfter` is unfloored.** The impact fact carries the true pre-`.max(0.0)` value (may be negative); the stored health component still floors at 0 for the HUD. This makes kill (`healthAfter.le(0)`) and gib (`healthAfter.le(-40)`) both expressible.
- **Manifest composition.** `events` composes with the §2 rule: mod-scope `events` are global; level-scope `events` add on `setupLevel`; the standard `levels` selector applies. Composition is additive first, then identity-merge resolves base+overrides.

## Acceptance criteria

- [ ] A damaging (in-tick) hit on an entity matching a `defineImpactEvent` filter runs its policy; IR facts evaluate against the real pre/post health of that hit.
- [ ] An entity reduced to 0 HP by a policy that omits `despawn` **remains present** (not auto-removed); a later frame can still observe and act on it.
- [ ] A `despawn()` removes the entity at end-of-frame — a same-group `playAnim` still plays; `despawn({ afterMs: N })` removes it ~N ms later.
- [ ] Zombie fixture: an entity at `healthAfter.le(0)` above the gib threshold plays a "down" clip and `setHealth(x, { afterMs })` is not removed and regains health after the delay; below the gib threshold it despawns.
- [ ] Per-entity state: a policy calling `setState("stagger", 1)` causes a subsequent impact on the *same instance* to read `state("stagger") == 1` and branch; a second instance reads its own independent `0`.
- [ ] Override: an override narrowing by an extra tag changes the policy for that subset only; the base still runs for entities the override doesn't match; no entity runs both. When two overrides match, the most-recently-executed wins.
- [ ] Independent gated groups: a single kill fires all matching groups (e.g. death-count store write + despawn together).
- [ ] Boolean composition: a policy using `.and(...)` type-checks and evaluates as the shipped `select` composition (no `and`/`or` opcode introduced).
- [ ] A resurrected entity (via `setHealth`) can be killed again — its `death_handled` latch is cleared, so kill detection re-arms.
- [ ] `alive_players` treats a latched-but-undespawned 0-HP player as present (presence follows despawn, not HP).
- [ ] Scoring, HUD health, and replication behave unchanged for a normal kill+despawn (kill counted once, HUD reads 0, remote entity pruned on despawn).
- [ ] The three spike files (`arena-death.spike.ts`, `lifecycle.spike.ts`, `proposed.d.ts`) type-check against the *shipped* SDK after the authoring surface lands.

## Tasks

### Task 1: Impact dispatch source
Fire a new dispatch source at the damage chokepoint (`apply_damage_with_context`), after the decrement, publishing an ephemeral dispatch scope with: **number/bool IR facts** — damage amount, pre-impact health, unfloored post-impact health (the `current - amount` value *before* the `.max(0.0)` floor — split the existing single statement so the unfloored value is captured, then floored), and max health; plus **two command-target tokens** — target (damaged) and source (damager, from `last_attacker`) — carried in the scope like trigger events' `on.activators`/`on.trigger`, NOT as number leaves (the IR has no entity type). Register the number/bool facts in the reaction dispatch model (scripting §11-12) so a policy binds them by name via `resolve_input`; pin the literal `@`-input names (Boundary inventory). The fire site already has `payload` (amount, `last_attacker`) and the health component (pre/post/max) in scope — no new plumbing beyond reading them and splitting the floor statement. For v1 fire host-authoritatively in the game-logic stage from the in-tick producer; the app-drain `applyDamage` producer fires the same dispatch but its consequential effects defer to the next tick (Decisions: Producer contexts). Do not add policy evaluation here (Task 6) — this task only makes impact an observable, bindable event.

### Task 2: 0 HP inert + resurrect re-arm
Remove auto-removal at 0 HP. In `sweep_deaths`, keep the plain-non-player branch's tag/ledger/kill-report capture but delete its immediate `registry.despawn`. Gate the AI-tick auto-despawn: the FSM no longer transitions to `Death` from 0 HP, and `death_despawn_remaining_ms` is seeded only by a modder despawn effect (Task 4), not by reaching 0 HP. Correct the resurrect model: the existing `brain.state == Death` recovery becomes vestigial once the 0-HP→Death transition is gone (nothing puts a brain into `Death` from HP); the load-bearing re-arm for kill *detection* is resetting the `death_handled` latch (set only by the sweep, never cleared today), which Task 4's `setHealth` does. Reconcile the single `alive_players` occupancy predicate: a latched-but-undespawned 0-HP player counts as present (presence follows despawn, not HP). Verify scoring, HUD, and replication are untouched (they key off the kill report and the despawn event, not 0 HP). `ai.rs` and `health.rs` are large — make surgical edits along the existing seams; do not restructure.

### Task 3: Per-entity state component + EntityScope (keystone)
Add a per-instance, name-keyed number state carried on entities, modeled on the store `SlotValue`/`SlotSchema` vocabulary but keyed per `(entity, name)` rather than by global dotted name. Storage: a new closed `ComponentKind` variant holding a small name→number map (the `HealthComponent.zone_multipliers` string-keyed-number-map is the structural precedent), materialized at spawn with author-declared defaults and reseeded on hot reload like the health pattern. Expose it to the IR as a new `BindingScope` (`EntityScope`): `state(name)` resolves to an IR input leaf, `setState(name, value)` to an IR output. Crucially, follow the **`DispatchScope::seed` per-fire idiom, not `StoreScope`'s bind-once-against-a-stable-table**: bind the `state(name)` handle once, but supply the *current target entity id* as per-fire ambient state (like the `@`-seeded dispatch inputs), because one policy is bound once yet fires against a different target each impact. Plumbing: the per-fire target id comes from Task 1's target command-target token. Host-authoritative; no replication.

### Task 4: Effects (despawn, setHealth, setState, playAnim, slot.add)
Implement the effects as command-buffer instructions targeting the impact's command-target tokens (Task 1). `despawn({ afterMs? })`: schedule removal for end-of-frame after the presentation drain (so a same-group `playAnim` still plays); with `afterMs`, extend via a deferred countdown. Own the countdown with a **new dedicated deferred-effect component** (a small `Option<f32>` ms field per pending removal/write), not `BrainComponent` — so non-brain entities defer too; tick it host-authoritatively in the game-logic stage, copying the proven `death_despawn_remaining_ms` decrement idiom. `setHealth(value, { afterMs? })`: a new absolute-set chokepoint on the health component mirroring `apply_damage_with_context` (write `current`, clamp to `[0, max]`), resetting `death_handled = false` to re-arm kill detection; `afterMs` defers via the same component. `setState(name, value)`: an EntityScope IR output (Task 3). `playAnim(clip)`: route through the existing `setAnimationState` primitive path (presentation, app-drain). `slot.add(delta)`: an SDK-side builder that lowers to a self-referential IR output `Add{ read(slot), delta }` on the same store slot — no new evaluator opcode; document the read-modify-write-races-LWW caveat.

### Task 5: Authoring surface (defineImpactEvent + override + IR skin)
Promote the WALL (`proposed.d.ts`) into the real `postretro` SDK. `defineImpactEvent(filter, policy)` is a pure builder returning a branded `ImpactEvent` handle (mirror `defineStore`: pure, no FFI, registered only by returning it through a manifest `events` child); the engine assigns each a stable derived id (Decisions: Cross-FFI identity). `handle.override(filter, policy)` returns a linked variant carrying the base's derived id. Lower a returned `ImpactEvent` (base + its overrides) into the manifest as a descriptor the engine merges by that id. The policy builder receives an `Impact` occurrence whose accessors return IR refs (`NumberRef`/`BoolRef`) and whose methods return effect descriptors; add the `NumberRef` op-set mirroring `runtime.*` and `BoolRef` `.and/.or/.not/.select` desugaring to `select`. `target`/`source` are opaque command-target tokens (no IR-leaf accessors; `source` has no v1 methods — grant is deferred). A policy returns `EffectOrGroup[]`. Ensure the three spike files type-check against the shipped exports.

### Task 6: Engine-side impact-policy evaluator
Own the runtime that turns a registered `ImpactEvent` descriptor into per-fire behavior. On an impact dispatch (Task 1): resolve which event variant applies to the target — merge base + overrides by derived id, most-recently-executed winning for the target (Decisions), whole-policy replace. Bind each surviving policy's gated-group IR against the impact dispatch scope (number/bool facts) and the target's `EntityScope` (Task 3); evaluate each group's `when`; for every group whose gate holds (independent evaluation), dispatch its effects (Task 4) in the fixed consequential-before-presentation order. This is where the manifest `events` "lowers into" a chokepoint-registered predicate + effect dispatch. Bind-once/eval-per-fire, reusing the §11 evaluator; no live VM. Implement the in-tick producer path; leave the app-drain producer's deferred-evaluation path stubbed at the same seam (Decisions: Producer contexts).

## Sequencing

**Phase 1 (foundational):** Task 1 — the dispatch source + command-target token channel everything downstream binds to.
**Phase 2 (concurrent):** Task 2 (0 HP inert), Task 3 (per-entity state + EntityScope) — independent, both consume Task 1 (Task 3 needs the target token for per-fire seeding).
**Phase 3 (sequential):** Task 4 (effects) — consumes Task 3's EntityScope write-binding and Task 2's inert/despawn semantics.
**Phase 4 (sequential):** Task 5 (authoring surface + descriptor) — consumes Task 1's dispatch scope.
**Phase 5 (sequential):** Task 6 (evaluator) — consumes Task 1's scope+tokens, Task 3's binding, Task 4's effects, and Task 5's descriptor.

## Boundary inventory

Casing per scripting §4 (primitives camelCase, types PascalCase, `@` reserved for ephemeral dispatch inputs §5). Command-target tokens are a separate channel from number/bool IR inputs (Decisions).

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| define impact event | `ImpactEventDescriptor` builder + derived id | manifest `events[]` entry | `defineImpactEvent(filter, policy)` | `defineImpactEvent(...)` |
| event handle type | descriptor + derived id | — | `ImpactEvent` | (table) |
| override | linked descriptor (base derived id) | `events[]` entry w/ base id | `handle.override(filter, policy)` | `:override(...)` |
| impact occurrence | dispatch scope | — | `Impact` (param) | (table) |
| impact facts (IR leaves) | `resolve_input` names | `@impact.amount` · `@impact.healthBefore` · `@impact.healthAfter` · `@impact.maxHealth` | `impact.amount` · `impact.target.healthBefore/healthAfter/maxHealth` | same |
| impact entity tokens | command-target tokens | `@impact.target` · `@impact.source` (token channel, not IR leaves) | `impact.target` · `impact.source` | same |
| per-entity state read | `EntityScope` input leaf | `{op:"input", name}` | `target.state(name)` | `target:state(name)` |
| per-entity state write | `EntityScope` output | command-buffer output | `target.setState(name, value)` | `target:setState(...)` |
| absolute health write | new health chokepoint | command output | `target.setHealth(value, opts?)` | `target:setHealth(...)` |
| deferred despawn/write | new deferred-effect component | command output | `target.despawn(opts?)` | `target:despawn(...)` |
| play animation | `setAnimationState` path | reaction command | `target.playAnim(clip)` | `target:playAnim(...)` |
| additive slot write | self-referential IR `Add` | command output | `slot.add(delta)` | `slot:add(...)` |

## Script syntax examples

See the fixtures, split by job: `arena-death.spike.ts` (the handle model — grunt baseline + tag-narrowed override + policy reuse), `lifecycle.spike.ts` (the keystone — per-entity state, zombie resurrect, the Doom-2016 stagger/glory-kill machine). Canonical shape:

```ts
// Proposed design — the death policy is data over impact facts.
const gruntImpactEvent = defineImpactEvent({ tag: "grunt" }, (impact) => [
  { when: impact.target.healthAfter.le(0), do: [           // author-defined death, in IR
      impact.target.playAnim("death"),                     // presentation
      deaths.add(1),                                        // consequential store write
      impact.target.despawn({ afterMs: 1500 }),            // engine does not auto-remove at 0 HP
  ]},
]);
// A map refines the same handle in a different scope; merged by derived id, most-recently-executed.
gruntImpactEvent.override({ tag: "arena_grunt" }, (impact) => [ /* reuse base + extra store write */ ]);
```

## Open questions

- **Per-entity state schema declaration.** Where the author declares an entity's state fields + defaults (a `components.state` block on the entity descriptor, paralleling `components.health`) vs. implicit-on-first-write. Recommend explicit declaration for validation and spawn-seeding; settle in Task 3.
- **Roadmap reconciliation.** This draft re-thinks the Combat Feedback & Economy milestone's `onImpact` / `CombatScope` substrate; `grant` maps to that milestone's `resource-grant chokepoint` item. Whether to formally restructure those roadmap items around impact-derived death is a separate call for the epic owner.
