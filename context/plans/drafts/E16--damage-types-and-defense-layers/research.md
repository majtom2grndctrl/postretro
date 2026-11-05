# E16--damage-types-and-defense-layers — research

Read at 44881f79. Findings that informed the brief but do not decide it.

## Consolidation

These roadmap items were weighed together in the draft session:
- **Damage application order** (E16). This brief.
- **Per-entity state reads in pair expressions** (E18). Stays on the roadmap. It is a small additive change (`classify_pair_read`, `SideInput`, `seed_sides`, an SDK spelling). The per-owner store already covers its stated use case: see the healer recipe in `docs/scripting-reference.md`. With engine layers it reframes as pair-scope *layer* reads.
- **Local-slot crossing origin** (E15). Not combat work. The classifier exists: `crossing_machine_source` yields `MachineSource::OwnState` inside `FireOrigin::EveryMachine`. No defect is observed. Fold it into `coop--client-leave-session`. The roadmap's `drafts/reaction-body-composition` reference is stale; that plan is in `done/`.

The roadmap item "damage types / elemental" folds into this brief, because type-scaled drain needs a type on every hit.

## Layer home

The roadmap put layer values in per-entity `@state`. Source:
- `EntityStateComponent` is a schema-less f32 map. Every entity has one. A typo'd name reads as 0.
- Only impact-policy `setState` (on the target) and `adjustSentiment` write it. It has no per-tick writer, and reaction `setState` writes store slots, not entity state.
- It does not replicate. The only path to clients is the overlay's host-sampled `shield_fraction` / `has_shield` on `WireOverlayRoster`.
- `defineStore` rejects `perOwner` + `accumulate`, so recharge cannot live in the store per player either.
- Owner-slot `slot.set` in impact policies binds only to `@impact.source`, so a player's own shield cannot reach their HUD through the store.

## Damage chokepoint

- Every damage source calls `apply_damage_with_context`:
  - weapon impact (hitscan, projectile, splash, and host-validated client hits) via `apply_weapon_impact_damage_with_source`, which applies `zone_multipliers` first;
  - NPC contact via `AiHost::apply_damage`;
  - script `applyDamage` (`DamageProducer::AppDrain`);
  - mover crush (`kinematic_mover/blocking.rs`);
  - traps through the projectile path. Not traced end to end.
- Order inside the chokepoint: subtract, floor, `set_component`, contributor ledger, brain perception, `push_impact_dispatch`. Policies run in `evaluate_pending_in_tick_impacts` after `publish_health_from_registry`, `InTick` only.
- Damage also lands outside the tick: `host_flush_pending_hit_declarations` after the tick, and AppDrain at frame end. The last-damage stamp must therefore come from the chokepoint, not from a stage. The chokepoint has no tick counter today.

## Tick order

`simulate_started_tick_with_presentation_aim` runs these stages in order:
1. `begin_tick` (respawn drain)
2. `tick_deferred_effects`
3. Movers
4. Movement
5. Triggers
6. Projectiles
7. AI
8. Traps
9. Steering (crush)
10. Weapons
11. Death sweep
12. Assist

Recharge at tick start, after the respawn drain, puts every hit in tick N after that tick's recharge.

## Amount consumers

All of these read raw `payload.amount` today:
- the contributor ledger, kill credit and `player_contributor_ledger`;
- brain `record_attacker_damage` (threat and tolerance);
- brain recency (`time_since_damage_ms`, `damage_bearing`, `last_known_target_pos`);
- `ImpactDispatch` facts;
- overlay enrollment. Its `hide_at_full` checks health only.

Drafts that read the amount:
- `E10--enemy-stagger` accumulates `payload.amount` in the chokepoint.
- `coop--predicted-combat-feedback` relies on `@impact.amount` being client-predictable from `zone_multipliers`. Once `amount` means points dealt against host-only layers, it is no longer predictable; that draft moves to `@impact.raw` or to host-confirmed numbers. The owner chose the long-term vocabulary over keeping the drafts unchanged.

## Lifecycle seams

- Spawn: `HealthComponent::from_descriptor`. Hot reload: `refresh_from_descriptor`.
- Respawn: `respawn::drain` → `set_health_absolute`.
- Carry: `CarriedState.health_current`, written by `SeatTable::harvest_pawn_inner` through `CarriedField`, restored by `restore_carried_health` (local and net pawns).
- Same-map restart goes through the same unload path.

