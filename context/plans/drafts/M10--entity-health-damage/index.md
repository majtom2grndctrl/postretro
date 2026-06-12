# Entity Health + Damage Surface

## Goal

Establish the engine's health/damage primitive on the Milestone 6 entity model: an entity carries HP, consumes a `DamagePayload`, and dies at zero. Closes the damage loop both ways — the shipped weapon gets a shootable target (hitscan now tests entity volumes), and the player takes damage from a reaction-driven stand-in until enemy AI lands. The real `player.health` producer replaces the M13 static-proxy stand-in behind the published slot contract. A foundation to grow: policy (regen, resistances, damage types) defers to the Shields + damage-type system over the M14 behavior-IR.

## Model anchor

**Script declares parameters, Rust runs the logic** — the M7/weapon precedent. A `components.health` block on `defineEntity` materializes into a `Health` component at spawn; Rust applies damage and resolves death. Damage arrives only as a `DamagePayload` (struct, never a bare scalar — `context/research/weapon-model.md` §7 #3), through one chokepoint function every producer calls. Damage sites only mutate the component; a per-tick sweep resolves death (collect-then-despawn, the particle-sim precedent) — required because reaction handlers see only the entity registry and cannot reach the progress tracker or fire events.

## Scope

### In scope

- `Health` component (new `ComponentKind`, next discriminant) + `components.health` descriptor block: `max`, optional `hitbox` (AABB half-extents + optional center offset). Typedefs regenerated; drift test passes. Hot-reload refresh mirrors the weapon component.
- One damage chokepoint: apply a `DamagePayload` to an entity; HP floors at zero; entities without `Health` ignore damage.
- Hitscan entity targeting: the weapon's fire path tests hitbox-bearing health entities along the aim ray, resolves nearest-of(world hit, entity hit) so walls block shots, clamps to weapon range, and carries the hit entity's id on the impact (spatial info rides beside the payload, never inside it).
- Death sweep (per game-logic tick): entities at zero HP despawn; their tags feed `ProgressTracker::on_entity_killed`; resulting event names fire after the tick loop. The player pawn (carries `PlayerMovement`) never despawns: HP latches at zero and a `playerDied` event fires exactly once.
- `applyDamage` tag-targeted reaction primitive — the player-side damage stand-in and a durable modder verb.
- Real `player.health` producer: each frame the engine publishes the pawn's current HP to the readonly `player.health` slot, replacing the static proxy's demo-health write (proxy keeps `player.ammo` and `intro.flashColor`).
- `player.health` slot gains a declared numeric range `[0, max]`, attached engine-side when the player's health component materializes (and refreshed on hot reload). Requires an engine-only slot-schema range mutation on the slot table.
- Reference content: a `target_dummy` archetype script (mesh + health) placed in a dev map; a level reaction damaging the player; a progress reaction demonstrating threshold-on-kills; modder-docs coverage of the new script surface in `docs/scripting-reference.md`.

### Out of scope

- Healing, regen, shields, resistances, damage types, crit, falloff — policy belongs to the Shields system over the M14 behavior-IR. `DamagePayload` stays amount-only; consumers must not assume it stays that way.
- Skeletal hit zones (bone-parented capsules) and per-zone damage multipliers — a later M10 plan. This plan's hitbox is one world-aligned AABB per archetype.
- Enemy AI, navigation, death animations. The sweep is the seam the AI plan interposes a death clip into (defer despawn); this plan despawns immediately.
- Respawn, death screen, input freeze at zero HP — M13 BIS territory. The pawn stays controllable at zero.
- Entity-entity overlap volumes, damage-over-time volumes (lava floors). `applyDamage` is the only non-weapon producer.
- Self-hit handling for hitscan. The player archetype declares no hitbox, so the firing player cannot be ray-targeted; revisit when enemies fire hitscans.
- A sequenced (per-entity step) variant of `applyDamage`. Plain named-reaction dispatch only.
- `player.ammo` producer — no ammo system exists; the proxy's demo value stands.
- FGD surface. Health params are descriptor-owned, never map-overridable (`entity_model.md` §4); placement rides the existing `canonicalName` data-archetype path.

## Acceptance criteria

- [ ] Shooting a map-placed entity whose archetype declares health + hitbox reduces its HP by the weapon's damage per hit, spawns the usual impact burst at the hit point, and despawns the entity when HP reaches zero. Verified by composed unit tests (fire path, chokepoint, sweep) plus a manual map run — the full `App` chain is not constructible under `cargo test`.
- [ ] A hitbox entity behind world geometry cannot be shot (the wall hit wins); a hitbox entity beyond weapon range cannot be shot; a shot past a near miss still hits the wall behind.
- [ ] A `progress` reaction over a spawn tag fires its event when the declared fraction of tagged health entities has been killed.
- [ ] An `applyDamage` reaction with a finite positive `amount` reduces every tagged target's HP; targets without a health component are skipped with a warning; a negative or non-finite `amount` warns and applies nothing.
- [ ] When the player pawn's HP reaches zero: HP reads 0 (never negative), the pawn remains controllable (manual-run check, not a runnable assertion), a `playerDied`-named reaction fires exactly once, and further damage neither re-fires it nor lowers HP.
- [ ] The `player.health` slot tracks the pawn's current HP frame-over-frame (the M13 HUD readout shows live damage), remains rejected for script writes, and — once a player with health has materialized — carries a declared range `[0, max]` that clamps out-of-range engine writes.
- [ ] `gen-script-types` output includes the health descriptor types; the type-definition drift test passes; a malformed health block (non-finite or non-positive `max`, non-positive hitbox extents) is rejected at declaration with an error, not a panic.
- [ ] Editing a health-bearing archetype's `max` and hot-reloading (debug) updates the live component; when the edited archetype is the player's, the slot range follows.
- [ ] `docs/scripting-reference.md` documents the `components.health` block, `applyDamage` (including its error-table rows), the `playerDied` event, and the readonly `player.health` read — in example-led, human-facing prose, not the context-library register.

## Tasks

### Task 1: Health component + descriptor block + damage chokepoint

Add `HealthComponent` (new file `scripting/components/health.rs`, mirroring `components/weapon.rs`): `max`, `current` (initialized to `max` at materialization), optional hitbox (half-extents + offset), and a `death_handled` latch — set once when the zero-HP player's death is reported, so the `playerDied` event fires exactly once; the death sweep (a later task) is its only writer. Add `ComponentKind::Health` (next discriminant after `Mesh = 9`) to the `VARIANTS` array, a `ComponentValue::Health` variant, and the `Component` trait impl. Extend `EntityTypeDescriptor` with `health: Option<HealthDescriptor>` (`data_descriptors.rs`), adding the `health` arm to **both** hand-written parsers — `entity_descriptor_from_js` and `entity_descriptor_from_lua` (a missing Luau arm compiles as `health: None` and silently drops Luau parity); validate fail-loud (`LightDescriptor::validate` precedent): `max` finite and `> 0`; each `halfExtents` element finite and `> 0`; each `offset` element finite. Wire keys are camelCase — `"health"`, `"max"`, `"hitbox"`, `"halfExtents"`, `"offset"` (`#[serde(rename_all = "camelCase")]` convention). Materialize in `attach_descriptor_components` (`builtins/data_archetype.rs`) with a new `DescriptorComponentKind::Health` provenance entry; include `health` in `is_directly_map_placeable` so a health-bearing archetype can be map-placed. Hot reload: add the `DescriptorComponentKind::Health` arm to the descriptor-refresh dispatch (`scripting/refresh_plan.rs` — `descriptor_declares` / `live_component_exists` / `plan_component_replace`, each an exhaustive match) via a `plan_health_replace` mirroring `plan_weapon_replace`, and a `HealthComponent::refresh_from_descriptor` mirroring the weapon component — `max` updates, `current` clamps to the new max, hitbox updates. Implement the chokepoint here: `apply_damage(registry, id, &DamagePayload)` — reusing the existing `crate::weapon::DamagePayload` (`weapon/mod.rs`), never a sibling type — subtracts `amount`, floors at zero, no-ops on entities without `Health`. Extend the typedef generator's type-name mapping (new Rust types do not auto-appear) and add `health?` to the `EntityTypeComponents` bag; regenerate typedefs.

### Task 2: Hitscan entity targeting

Extend the weapon fire path (`weapon/mod.rs`) so `fire_hitscan` also tests entity hitboxes: thread the entity registry in (the caller `weapon::tick` already holds `&mut EntityRegistry`), iterate `ComponentKind::Health` entities carrying a hitbox, ray-vs-AABB test each (AABB centered at `transform.position + offset`, world-aligned — entity rotation ignored), keep the nearest entity toi. Resolve nearest-of(world hit, entity hit), both clamped to weapon range. Add `target: Option<EntityId>` to `WeaponImpact` — spatial info rides beside the payload, never inside `DamagePayload`. An entity hit still populates `point`/`normal` (ray entry point, face normal of the struck AABB slab) so the impact burst renders. Damage application does not happen here — the caller consumes it (Task 3).

### Task 3: Death sweep + weapon-damage wiring

Wire the consumer side in `main.rs`. (a) Weapon: where the tick-loop caller handles `events.impact` (the `spawn_impact_effect_at` site), an impact carrying `target` also calls `apply_damage` with the payload from `ActivationOutcome::Hit`. (b) Death sweep: a per-tick pass in a new `scripting/systems/health.rs` (plus its `systems/mod.rs` declaration; the component/chokepoint file `components/health.rs` stays system-free — the `particle_sim` split), run in the game-logic stage after the weapon fire tick, two-pass like `particle_sim`: collect entities with `current == 0`; for each non-player (no `PlayerMovement` component) capture its tags, despawn, and report the kill; for the player, set `death_handled` once and report `playerDied`. The sweep returns reported kills/tags + the player-death flag; the `App` caller feeds tags through `self.progress_tracker.on_entity_killed` and accumulates returned event names plus `playerDied` into a `pending_death_events` list. Drain it after the tick loop in its own sibling loop calling `fire_named_event_with_sequences` — not folded into the existing `fire_named_event` drains, which would no-op a progress `fire` that names a sequence. Discard the returned chained-event names (`let _ =`), matching the existing drains. `App` already holds the sequence and reaction registries the call needs.

### Task 4: `applyDamage` reaction primitive

New `scripting/reactions/apply_damage.rs` mirroring `set_emitter_rate.rs`: args struct `{ amount: f32 }`, per-target dispatch calling Task 1's `apply_damage` with a `DamagePayload`. Negative or non-finite `amount` warns and no-ops (healing is not in scope); targets without `Health` warn and skip; empty target set debug-logs. Register as `"applyDamage"` alongside `setEmitterRate` in the existing builtin registration function in `scripting/reactions/registry.rs` — extend that function; no new `main.rs` call site. No sequenced variant. Death from reaction damage is resolved by the next death-sweep pass (reactions dispatch outside the tick loop) — the handler itself never despawns.

### Task 5: `player.health` producer + slot range

Replace the static proxy's demo-health write: `StaticUiProxy::tick` (`scripting/systems/ui_proxy.rs`) already holds a `ScriptCtx` clone — read the pawn (first `PlayerMovement` entity) health component via `ctx.registry` and `write_store_slot` its `current`; keep the `player.ammo` and `intro.flashColor` writes unchanged; update the module's "until M10" comments. No pawn or no health component → skip the write (the slot keeps its last value; acceptable, the table never clears). Slot range: add an engine-only mutation on `SlotTable` that sets `schema.range` on an `SlotOwnership::Engine` numeric slot (re-clamping any current value); existing `write_store_slot` validation then enforces it. Call it with `[0, max]` where the player materializes (`spawn_from_player_starts` caller in the level-install path, which holds `script_ctx`); read the pawn's `max` through the registry borrow already live at that site before it drops — a second `borrow()` under the live `borrow_mut` panics. Hot reload: the range hook lives at the refresh driver in `scripting/runtime.rs` (the `apply_descriptor_refresh_plan` call site — the only refresh site holding `ctx.slot_table`; `refresh_plan.rs`'s plan functions see only entity/descriptor/registry). After a refresh plan replaces a health component on the pawn (the entity carrying `PlayerMovement`), re-set the range to `[0, max]` unconditionally — idempotent, no `max`-delta detection needed. Factor the hook as a function taking the refresh plan + `ScriptCtx`, so the range-follow is unit-testable without the file watcher.