## Co-op

- Damage resolves only on the host, and clients predict no health.
- Engine owner-private slots form a fixed `PlayerSlot` enum plus catalog rows. A name-keyed family is a new mechanism.
- Slots are addressed by name on the wire. The owner-private weapon-sample slots are the version-gate precedent the owner-private Decision follows (`networking.md` §Version gates).
- A general declared-resource family (stamina, heat), with a defense layer as one kind, was considered. The `player.defense.` prefix is kept; a future resource family would sit beside it.

## Commitments touched

- `entity_model.md` §1: shields are an engine noun, and the component vocabulary is engine-closed. Mods extend by declared data.
- `scripting.md` §11: "the engine ships the component and its per-tick system; the author ships the policy". Interned IR identity is decided, not built (`E16--persistent-firing-modes` proposes building it). The brief avoids that dependency with Bool-per-type facts: each is an identity comparison expanded at bind, a workaround. Once interning lands, an interned type read can sit beside the Bools without breaking a mod.
- `research/weapon-model.md` §2–4: type grows inside the payload; augments may add a type. `WeaponComponent::effective()` exists as a passthrough.
- `research/combat-events.md` §4, §8: `damage` is post-mitigation, `rawDamage` is pre-mitigation, and `brokeShield` / `brokeArmor` / `element` are gated on this milestone. `@impact.absorbed` and `@impact.broke` are the substrate those facts reduce over.
- `plans/done/E16--combat-presentation-substrate`: the overlay `shield` reads `@state.shield`, with absorption deferred to Damage & Defenses.
- `plans/done/M10--skeletal-hit-zones`: zone multipliers apply before the chokepoint. The brief keeps that order: zone, then type and layers.

## Body order

Today a reaction's store writes do not apply at one moment on every source. Source:
- **Player and trigger events.** `partition_direct_reaction` keeps `grantHealth` and `addSlot` as consequential steps, and `bind_command_at` binds each as an in-tick command. `PlayerEventTable::fire_event` runs an event's commands in order, so body order already holds in-tick.
- **Reactions fired by address** (crossings, `levelLoad`, `fire` targets). `dispatch_add_owner_slot` (`primitive_dispatch.rs`) validates the slot and resolves seats at fire time, then pushes `SystemReactionCommand::AddOwnerSlot`; a `setState` step pushes `SystemReactionCommand::SetState` through `dispatch_system_reaction`. The app drains that queue in order (`dispatch_system_commands` in `main.rs`), where `AddOwnerSlot` re-checks seat liveness, readonly and range, and `SetState` evaluates its IR value against live slots. `grantHealth` and `applyDamage` apply at dispatch through the reaction registry. So in `[grantLayer(shield, 25), damage(20)]` a queued grant would land after the damage, and a grant applied at dispatch would read capacity before an earlier `addSlot` lands.

The brief moves the host's store writes to dispatch, so every step lands in body order. Why it holds:
- **Frame order.** `frame_loop` runs the first drain after the post-tick named-event dispatch, then the HUD publisher, then the crossing stage, then a second drain, then connected-client per-owner persistence. A write applied at dispatch lands no later than the drain that would have applied it, so crossing evaluation and persistence still see every write.
- **`fire` targets.** `dispatch_sequence` collects `fire` names and the fired reactions run after the firing body, so pin S27 in `plans/done/reaction-body-composition/research.md` (`b` reads 1, `a` ends 2) holds with writes at dispatch.
- **Checks move with the write.** Seat liveness, readonly and range run at dispatch; a seat released earlier in the frame warns and is skipped, where the drain skips it silently today. The client gate `owner_slot_writes_enabled` is unchanged.
- **Wire.** `presentation_commands.rs` keeps `AddOwnerSlot` and `SetState` off the wire today, so no presentation routing changes.

`grantLayer` then applies at dispatch beside `grantHealth` on every path. Impact policies already apply owner-slot writes in effect order; a grant there evaluates capacity when its effect applies, not against the plan snapshot.