### Task 6: Reference content + integration demo

Add `health: { max: <pick a value> }` — deliberately **no** `hitbox` (the player is not ray-targetable) — to the player descriptor in `content/dev/scripts/player.ts`; without it the producer, the slot range, and the player-damage demo all silently no-op. A `target_dummy` reference script under `content/dev/scripts/` (TS, the authoring template): `canonicalName: "target_dummy"`, `components: { mesh, health: { max, hitbox } }`, reusing the existing demo glTF model; import and register it in the mod's `entities` list in `content/dev/start-script.ts` — there is no auto-scan. Place instances in a new `content/dev/maps/combat-demo.map` (the `anim-demo.map` precedent; keep `campaign-test.map` stable), compiled via `prl-build`, with a `_tags` spawn tag exclusive to the dummies (a shared tag skews the progress denominator, which counts all tagged entities) alongside a `player_spawn` tagged `player`. Level script declares: a `progress` reaction over the dummy tag, and an `applyDamage` reaction targeting the `player` tag named after the event the `progress` reaction fires — that event dispatches through the death-event drain (`fire_named_event_with_sequences`); the plain `fire_named_event` drains (movement/weapon names) never invoke primitive handlers, and `levelLoad` fires before the first rendered frame, so neither can drive a visible HUD drop. Keep the animation-demo map untouched — the grunt demo proves animation, this map proves combat. Document the new modder-facing surface in `docs/scripting-reference.md`: the `components.health` block (fields, validation, the optional hitbox and what carrying one means), the `applyDamage` reaction primitive (plus warn/skip rows in the error-handling table, mirroring the fog primitives' rows), the `playerDied` event, and the readonly `player.health` slot read. Tone: `docs/` is human-facing, not agent context — write example-led prose for a human engineer; do not use the context-library's token-lean register (the `context/lib/index.md` router separates `docs/` for exactly this reason).

## Sequencing

**Phase 1 (sequential):** Task 1 — component, descriptor, chokepoint; blocks everything.
**Phase 2 (concurrent):** Task 2 (weapon files), Task 4 (reaction files) — independent of each other, both consume Task 1.
**Phase 3 (sequential):** Task 3 — consumes Task 2's `target` id; owns the `main.rs` tick-loop wiring.
**Phase 4 (sequential):** Task 5 — also touches `main.rs` (proxy call site, level-install range hook); sequential to avoid shared-file conflicts with Task 3.
**Phase 5 (sequential):** Task 6 — consumes all prior tasks.

## Rough sketch

- **Component shape** (`// Proposed design`): `HealthComponent { max: f32, current: f32, hitbox: Option<Hitbox>, death_handled: bool }`, `Hitbox { half_extents: Vec3, offset: Vec3 }` (offset defaults to zero). `current` initializes to `max` at materialization. `death_handled` exists so the persisting zero-HP player reports death once; despawned entities never need it but the field is harmless.
- **Ray-vs-AABB**: slab test, no dependency needed (or `parry3d`'s `Aabb::clip_ray` — implementer latitude). Hit normal = axis of the entered slab, sign toward the ray origin.
- **Dead-entity targeting**: the sweep despawns at end of the same tick damage lands, so a zero-HP entity is never ray-targetable on a later tick. Within a tick the weapon fires at most once; no double-kill path.
- **Player detection** in the sweep: carries `PlayerMovement` (`entity_model.md` — "a player by virtue of carrying `PlayerMovement`").
- **Events**: death event names follow the `pending_movement_events` / `pending_weapon_events` pattern (accumulate during ticks, drain after the loop) but in a separate drain loop calling `fire_named_event_with_sequences` — the existing drains call plain `fire_named_event` and stay untouched.
- **Slot staleness**: after a level swap to a pawn-less map, `player.health` holds the previous level's last value (the slot table never clears by contract). Accepted; the fly-cam path has no HUD expectation.
- **Tags**: map `_tags` flow to spawned entities via `try_spawn(transform, &entity.tags)`, including the `player_spawn` placement — the demo map tags the player `player`; the engine forces no tag convention.
- **Key files**: `scripting/components/health.rs` (new), `scripting/registry.rs`, `scripting/data_descriptors.rs` (both the JS `entity_descriptor_from_js` and Luau `entity_descriptor_from_lua` parser paths gain a `health` arm), `scripting/builtins/data_archetype.rs`, `scripting/refresh_plan.rs` (hot-reload component-refresh dispatch — `DescriptorComponentKind::Health` arm), `scripting/runtime.rs` (drives the hot-reload refresh via `plan_descriptor_refresh` / `apply_descriptor_refresh_plan`; the only refresh site with `ctx.slot_table` access, so the slot-range hot-reload re-set hook lives here), `weapon/mod.rs`, `scripting/reactions/apply_damage.rs` (new) + `reactions/mod.rs` (module decl) + `reactions/registry.rs`, `scripting/systems/health.rs` (new, death sweep) + `systems/mod.rs` (module decl), `scripting/systems/ui_proxy.rs`, `scripting/slot_table.rs`, `scripting/typedef.rs`, `main.rs` (tick loop, level install), `content/dev/scripts/` (incl. `player.ts`), `content/dev/start-script.ts`, `content/dev/maps/combat-demo.map` (new).

## Boundary inventory

Rust snake_case; wire/JS/Luau camelCase (`#[serde(rename_all = "camelCase")]` convention). No FGD KVPs — health tuning is never map-overridable; only placement (`canonicalName`) and spawn tags (`_tags`) touch the map.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| health block | `health: Option<HealthDescriptor>` | `"health"` | `components.health` | `components.health` | n/a |
| max HP | `max: f32` | `"max"` | `max` | `max` | n/a |
| hitbox | `hitbox: Option<HitboxDescriptor>` | `"hitbox"` | `hitbox` | `hitbox` | n/a |
| half extents | `half_extents: [f32; 3]` | `"halfExtents"` | `halfExtents` | `halfExtents` | n/a |
| center offset | `offset: Option<[f32; 3]>` | `"offset"` | `offset` | `offset` | n/a |
| component kind | `ComponentKind::Health` | n/a | n/a | n/a | n/a |
| damage reaction | registered handler | `"applyDamage"` + args `{ "amount": f32 }` | `applyDamage` | `applyDamage` | n/a |
| player death event | `&'static str` | `"playerDied"` | reaction `name: "playerDied"` | same | n/a |
| health slot | dotted name | `"player.health"` | `"player.health"` | `"player.health"` | n/a |

Validation (fail-loud at declaration): `max` finite and `> 0`; each `halfExtents` element finite and `> 0`; each `offset` element finite. `applyDamage.amount`: finite and `>= 0` enforced at dispatch (warn + no-op on violation), not declaration — reaction args are level data.

## Decisions

1. **Dedicated `Health` component kind**, not a generic scalar-stat kind. Lean rule: smallest surface that reads game-y. Shields generalize the storage later; the script surface (a `health` block) is unaffected either way (`entity_model.md` §1).
2. **Player death = clamp at zero + `playerDied` event** (owner, 2026-06). No despawn (breaks movement/camera), no input freeze, no respawn — those are M13 BIS / later consumers of the event.
3. **`applyDamage` reaction is the player-side producer** (owner, 2026-06). Reuses the tag-targeted reaction registry; enemy AI later calls the same Rust chokepoint directly.
4. **`player.health` gains a declared range** (owner, 2026-06, overriding the keep-rangeless option). Range `[0, max]` attaches when the producer materializes — it cannot be declared at `SlotTable` construction because max HP is mod data. This amends the M13-shipped "no range" schema note; the M13 bind path reads values, not schema, so no UI change. Fix the roadmap/M13 wording at promotion.
5. **Hitbox is optional and lives in the health block.** Hitscan-targetable iff present. The player declares health without a hitbox (nothing ray-targets the player in M10 — also forecloses self-hit). A future hittable-without-health case would split it out; not now.
6. **Hitbox is one world-aligned AABB, fixed per archetype** (`entity_model.md` §7: AABB for enemies, size per type not per instance). Rotation-aware and skeletal volumes are the hit-zones plan.
7. **Damage mutates, the sweep resolves.** Handlers can't reach the progress tracker or event firing (reaction dispatch passes only the entity registry), and collect-then-despawn is the established pattern. The sweep is also the seam where the AI plan later interposes a death clip before despawn.
8. **No per-hit `damaged` event.** Only death is observable to reactions in M10; a damaged-event surface waits for a consumer.
9. **Health-bearing descriptors are map-placeable** (added to the placeable predicate) — a health-only placement is a legitimate invisible target.

## Open questions

- None blocking. Deferred-by-design items live in Out of scope; amendments to roadmap wording and the M13 schema note land at promotion, not during drafting.