## Pinned orderings

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| R-P1 | Shield 30, health 100. One host tick carries a crush for 20 and a host hitscan for 20. A host-validated client hit for 20 is flushed after that tick. | Producers drain in tick stage order, crush before weapons. A flushed client hit lands after the tick's death sweep and before the next tick's recharge. | Crush leaves the shield at 10 and reports no broke. The hitscan takes 10 from the shield and 10 from health, and only it reports broke. The flushed hit takes 20 from health, with absorbed 0 and no broke. |
| R-P2 | Delay 3000 ms at 60 Hz. Three entities at shield 0, each hit once: one in tick N's weapon stage, one by a client hit flushed after tick N, one by script damage at frame end after tick N, once the host tick counter has advanced. | Every hit that lands between tick N's recharge and tick N+1's recharge stamps tick N, whatever its producer and whatever the counter reads. | All three first recharge at the start of tick N+180. With delay 0, all three first recharge at tick N+1. Single player, which has no net host tick, gives the same ticks. |
| R-P3 | Delay 3000 ms, rate 60 points/s, 60 Hz, shield 0, hit on tick N. Repeated after 100 000 ticks of uptime. | Expiry is counted in whole ticks from the stamp, never in accumulated float time. | No recharge through tick N+179. Tick N+180's recharge adds exactly 1 point before any hit on that tick drains. The same tick numbers hold after long uptime. |
| R-P4 | Store writes on tick N. One raises a pawn's capacity from 100 to 150, and the pawn respawns at the start of tick N+1. Another lowers an enemy's capacity from 100 to 40 while its shield is 80, and a 30-point hit lands later in tick N. | A respawn evaluates capacity itself and refills to it. Hits use the capacity in force: tick start's evaluation, or a later grant's (R-P18). | The respawned pawn reads 150 / 150 on tick N+1. The enemy's hit on tick N takes the shield from 80 to 50 against capacity 100, and tick N+1 clamps it to 40 / 40. |
| R-P5 | A trigger spawns an enemy with an expression capacity mid-tick, and a hitscan hits it in the same tick's weapon stage. A player pawn with an expression capacity materializes at level load, before the first tick. | Spawn evaluates the layer parameters itself and does not wait for the next tick start. | The enemy is full at spawn, and the hit drains its layer before health. The player reads full on the first tick. |
| R-P6 | A player at shield 120 / 150, whose capacity is read(progression.shieldCapacity).plus(50) over a per-owner slot at 100, changes level, restarts, or reclaims the seat on rejoin. Separately, a seat carries health 0. | Carried layers clamp against the capacity evaluated with the seat's per-owner values in place. A layer restores only when its carried health does. | The carry restore reads 120 / 150, and the first tick after arrival adds rate × dt (R-P13). The seat carrying health 0 arrives with full health and full layers. |
| R-P7 | Player pawn P at shield 0 / 100, broken on tick N−10, delay running. On tick N: a trigger-stage activator grant of 25; an NPC melee for 20 on P in the AI stage; P's hitscan hits an enemy whose impact policy grants `impact.source` 10; frame-end `players().damage(20)` after tick N. Separately, grantLayer(shield, 25) reaches a pawn latched at health 0. | Grants and hits apply in tick order, each seeing the last. A grant never moves the stamp. Only health reads the latch. | Melee takes 25 to 5 with no broke. The policy grant makes it 15. The script hit empties it, reports broke and spills 5 raw to health. The stamp is tick N. The latched pawn's shield reads 25, its latch stays set, and it does not recharge. |
| R-P8 | An entity with layers [armor, shield] is at shield 20 / 100 after a hit on tick N, delay 3000 ms. A hot reload on tick N+60 reorders the list to [shield, armor] and lowers shield capacity to 60. Variants: the reload makes shield capacity an expression that evaluates to 60 and later faults; or one that faults on its first evaluation. | Hot reload keeps values, and each entity's last good parameter values, by layer name, and keeps the entity's last-hit stamp. It re-arms only the warning. | After the reload, the shield reads 20 / 60 and the armor keeps its value. The shield first recharges at tick N+180. The next hit drains the shield before the armor. The first variant keeps 20 / 60 after the fault. The second keeps the pre-reload capacity, reads 20 / 100, and warns once. |
| R-P9 | Layer capacity 0, as a literal or as an expression that evaluates to 0 or below. A hit, a grant of 25 and a capacity rise to 50 follow. | The clamp runs before any drain. A zero layer is skipped. | The layer reads 0 / 0 and is present, not absent. The hit skips it with no broke. The grant leaves it at 0. The overlay shows no shield and counts the entity as full. After the rise it reads 0 / 50 until recharge or a grant. |
| R-P10 | Shield 10, health 5, untyped 30. The same hit on an unlayered entity at health 5 dispatches amount 30 today. | Health's share of points dealt is unfloored, as healthAfter is. | amount 30, absorbed 10, raw 30, healthAfter −15. The contributor ledger records 30. |
| R-P11 | resist: { shock: 0 }, shield 30, health 100, a shock hit for 20. | Restamping and recency follow raw. Credit follows points dealt. | Nothing drains. The delay restarts and brain damage recency resets. The impact dispatches with amount 0, absorbed 0 and no broke. The attacker is not a contributor. |
| R-P12 | An enemy latched at health 0 and not yet despawned, with armor 50 / 100 and a brain. A second attacker hits it for 20 untyped on tick N. | Drain and restamp follow every hit with a positive raw amount. Credit and perception follow target_was_live, which reads health only. | Armor drops to 30. The stamp is tick N. The impact dispatches with amount 20, absorbed 20 and no broke. The ledger, kill credit and brain memory are unchanged, and no second death report fires. |
| R-P13 | A player pawn at shield 120 / 150, rate 60 points/s, delay 3000 ms, 60 Hz, hit on the tick before a level change. On the new level its first tick's weapon stage carries a hitscan for 20 against it. | The last-hit stamp is not carried. The first tick's recharge runs before that tick's hits. | The carry restore reads 120 / 150. The first tick's recharge makes it 121 / 150, the hit drains it to 101, and the next recharge is at the start of the tick 180 ticks after that first tick. |
| R-P14 | Pawn at shield 0 / 100, delay 3000 ms, rate 60 points/s, 60 Hz. Hit on tick N, latched at health 0 on tick N+10, revived in tick N+300's assist stage. Variant: a second hit lands on the latched pawn on tick N+250. | Latched time counts toward the delay. Recharge needs a tick that begins above zero health. A hit on a latched entity restamps. | No recharge through tick N+300. Tick N+301's recharge adds 1 point. In the variant, nothing recharges before tick N+430, and tick N+430's recharge adds 1 point. |
| R-P15 | An entity with layers [shield] at 20 / 100. A hot reload adds armor with capacity read(armorLevel).times(10), with the global armorLevel at 5, and rate 0. | Hot reload evaluates the new parameters itself, as spawn does, before it clamps or fills. | The first tick after the reload reads armor 50 / 50 and shield 20 / 100. |
| R-P16 | A fresh co-op joiner whose seat defaults give shieldCapacity 0, so its pawn materializes at 50 / 50. Its seed, carrying shieldCapacity 100, lands three polls later. Nothing hits the pawn in between. Variant: a hit takes the shield to 30 / 50 one poll before the seed lands. | The spawn fill is redone on the tick the seed lands, with the seeded per-owner values, only for a layer still full at its pre-seed capacity. | The first tick after the seed lands reads 150 / 150. In the variant it reads 30 / 150. |
| R-P17 | An enemy whose capacity expression faults at its spawn evaluation and evaluates to 80 from the next tick on. Variant: the same layer with rate 0. | Spawn fills against the value in force at spawn. A good value arriving later is a capacity raise. | It spawns at 0 / 0 and warns once. The next tick's evaluation makes it 0 / 80, and it fills only by recharge, which starts that tick because nothing has hit it (R-P19), or by a grant. The rate-0 variant stays at 0 / 80. |
| R-P18 | Shield 80 / 100. In one tick: a store write lowers capacity to 40, grantLayer(shield) with no amount, then a 30-point hit. Variant: a full shield at 100 / 100, a store write raising capacity to 125, then grantLayer(shield, 25). | A grant evaluates capacity at the grant, and that evaluation is in force from then on. The value clamps to it, then the grant adds. | The grant leaves 40 / 40. The hit leaves 10 / 40. The next tick reads 10 / 40, its delay restarted by the hit. The variant reads 125 / 125 for the rest of the tick. |
| R-P19 | An enemy with rate 60 points/s, delay 3000 ms, 60 Hz, whose capacity expression faults at spawn and evaluates to 80 from tick N on. Nothing hits it. | At tick start, parameters evaluate and clamp before recharge runs. A layer never hit has no stamp and recharges at once, as a carried layer does. | Tick N's evaluation makes it 0 / 80, and the same tick's recharge makes it 1 / 80. Tick N+79 reads 80 / 80. |
| R-P20 | Shield 0 / 100, delay read(shieldDelay) at 3000 ms, rate 60 points/s, 60 Hz, hit on tick N. On tick N+60 a store write sets shieldDelay to 500. Variant: a hot reload after tick N+60 changes a literal delay from 3000 to 500. | Expiry compares whole ticks since the stamp with the delay in force at each tick start. The engine stores the stamp, never a deadline. | Tick N+61's recharge adds 1 point. The variant gives the same tick. |
| R-P21 | Shield 80 / 100, capacity read(cap). On tick N a store write sets cap to 40 and grantLayer(shield) runs. From tick N+1 the expression faults. | The grant's evaluation becomes the entity's last good value. | Tick N reads 40 / 40 after the grant. Tick N+1 faults, keeps 40, warns once and reads 40 / 40, not 40 / 100. |
| R-P22 | An enemy latched at health 0 with shield 20 / 100, its tag carrying a policy that grants impact.source on broke. A second attacker hits it for 20 untyped on tick N. | Impact facts describe every hit. Credit follows target_was_live. | The shield reads 0. The impact dispatches with amount 20, absorbed 20 and broke true, and the policy runs. The ledger and brain memory are unchanged. |
| R-P23 | A player pawn latched at health 0 with shield 25 / 100 after a grant. A store write on tick N lowers capacity to 10. Variant: the write raises capacity to 150. | Tick-start evaluation and its clamp run for every entity. Only the recharge step skips health ≤ 0. | Tick N+1 reads 10 / 10 and nothing recharges. The variant reads 25 / 150 and stays there while latched. |
| R-P24 | A fresh joiner materializes at 50 / 50. Its seed raises capacity to 150 and lands between ticks. The first tick after it lands carries a hitscan for 20 against the pawn. | The refill runs before that tick's first hit. | The hit drains the shield from 150 to 130. |
| R-P25 | A fresh joiner materializes at 50 / 50. A hit takes it to 30 / 50, and grantLayer(shield) refills it to 50 / 50 before its seed lands; the seed raises capacity to 150. | The seed redoes the fill for any layer at its pre-seed capacity when the seed lands, however it got there. | The first tick after the seed lands reads 150 / 150. |
| R-P26 | A pawn at shield 0 / 100, health 100, delay running. A crossing fires a reaction whose body is players().grantLayer(shield, 25), then players().damage(20). Variant: the reverse body order. | A body's store writes, grants and damage land at dispatch, in body order, on every source. | The grant brings the shield to 25 / 100, and the damage drains it to 5 / 100 with no broke; health stays 100. In the variant the damage takes health to 80 with absorbed 0, and the grant then leaves the shield at 25 / 100. |
| R-P27 | A pawn with capacity read(progression.shieldCapacity).plus(50) at 100, delay 3000 ms, rate 60 points/s, 60 Hz. A lethal hit lands on tick N, and the pawn respawns at the start of tick N+60 at 100 / 100. A store write on tick N+60 raises capacity to 150, and nothing hits it again. | Only a hit sets the stamp. Respawn evaluates capacity itself and refills every layer, and neither sets nor clears the stamp. Latched time counts toward the delay. | Tick N+61 reads 100 / 150. Nothing recharges before tick N+180, and tick N+180's recharge makes it 101 / 150. |
| R-P28 | A fresh joiner whose capacity is read(progression.shieldCapacity), with the seat default 0, materializes at 0 / 0. Its seed carries shieldCapacity 150 and lands between ticks. Rate 60 points/s. | A layer at 0 / 0 is full at its pre-seed capacity, so the seed redoes its fill. | The first tick after the seed lands reads 150 / 150, not 1 / 150. |

## Roadmap updates at promotion

- Mark "damage application order" and "damage types / elemental" as owned by this brief.
- Narrow "shields + shield types" to special effects.
- Reword the pair-scope item toward layer reads.
- Fold "Local-slot crossing origin" into `coop--client-leave-session` and fix its stale reference.
- Add an item for teammate shield bars: a replication decision for owner-private layer values.
- Add an item, "authored defense-layer recharge and overflow policy": a per-layer `overflow` option (gate, scaled), recharge-scope inputs (time since hit, the layer's own value) and a per-layer recharge expression. It carries the IR write path `plans/done/M14--movement-dash-runtime-values` earmarked for shield recharge.
