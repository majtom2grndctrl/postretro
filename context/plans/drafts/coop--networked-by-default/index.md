# Co-op: networked by default

## Goal

Whatever the host owns at runtime reaches every client, a late joiner included. A client can damage, kill, and watch the despawn of any host-owned descriptor entity — target dummies, props, invisible health-only targets — and sees the lights, fog, and emitters a host-only reaction changed. A trigger-fired one-shot (sound, rumble, screen effect) plays on the machines its audience names. Today the host sends an allow-list of entity classes and runs trigger reactions alone, so each class outside the list and each trigger-driven presentation change silently diverges per machine.

## Scope

### In scope

- One exhaustive classifier deciding which descriptor entities the host networks and which placements a connected client leaves to the host, replacing the AI-enemy and world-item predicates; a new descriptor component kind fails to compile until classified.
- Client display copies that carry the descriptor's presentation (mesh, light, emitter) and a hit volume, never authority components.
- The playtest's client-side miss against a map-placed dummy: reproduced against the real model and fixed if it is a defect.
- Replicated state for lights, fog volumes, and emitters changed by a host-only reaction, bound to each peer's local copy, rebased to the receiving clock, visible to late joiners.
- A per-snapshot byte budget on join baselines.
- An author-declared `audience` on trigger-fired presentation one-shots, with per-kind defaults, routed over E16's player-addressed lane.
- An install warning for UI-stack and text verbs a hosted session's trigger can reach.
- Dev content and author docs for the above; re-running E16's carried-forward manual checks.

### Out of scope

- **Forwarding UI-stack and text verbs** (`showDialog`, `openMenu`, `closeDialog`, cell and text edits) to clients. Owner direction: lobby flows, an end-of-level scoreboard, and map voting are coming; this spec keeps the door open (Direction, *Foreclosure*) without building them.
- **Undeclared store slots.** Replication stays the author's `network:` choice.
- **Gravity.** `setGravity` is an imperative script call, not reachable from a trigger.
- **Trigger armed state.** Only the host evaluates triggers; clients see consequences.
- **Spawners spawning non-AI archetypes.** The spawner's install gate stays AI-only, so `RuntimeSpawn` has no light-only or emitter-only producer.
- **Tag mutation.** No primitive exists.
- **The mover binding.** Unchanged; it is the precedent, not a target.
- **Positional trigger sounds.** A trigger has no emitter today and the wire `PresentationCommand` drops `at`. Trigger sounds stay unpositioned.
- **Interest management.** Every networked entity still goes to every participating client.
- **Light `radius` curves.** Engine-internal; rejected by validation already.

## Direction

**Problem.** The host decides membership in replication by naming classes, and a host-only reaction's effect reaches clients only if its target happens to be in one of them — so every class or state outside the list diverges silently, discovered one playtest at a time.

**Prior commitments.**
- `networking.md` §Current contract: replicable-set policy is "gameplay-authoritative first"; deterministic client-local data stays off the wire "unless gameplay authority requires otherwise." This spec applies that rule rather than amending it: a host-only write to a light is gameplay authority requiring otherwise.
- E10 deferred non-AI descriptor props to "their own specs when they become gameplay requirements" (`plans/done/E10--networked-enemy-authority-baseline`). This is that spec.
- Client copies are presentation-only: "`Brain`, `Agent`, `Health`, and `Weapon` components are never attached on the client for a remote enemy" (`networking.md`). Kept, generalized to every display copy. The hit volume is a new non-authority component, so the rule holds.
- Lights, fog, emitters stay client-local spawns (`networking.md`). Kept: they are never host-materialized; only their runtime state is bound and replicated, like movers' phase onto PRL-loaded movers (`mover_network_ids`).
- Trigger-fired presentation "is unchanged and plays on the host" (`networking.md` §Presentation events). **Diverges:** it now plays on its audience. The E16 lane exists so presentation can reach the machine it concerns; the host-only default was a gap, not a choice.
- E16 player events: presentation "reaches the subject only," routing "internal to the fire, not authored," and author-chosen routing was a non-goal there. Kept as the default. **Diverges** narrowly: a player event may set `audience: "everyone"` — a host-decided announcement about one player for the whole session (a "rampage" callout). The shared-slot workaround E16 named needs a counter slot plus a crossing on every client for a one-shot; `"everyone"` is the direct form. No other token is in scope there.
- E18's persistent-atmosphere channel (`plans/done/E18--trigger-event-fanout`, `E18--timed-reaction-steps`; used by `content/dev/scripts/closet-reveal.ts`): a host trigger writes a `network: "shared"` slot and a client crossing fires a client-local light or fog reaction. It already replicates state, survives late join, and stays valid. This spec adds what it cannot: no extra authoring for the common case (a trigger's own light step just works), and a finite animation stays clock-aligned for a late joiner instead of restarting at the joiner's crossing. Docs present direct steps as the default and the channel as the tool when clients need their own logic over the shared value.
- Scope typing: a reaction that reads its fire context (`on.activators`, `on.trigger`, `on.player`, `on.emitter`) is scoped, and TS types plus install binding decide where it is legal (`scripting.md` §12; precedent `playSound(…, { at: on.emitter })`). `audience` reuses that typing instead of a parallel legality table.
- The level content digest holds only deterministic inputs to client prediction. Kept: presentation identities stay out of it; each binding carries its own mismatch check instead (Decision 6).

**Alternatives rejected.**
- *Add classes one at a time* (network health-bearing entities, stop). Fixes the dummy and leaves props, breakables, and every future component to the next playtest. Owner chose the flipped default.
- *Network every light, fog volume, and emitter eagerly.* Stress maps carry ~1,300 lights; every join would pay a baseline per light. Registering on first host-only change costs nothing until a level uses it.
- *Forward the reaction instead of the state.* Smaller, but a lost packet or a late joiner leaves that client's world permanently wrong. Owner chose state.
- *Bind map-placed descriptor entities to the client's local copy by placement ordinal* (no suppression). Avoids join pop-in and keeps local data, but client hit declarations must name a `NetworkId` the host assigned, and a bound local copy would keep authority components (Health) the display-copy rule forbids. Rejected for descriptor entities; adopted for lights, fog, and emitters, which nobody hits and nothing materializes.
- *E18's shared-slot channel as the general answer* (plus an install warning on light/fog/emitter steps reachable only from host-only sources). Already shipped and correct, but every host-only presentation change would need a slot, a crossing per client, and authoring discipline; finite animations restart at each client's crossing. Kept as a tool, not the default (Prior commitments).
- *Presentation verbs on addresses* (`on.activators.flashScreen`). Fits entity addressing but multiplies verbs across every token, group, and both runtimes. Owner chose an `audience` option.
- *String-valued `audience`* (`"activator" | "occupants" | "everyone"`). Readable, but needs its own per-source legality rules beside scope typing, on an author-facing typedef in two runtimes. Token-valued `audience` gets legality from the existing scope machinery.
- *An allow-list of networked components.* "Networked iff Mesh, Health, Touchable, or AI" leaves a new component kind un-networked until someone notices — the fail-open pattern `networking.md` names. Decision 1 classifies every kind exhaustively instead.
- *Two specs* (networked descriptor entities; host-only effects). The halves share only the fire origin. Owner kept one spec: the record of "what the host owns reaches clients" stays whole.

**Placement.** Classification is an engine floor policy in `postretro-sim`'s builtins (the descriptor predicate) with a live twin in `postretro-netcode` (the registry predicate), because sim cannot depend on netcode; one agreement test pins them. Audience resolution splits along the existing seam: the queue (entities) resolves recipients from the fire context; the app drain (postretro + netcode) delivers. Override capture sits at the reaction-dispatch chokepoint, not in each primitive, so a new presentation primitive joins by classification rather than by copying a call.

**Foreclosure and one-way doors.**
- Flipping the default is a one-way door for content: once props network, a mod that relied on client-local props (a client-only decoration a script mutates per machine) changes behavior. Undoing it means re-adding per-class predicates. Mitigation: exempt kinds are named in one table.
- The wire bump (Decision 8) is cheap to repeat; the record-level binding field is the one new shape later work must keep.
- Recipients resolve to a set of seats independent of command class (Invariant I7). A later spec forwards `PushTree` and text edits to an audience by adding wire variants and lifting the install warning — no routing change. Client-originated choices (a map vote) need a client→host lane this spec does not build or block.

## Decisions

1. **Exhaustive classification.** Every `DescriptorComponentKind`, and the `behavior` block, maps to a `ReplicationRole` in one exhaustive `match` with no wildcard arm: `Mesh`, `Health`, `Touchable`, `behavior` → `NetworksEntity`; `Light`, `Emitter` → `ClientLocalPresentation`; `Movement` → `PawnPath`; `Weapon` → `HeldOrPawn`. A new kind fails to compile until classified. An entity is **networked iff** it has `DescriptorProvenance` with spawn path `MapPlacement` or `RuntimeSpawn` **and** currently carries a component whose role is `NetworksEntity` (live `Brain`+`Agent` stand in for `behavior`, which `owned_components` does not track). Entities with only `ClientLocalPresentation` components stay client-local; their state binds per Decision 5. Pawns and projectile presentation keep their own paths. Membership is live: losing every `NetworksEntity` component unregisters the entity (the world-item pickup rule, generalized).
2. **Descriptor twin.** A connected client suppresses a placement iff its descriptor declares a component whose role is `NetworksEntity`, read through the same `match`. The model-upload list for suppressed placements uses the same predicate.
3. **Display copy** = the descriptor's `Mesh`, `Light`, `Emitter`, plus a hit volume when the descriptor authors `health.hitbox`. Never `Health`, `Brain`, `Agent`, `Weapon`, `Touchable`, `PlayerMovement`.
4. **Hit volume** is a client-only, non-damageable component holding the descriptor's `health.hitbox`. The client hit query treats it exactly like `HealthComponent.hitbox` for target selection. No damage path, splash volume, perception, or impact-policy consumer reads it.
5. **Presentation binding.** A light, fog volume, or emitter that is not part of a networked entity binds by `(kind, index, origin)`: `MapLight` → authored map-light index; `FogVolume` → PRL fog record index; `MapEntity` → the PRL map-entity list position, stamped on `MapEntity` at load. A descriptor light or emitter on a networked entity rides that entity's record instead.
6. **Binding check.** The client resolves the index locally and drops the update, with one warning per binding, when the index is out of range, the local kind differs, or the local origin differs from the wire origin by more than 1 cm. It never writes another entity.
7. **Override capture.** The host records a presentation override only when a light, fog, or emitter primitive applies under a **host-only origin** — set per drain call site by where that drain's input is produced (*Drain origins* table), never by source kind. The record is the component value right after the write, plus the host's authoritative tick and `script_time` at the write. Every-machine drains record nothing. A first override registers the bound entity; it stays registered until the level changes.
8. **Wire.** New `ComponentPayload` kinds for light, fog, and emitter state; a record-level optional presentation binding. `WIRE_VERSION` 26 → 27, `SNAPSHOT_VERSION` 17 → 18, `PROTOCOL_ID` unchanged (no new message vocabulary; audience delivery reuses `ServerPresentationPayload::Command`).
9. **Clock rebase.** The client seeds each bound or materialized presentation animation so its age equals the host's: age = (snapshot server tick − recorded tick) × fixed step. A looping light's `phase` shifts by the host's `script_time` offset so host and client sample the same point at the same instant. Finite animations past their duration settle immediately on arrival.
10. **Baseline budget.** Each per-client snapshot carries at most `JOIN_BASELINE_BUDGET_BYTES` of `FullBaseline` records beyond the first; despawn tombstones and deltas for entities the client already holds go first and never wait on the budget. Pending baselines go out in stable `NetworkId` order across snapshots.
11. **Audience.** Presentation builders (`playSound`, `rumble`, `flashScreen`, `screenShake`, `vignette`) take an optional `audience`: a subject token in the reaction's scope — `on.activators` (the edge's player), `on.trigger` (players inside that volume), `on.player` (the event player) — or `"everyone"`. A token makes the reaction scoped, so TS types and install binding reject it under a source that does not publish it, exactly as for any other token read. Audience tokens are the one token kind that survives `wait`: the scheduler's instance origin restores them at landing (`on.trigger` re-reads the volume then). `"everyone"` under an every-machine drain means each machine presents its own run locally — no broadcast. Defaults and resolution are in the *Audience resolution* table. Recipients resolve once, at the queue push, to local and/or a set of seats.
12. **UI verbs.** A `MachineLocal` reaction a trigger binding can reach (directly, after `wait`, or through `fire`/`onComplete`) warns at install in a hosted session, naming the reaction and the binding. Behavior unchanged.

### Exempt and networked kinds

| Entity | Networked | State path |
|---|---|---|
| Descriptor with mesh, health, touchable, or behavior (dummy, prop, breakable, enemy, world item, invisible health-only target) | yes, host-materialized display copy | Transform, `MeshAnimationState`, descriptor light/emitter state on the same record |
| Light-only or emitter-only descriptor placement | no | bound by `MapEntity` ordinal on first host-only change |
| PRL light | no | bound by map-light index on first host-only change |
| Fog volume | no | bound by fog record index on first host-only change |
| Map-placed `billboard_emitter` | no | bound by `MapEntity` ordinal on first host-only change |
| Pawn, projectile presentation, mover | own paths | unchanged |
| Held wieldable | no | unchanged |

### Drain origins

Origin is set where each drain is called (`frame_loop/mod.rs`), from where that drain's input is produced. Verified against the client branch, which fills only its own predicted movement events and its own weapon events.

| Drain | Input produced | Origin |
|---|---|---|
| Trigger residuals (`drain_frame_residuals`) | host authoritative tick | host-only: edge player, trigger, occupants at the edge's tick |
| Player-event residuals (`drain_frame_residuals`) | host | host-only: event player |
| `wait` landings (`drain_landings`) | host scheduler | the originating fire's origin, restored from the instance key |
| AI events (`pending_ai_events`) | host tick only | host-only, no subject |
| Death events (`pending_death_events`, both sweeps) | host tick and host death sweeps | host-only, no subject |
| Mover events (`pending_mover_events`) | host tick only | host-only, no subject |
| `fire` / `onComplete` follow-ups | the dispatching fire | host-only, no subject, when the dispatching fire was host-only; else every-machine |
| Movement events | each machine's own simulation (host: every pawn; client: its own prediction) | every-machine |
| Weapon events, including the host's remote-impact events | each machine's own fire or observation | every-machine |
| levelLoad, crossings | every machine | every-machine |

### Audience resolution

| Origin | Default `playSound` | Default rumble / screen effects | `on.activators` | `on.trigger` | `on.player` | `"everyone"` |
|---|---|---|---|---|---|---|
| Trigger residual, or its `wait` landing | everyone | activator | the edge's player | players inside the volume (edge's tick; at landing after `wait`) | out of scope | host + every participating client |
| Player event, or its `wait` landing | event player | event player | out of scope | out of scope | event player | host + every participating client |
| Host-only, no subject | everyone | host-local | out of scope | out of scope | out of scope | host + every participating client |
| Every-machine | local | local | out of scope | out of scope | out of scope | local |
| Single player, any origin | local | local | local | local | local | local |

"Out of scope" means scope typing rejects the token there (Decision 11). A player with no bound seat (held, disconnected) presents nowhere. `"everyone"` never presents twice on one machine and never reaches a held or demoted client.

## Acceptance criteria

Automated:

- [ ] 1. On a connected client, a hitscan hit on a map-placed `target_dummy` is declared, accepted, and applied on the host; the kill credits the client's player (`byPlayer(impact.source)` XP lands on the client's seat); the host despawn removes the client's copy by the next snapshot.
- [ ] 2. For each descriptor shape — AI enemy, world item, health-only with mesh, mesh-only prop, health-only without mesh, light-only, emitter-only, light+mesh, emitter+health — host registration and client suppression agree, and the outcome matches the *Exempt and networked kinds* table: light-only and emitter-only stay client-local; every other shape registers. The role classification is one exhaustive `match` with no wildcard arm, so adding a `DescriptorComponentKind` variant fails to build until it is classified.
- [ ] 3. After join, a connected client holds exactly one entity per placement: no networked placement has both a local copy and a host-built copy, and no networked placement is missing its display copy.
- [ ] 4. Membership follows live components: a picked-up item unregisters and its client copy despawns; dropping it re-registers it; a networked prop's `setAnimationState` from a host-only reaction reaches every client.
- [ ] 5. A display copy carries the descriptor's mesh, light, emitter, and (when authored) hit volume, and none of `Health`, `Brain`, `Agent`, `Weapon`, `Touchable`, `PlayerMovement`.
- [ ] 6. A client hit inside the authored `health.hitbox` of a networked health-only target without mesh lands on the host; the same target with no authored hitbox is not hittable; no client-side damage, splash, or impact policy reads the hit volume.
- [ ] 7. A client shot aimed at the torso zone of a map-placed entity using the dev dummy model (`content/dev/models/decraniated_low_poly_retro_pixel/scene.gltf`, real `HitZoneStore` entry) resolves as an entity hit — both for a locally spawned entity and for a host-built display copy.
- [ ] 8. A trigger-fired `setLightAnimation`, each `setFog*` (Density, Glow, EdgeSoftness, Falloff, Params, Animation), `setEmitterRate`, and `setSpinRate` (rate and animation forms) on a PRL light, a fog volume, a map-placed emitter, a light-only descriptor placement, and a networked entity's descriptor light/emitter shows the same component values on every connected client and on a client that joins afterward.
- [ ] 9. A finite light or fog animation started N seconds before a client joins finishes on that client at the host's finish time (within one snapshot interval) and settles to the host's final values; a looping light animation's sampled phase on host and client agrees within one snapshot interval at a shared instant.
- [ ] 10. A light, fog, or emitter change from an every-machine drain (levelLoad, a crossing, a movement or weapon event) records nothing on the host and reaches no client: a host-player low-health crossing that dims lights leaves a client's lights unchanged. A change from a host-only drain with no subject records and reaches every client: an enemy-death reaction's `setLightAnimation` shows on every connected client.
- [ ] 11. A bound update whose index is out of range, whose local kind differs, or whose local origin differs by more than 1 cm warns once and changes no entity; a matching update applies.
- [ ] 12. With no host-only presentation change, a join sends zero light, fog, or emitter records; an override written twice in one tick sends only the final value; an acked, unchanged override sends nothing on later snapshots.
- [ ] 13. With every light, emitter, and fog volume on `stress-env-volumes-baseline` changed by one trigger, no per-client snapshot carries more than `JOIN_BASELINE_BUDGET_BYTES` of baselines beyond its first record; a joining client holds every baseline within ⌈total baseline bytes ÷ budget⌉ + 2 snapshots; a despawn issued during the backlog arrives in the next snapshot.
- [ ] 14. Trigger defaults: `playSound` presents on the host and every participating client exactly once each; `rumble`, `flashScreen`, `screenShake`, `vignette` present only on the activator's machine (the host's when the host's player activated). Host-only drains with no subject (AI, death, mover events, follow-ups): `playSound` presents everywhere; the other four present on the host only; an enemy-death reaction's `flashScreen` with `audience: "everyone"` presents on every machine once.
- [ ] 15. `audience` tokens on a trigger: `on.trigger` reaches exactly the players inside the volume at the edge's tick; `on.activators` reaches only the edge's player; `"everyone"` reaches every machine once — for every presentation kind. After a `wait`, `on.activators` resolves to the same origin player and `on.trigger` to the volume's players at landing.
- [ ] 16. Player events keep E16 behavior with no `audience`; `audience: "everyone"` in a player event presents on every machine once; `on.activators` or `on.trigger` as a player event's audience is rejected at install (and by TS types), naming the reaction.
- [ ] 17. An `audience` token under a source that does not publish it is rejected by scope typing in TypeScript and at install in both runtimes, naming the reaction and source; `"everyone"` under an every-machine drain presents locally on each machine, once; `audience` on a non-presentation builder or an unknown value is rejected at install — identically in TypeScript and Luau.
- [ ] 18. A held or demoted client receives no presentation command; single player presents every command locally regardless of `audience`.
- [ ] 19. A `showDialog`, `openMenu`, `closeDialog`, cell, or text verb reachable from a trigger binding warns at install in a hosted session, naming the reaction and the binding, and still runs on the host only; single player and connected clients log nothing.
- [ ] 20. A peer on `WIRE_VERSION` 26 is refused at the transport gate; `WIRE_VERSION` is 27 and `SNAPSHOT_VERSION` is 18, pinned by tests.
- [ ] 21. `docs/scripting-reference.md` documents `audience`, its defaults, what reaches clients in co-op (networked kinds, host-only presentation changes), and the UI-verb limitation; the dev `player-events.ts` example includes a `screenShake` in a player event's fire list.

Manual (owner, Mac↔Windows on `content/dev/maps/splash-damage-demo.map` and the co-op test fixture from Task 11):

- [ ] M1. Client pistol kills on dummies credit XP to the client; a dummy either player kills disappears on both machines; client impact bursts land on the dummy.
- [ ] M2. A trigger dims lights, changes fog, and stops an emitter for both players, including a client that joins after the trigger; a looping light pulses in visible sync on both screens.
- [ ] M3. A pressure-plate flash plays only for the player who stepped on it; its alarm sound plays for both.
- [ ] M4. E16 M1 (level-up and scald on the affected machine only; vignette local, no delay) and E16 M2 (flash limiter and reduce motion apply to a forwarded flash and shake on a client).

## Tasks

### Task 1: Split-first

Behavior-preserving splits of files this plan extends past ~800 lines. No logic changes; existing tests pass unchanged; each split is its own commit-sized unit and the five are independent.
- `crates/netcode/src/lib.rs` (7.5k lines): move `client_receive_and_apply`'s remote-entity materialization loop (the class dispatch to `materialize_armed_remote_projectile` / `materialize_armed_remote_player` / `materialize_armed_remote_enemy` and the follow-on animation and weapon-attachment steps) into a `snapshot_apply` module; `client_receive_and_apply` calls it.
- `crates/netcode/src/client.rs` (5.5k): move mover binding (`mover_network_ids`, `find_loaded_mover_entity`, `validate_kinematic_mover_state_binding`, `prepare_mover_apply`'s binding half) into a `client/bindings` submodule that Task 10 extends.
- `crates/net/src/wire.rs` (3.6k): move `ComponentPayload`, `RawComponentPayload`, the `Wire*` component structs, `COMPONENT_KIND_*`, and their validate/convert arms into a `wire/components` submodule, re-exported at the old paths.
- `crates/net/src/replication.rs` (2k): move `ClientReplicationState` and `encode_for_client_with_sequence` into a `replication/encode` submodule that Task 6 extends.
- `crates/sim/src/scripting/builtins/data_archetype.rs` (3.6k): move `descriptor_carries_brain`, `descriptor_materializes_ai_enemy`, `descriptor_materializes_world_item`, `filter_out_client_host_replicated_placements`, `suppressed_client_host_replicated_mesh_models`, `ai_capsule_center_from_feet_offset`, and their tests into a `host_replicated` module, re-exported from `builtins` as today.

Light touches below do not trigger a split: `light_bridge.rs`, `fog_volume_bridge.rs`, `hit_zones.rs`, `system_reactions.rs`, `trigger_system.rs`, `presentation.rs`, `endpoint.rs`.

### Task 2: Thin slice — networked target dummy

Falsifies the classifier and display-copy assumptions end to end before fan-out. Build Decisions 1–2 and the class path:
- **Roles.** In `postretro-entities` beside `DescriptorComponentKind` (`provenance.rs`): `enum ReplicationRole { NetworksEntity, ClientLocalPresentation, PawnPath, HeldOrPawn }` and `DescriptorComponentKind::replication_role(self)`, an exhaustive `match` with no wildcard arm per Decision 1; `behavior` has its own role constant beside it. Both predicates below read roles only.
- **Descriptor predicate** in `builtins::host_replicated`: `descriptor_is_host_replicated(&EntityTypeDescriptor) -> bool` = any declared `DescriptorComponentKind` whose role is `NetworksEntity`, or `behavior` declared. Replace both predicate calls inside `filter_out_client_host_replicated_placements` and `suppressed_client_host_replicated_mesh_models` with it. Delete `descriptor_materializes_world_item`; keep `descriptor_materializes_ai_enemy` for the spawner resolver in `startup/lifecycle.rs`, which stays AI-only. Rename the `suppress_ai_enemies` field and parameters to `suppress_host_replicated` at every site (`lifecycle.rs`, `lifecycle_world_cpu.rs`, test fixtures in `observability/driver.rs`, `lifecycle_progress_install_tests.rs`).
- **Live predicate** in netcode `descriptor_class.rs`: `is_host_replicated_descriptor_entity(registry, id) -> bool` per Decision 1 — provenance spawn path, then any live component whose `DescriptorComponentKind` role is `NetworksEntity` (via `DescriptorComponentKind::component_kind`), or live `Brain`+`Agent`. `descriptor_entity_class`'s third branch uses it, replacing `is_networked_ai_enemy || Touchable`.
- **One sweep.** Replace `host_register_map_enemies` and `host_register_world_items` with `host_register_descriptor_entities` (same signature; stale rule = `!is_host_replicated_descriptor_entity`; iterate entities carrying `DescriptorProvenance`). Merge `NetEndpoint::Host::{map_enemies, world_items}` into one `descriptor_entities` set, cleared where both are cleared today; update the two `NetEndpoint::Host` destructures in `main.rs`. Collapse the `App` wrappers into `host_register_descriptor_entities_after_install` / `_after_fixed_sim_tick` at the existing call sites (install path in `startup/lifecycle.rs`, per-tick sweep in `frame_loop/mod.rs`).
- **Agreement test** replacing `classifier_agrees_with_live_predicate_one_source_of_truth`: spawn every shape listed in AC 2 through `apply_data_archetype_dispatch` and assert `descriptor_is_host_replicated(descriptor) == is_host_replicated_descriptor_entity(&reg, id)` and the expected value. Invert existing assertions that pinned props as unregistered (`enemy_replication_harness_test.rs`, `replication.rs` unit tests, `data_archetype` filter tests) — they encode the old policy.
- **Harness**: extend `ingest_hit_harness_test` with a `target_dummy`-shaped placement: host registers it, client suppresses and materializes a mesh copy, a client hitscan `LocalHitRecord` on it becomes a declared record, `apply_valid_hit_record` applies it, the impact policy credits the client's seat, and the host despawn tombstone removes the client copy.
Proves AC 1, 2, 3, and the membership half of AC 4.

### Task 3: Client hit against the real dummy model

Reproduce the playtest miss (research.md, *The impact-burst observation*). Build a registry with a `target_dummy` descriptor entity via `attach_descriptor_components`, install the real glTF with `HitZoneStore::insert_from_load` against the `content/dev` root, fire `resolve_test_client_shot` at the torso zone with the dev pistol, and assert `resolution.hits` names the entity and `impact_contacts()` carries `Some(target)`. Repeat with a mesh-only display copy built by `materialize_net_mesh_presentation`. If either fails, trace to cause — handle-key mismatch between `mesh.model` and the sweep's `ModelHandle`, broad-phase reject from `origin_offset` or transform, pose availability, or aim ray — and fix it in the owning module; record the cause in the task's commit message. If both pass, add a test pinning the playtest's actual aim (client eye height, dummy at the map's `splash_direct` origin) before closing. Proves AC 7.

### Task 4: Display copy presentation set and hit volume

Extend `materialize_net_mesh_presentation` (sim `net_descriptor.rs`) into the display-copy builder for Decision 3: after the mesh, attach the descriptor's `LightComponent` (through the same conversion `attach_descriptor_components` uses, so `LightBridge::absorb_dynamic_lights` enrolls it) and `BillboardEmitterComponent`, and a `HitVolumeComponent { hitbox: Hitbox }` (new, in `entities/src/components/`, new `ComponentKind`) when `descriptor.health.hitbox` is set, using `HealthComponent::from_descriptor`'s `Hitbox` conversion with `ai_capsule_center_from_feet_offset` applied as today. A descriptor with no mesh no longer returns false: it materializes whatever presentation it has; transform-only stays the result when it has none. In `hit_zones.rs`, add `entity_hitbox(registry, id) -> Option<&Hitbox>` reading `HealthComponent.hitbox` then `HitVolumeComponent.hitbox`; use it at the hit query's hitbox lookup only — `damageable_volume`, splash, impact-policy anchor, and AI perception keep reading `HealthComponent` (Decision 4). `for_each_hittable_candidate` also visits `HitVolumeComponent` entities without Health or Mesh; check every caller's own liveness gate so none treats them as damageable. The client overlay anchor (`client_overlay_hitbox`) already reads the descriptor; leave it. Proves AC 5, 6.

### Task 5: Presentation state on the wire

In `wire/components` (Task 1), add three payload kinds with `COMPONENT_KIND_*` equal to the engine `ComponentKind` discriminants for `Light`, `FogVolume`, `BillboardEmitter`, in engine numeric order (`component_kind_pinned_to_engine_discriminants` extends):
- `WireLightState { intensity, color, cone_direction, animation: Option<WireLightAnimation>, recorded_tick: u32, recorded_script_time: f64 }`.
- `WireFogState { density, glow, edge_softness, falloff, tint, saturation, min_brightness, light_range, animation: Option<WireFogAnimation>, recorded_tick: u32, recorded_script_time: f64 }`.
- `WireEmitterState { rate, spin_rate, spin_animation: Option<WireSpinAnimation>, recorded_tick: u32 }`.
Add `presentation_binding: Option<WirePresentationBinding { kind: u8, index: u32, origin: [f32; 3] }>` to `FullBaseline` and `Delta`, beside `projectile_presentation`. Validation: every float finite; curve vectors at most `MAX_WIRE_CURVE_KEYS` (64) entries; `period_ms > 0`; `kind` one of the *Boundary inventory* values; a record with a binding carries no `entity_class` and no Transform, and one of the three state payloads; a record without a binding may carry state payloads only beside a Transform. Bump `WIRE_VERSION` to 27 and `SNAPSHOT_VERSION` to 18; update the version-pin tests in `handshake.rs` and `wire.rs`; add a gate test refusing 26. Round-trip tests per payload, including `None` animations and maximum curve length. Proves AC 20 and the wire half of AC 8.

### Task 6: Join baseline budget

In `replication/encode` (Task 1), make `encode_for_client_with_sequence` budgeted per Decision 10. Order: despawn tombstones, then deltas for entities the client has acked a baseline of, then pending `FullBaseline`s in ascending `NetworkId`, admitting baselines while the encoded size of admitted baselines stays within `JOIN_BASELINE_BUDGET_BYTES` (const in `crates/net`, initial value 16 KiB; the first pending baseline always goes so one oversized entity cannot stall). Measure with the same encoder `host_replicate` uses. An entity whose baseline is deferred gets its newest state when admitted; an entity despawned while pending gets only its tombstone. Add a `debug`-level log in `host_replicate` reporting per-client snapshot bytes and deferred-baseline count when any baseline is deferred. Unit tests over `ServerReplication` with synthetic payload sizes pin ordering, the first-record exception, the despawn-during-backlog case, and convergence. Proves the budget half of AC 13.

### Task 7: Stable presentation identity

Give each bindable kind a resolver usable on both peers (Decision 5):
- Stamp `map_ordinal: u32` on `MapEntity` (sim `map_entity.rs`) from its position in the PRL map-entity list at load; carry it through `apply_classname_dispatch` and `spawn_descriptor_instance` into a new `MapOrdinal(u32)` component on the spawned entity.
- Make `LightBridge::entity_for_map_index` live (drop `#[allow(dead_code)]`) and add `LightBridge::map_index_for_entity`; add the same pair on `FogVolumeBridge` over `entity_ids`.
- Define `PresentationBindingKey { kind: PresentationBindingKind, index: u32 }` with `PresentationBindingKind::{MapLight, FogVolume, MapEntity}` in `postretro-entities` (both netcode and the app can name it), and a resolver trait the app implements over the two bridges and a registry `MapOrdinal` scan, returning `(EntityId, origin)`.
- Record that a `try_spawn` failure during population shifts no later index: populate keeps the index of every record it attempts, storing `None` for a failed slot.
Unit tests: resolution round-trips per kind; a failed spawn leaves later indices intact. Feeds Tasks 9 and 10.

### Task 8: Audience

Build Decisions 11–12 and the *Drain origins* and *Audience resolution* tables.
- **Fire origin.** Replace `SystemCommandFireContext.presentation_seat` with `origin: FireOrigin` — `EveryMachine`, or `HostOnly { player: Option<Seat>, trigger: Option<EntityId>, occupants: Vec<Seat> }` (all empty for a host-only drain with no subject). Set it at each drain call site in `frame_loop/mod.rs` per the *Drain origins* table: `drain_frame_residuals` sets trigger origins (the edge's player through `seat_for_pawn` for `PlayerId::Local`; for `PlayerId::Remote`, a client→seat lookup the app passes in from its `SeatTable`; occupants captured at the edge's tick — extend the residual tuple and add `TriggerSystem::occupant_players(trigger)`) and keeps player-event seats; the AI, death, and mover `drain_named_events_with_sequences` calls pass `HostOnly` with no subject; the movement and weapon calls pass `EveryMachine`. `drain_landings` restores the origin from the scheduler instance key, re-reading occupants at landing. Follow-ups carry their dispatching fire's origin: `pending_trigger_follow_ups` entries gain the origin, and `dispatch_deferred_named_events_with_sequences` installs it (subject fields cleared, since follow-ups are context-free).
- **Recipients.** `SystemCommandQueue::push` resolves `Presentation`-class commands to `Recipients::{Local, Seats(Vec<Seat>), Everyone}` from the command's audience and the origin; `routed` holds `(Recipients, command)`. `route_player_presentation` delivers: `Everyone` → local once plus one `ServerPresentationPayload::Command` per participating client; `Seats` → local for the host's seat, one send per bound remote seat, drop for unbound. Under `EveryMachine`, every audience resolves `Local`. No other class changes behavior (Invariant I7).
- **Authoring.** Add `audience?` to the five builders in `sdk/lib/ui/reactions.{ts,luau}`: the subject-token types `ActivatorsTarget`, `TriggerTarget`, `PlayerTarget` (`sdk/lib/data_script/commands.ts` and Luau twin), or `"everyone"`. A token-valued audience makes the builder's result scoped, so the reaction must take `on` and its source must publish that token — reuse the existing scoped-reaction typing; no new legality table. Update typedef templates, generated `sdk/types`, and the build-time mirror in `script-compiler/src/light_membership.rs`. The descriptor carries `audience: "@activators" | "@trigger" | "@player" | "everyone"`; carry it as a field on the five `SystemReactionCommand` variants and the system-reaction args structs. Partition (`trigger_bindings/partition.rs`) admits these sentinel values on presentation residual steps — today it drops sentinel-targeted presentation — and `wait` admits audience tokens past the wait (the one carve-out from "tokens do not survive `wait`"). Install validation rejects an unknown value or `audience` on any other builder; player-event validation (`player_events/validate.rs`) rejects `@activators` and `@trigger`.
- **UI-verb warning** (Decision 12): at trigger-binding install in a hosted session, walk the binding's reactions (direct, post-`wait`, follow-ups — reuse the transitive `fire`/`onComplete` walker from player-event validation) and warn once per (binding, reaction) reaching a `MachineLocal` kind.
Tests per AC row through `SystemCommandQueue` and `route_player_presentation` with a fake server; a test that a connected client's frame never fills the AI, death, or mover queues (pins the *Drain origins* table); TS type tests in `sdk/type-tests/` and TS/Luau install-parity fixtures. Proves AC 14–19 and the drain half of AC 10.

### Task 9: Host override capture and registration

Build Decision 7 on the host.
- **Chokepoint.** In scripting-core reaction dispatch (`primitive_dispatch.rs` for tag-targeted fog and emitter primitives; the sequence-step dispatch that runs `setLightAnimation` handlers), after a successful apply of a primitive classified as replicated presentation (`setLightAnimation`, `setFogDensity`, `setFogGlow`, `setFogEdgeSoftness`, `setFogFalloff`, `setFogParams`, `setFogAnimation`, `setEmitterRate`, `setSpinRate` — one `const` list, with a test asserting it matches the registered light/fog/emitter primitives), and only when `ScriptCtx`'s current fire origin is `HostOnly`, write `PresentationOverride { component snapshot, recorded_tick, recorded_script_time }` for each target into a `PresentationOverrides` table on `ScriptCtx` (last write wins per entity). `ScriptCtx` already reaches both dispatch paths (`primitive_dispatch` and `dispatch_sequence` each take `script_ctx: &ScriptCtx`); the app sets `recorded_tick` and `recorded_script_time` on `ScriptCtx` before each drain.
- **Registration.** A host sweep `host_register_presentation_overrides` (netcode `replication.rs`, beside `host_register_descriptor_entities`, called after the frame-end drain on the host) stamps a `NetworkId` and registers each overridden entity not already replicable, recording its `PresentationBindingKey` from Task 7's resolver. Overrides on a networked descriptor entity need no binding. The table and registrations clear at level install.
- **Production.** `collect_payloads` emits the matching state payload from the override record (never from the live component, which fog and finite lights rewrite each frame), and the snapshot producer sets `presentation_binding` for bound entities. A networked descriptor entity with a descriptor light or emitter and no override emits its state payload from the descriptor's authored values with `recorded_tick` = install tick, so a display copy matches without an override.
Tests: every-machine drains record nothing and a host-only drain with no subject records (AC 10); double write in one tick yields one final record (AC 12); zero records with no override; fog override captures the authored write, not a later sample. Proves AC 10, 12 and the host half of AC 8.

### Task 10: Client binding and clock rebase

Build Decisions 6 and 9 in `client/bindings` (Task 1).
- On a `FullBaseline` with `presentation_binding`, resolve the local entity via Task 7's resolver; apply Decision 6's checks; on success map `NetworkId` → `EntityId` (no materialization, no Transform write) and remember the binding; on failure warn once per `(kind, index)` and ignore further records for that `NetworkId`. Clear bindings where `mover_network_ids` clears.
- Apply each state payload by writing the component, then rebasing: age = (snapshot server tick − `recorded_tick`) × fixed step. Lights: finite → seed `LightBridge`'s `animation_start_time` to local `script_time − age` (add a seeding entry point); looping → set `phase` to `phase + (recorded_script_time + age − local script_time) / period` (mod 1). Fog: seed `FogVolumeBridge`'s anim slot start to local `script_time − age`. Emitter: seed `EmitterBridgeState.spin_elapsed` to `age`. A finite animation whose age exceeds its duration settles on the next bridge update.
- Unbound records carrying state (descriptor display copies) apply the same way onto the materialized copy.
- A despawn tombstone for a bound `NetworkId` removes the mapping only; the local entity stays.
Tests: binding mismatch cases (AC 11); late-join rebase for finite and looping animations against a synthetic host clock (AC 9); bound despawn leaves the entity. Proves AC 9, 11 and the client half of AC 8.

### Task 11: Content, docs, end-to-end proof

- **Fixture.** Add `content/dev/maps/coop-presentation-test.map` with a pressure-plate trigger whose reaction plays an alarm (`playSound`, default audience), flashes (default), and shakes with `audience: on.trigger`; an enemy whose death reaction runs `setLightAnimation` and a `flashScreen` with `audience: "everyone"`; a second trigger that animates a PRL light, sets fog density and animation, and stops a map-placed emitter and a light-only descriptor placement; target dummies; a mesh-only prop with an animation state; an invisible health-only target with an authored hitbox. Add `screenShake` to a player event in `content/dev/scripts/player-events.ts` and its Luau twin.
- **Docs.** `docs/scripting-reference.md`: `audience` on each presentation builder, the defaults table in author terms, a *Co-op* subsection listing what reaches clients (networked kinds, host-only presentation changes and their late-join behavior) and the UI-verb limitation.
- **End-to-end harness** (netcode, extending the conditioned-link harness): host + two clients, one joining after both triggers fire; assert AC 8 and 9 on the late joiner, AC 14–15 per client, AC 1 over the conditioned link, and AC 4's animation half (a host-only `setAnimationState` on the fixture's mesh-only prop reaches both clients).
- **Resource bound** (AC 13, `testing_guide.md` §Resource bounds): fixture `stress-env-volumes-baseline` with a test-only trigger reaction overriding every light, emitter, and fog volume; inputs one joining client; metric per-snapshot baseline bytes and snapshots-to-converge from the Task 6 log; machine class any (byte counts are platform-independent); upstream limits named — renet fragmentation and `available_bytes_per_tick`. Record the measured totals in the test.
Proves AC 13, 21 and drives M1–M4.

## Sequencing

**Phase 0 (concurrent):** Task 1 — five independent splits.
**Phase 1 (concurrent):** Task 2 — thin slice, falsifies the classifier and display-copy assumptions across host registration, wire class, client suppression, materialization, and host HIT ingestion; Task 3 — independent of Task 2's files.
**Phase 2 (concurrent):** Task 4, Task 5, Task 6, Task 7, Task 8 — disjoint files (sim `net_descriptor.rs`/`hit_zones.rs`/entities components; net `wire/components`; net `replication/encode`; sim `map_entity.rs` + bridges; entities `system_commands.rs`/sim `residual_drain.rs`/netcode `presentation_commands.rs`/SDK).
**Phase 3 (concurrent):** Task 9, Task 10 — both consume Task 5's payloads and Task 7's identity; Task 9 consumes Task 8's fire origin.
**Phase 4 (sequential):** Task 11 — consumes everything.

## Rough sketch

- Classifier pair lives beside the existing pair it replaces; the agreement test stays in netcode because it needs both crates.
- `FireOrigin` replaces a field E16 added; E16's player-event path sets `HostOnly { player: Some(seat), .. }` where it set `presentation_seat: Some(seat)`.
- `PresentationOverrides` is a host-side `HashMap<EntityId, PresentationOverride>` on `ScriptCtx`; clients and single player never read it. Single player still writes it harmlessly; the registration sweep runs only on a host.
- The looping-light rebase works in host `script_time`: the host sampled `t_h / period + phase` from the moment of the write, so the client chooses its phase so `t_c / period + phase'` equals the host's sample at the same instant.

## Boundary inventory

| Name | Rust | Wire | TS | Luau |
|---|---|---|---|---|
| audience values | `Audience::{Activators, Trigger, Player, Everyone}` | n/a (resolved host-side) | `on.activators \| on.trigger \| on.player \| "everyone"` | `on.activators \| on.trigger \| on.player \| "everyone"` |
| audience in descriptor JSON | serde `"@activators" \| "@trigger" \| "@player" \| "everyone"` | n/a | emitted by the builder | emitted by the builder |
| audience field | `audience: Option<Audience>` on the five `SystemReactionCommand` variants | n/a | `audience?:` in the builder options | `audience =` in the builder options |
| binding kind | `PresentationBindingKind::{MapLight, FogVolume, MapEntity}` | `kind: u8` = 0, 1, 2 | n/a | n/a |
| light state | `ComponentPayload::LightState` | `COMPONENT_KIND_LIGHT` = `ComponentKind::Light as u16` | n/a | n/a |
| fog state | `ComponentPayload::FogState` | `COMPONENT_KIND_FOG_VOLUME` = `ComponentKind::FogVolume as u16` | n/a | n/a |
| emitter state | `ComponentPayload::EmitterState` | `COMPONENT_KIND_BILLBOARD_EMITTER` = `ComponentKind::BillboardEmitter as u16` | n/a | n/a |

Builders whose positional signature has no options object today (`rumble`, `flashScreen`, `vignette`, `screenShake`) gain a trailing options object `{ audience? }`; `playSound`'s existing options object gains the field.

## Wire format

bitcode, pinned; layout follows the existing `RawComponentPayload` pattern — one new `Option` slot per payload kind, appended after `kinematic_mover`, and one `Option<WirePresentationBinding>` record field appended after `projectile_presentation` on `RawEntityRecord`. Field order within each struct is as listed in Task 5. `Option<Vec<f32>>` curves encode as bitcode options of vectors; empty curve vectors are invalid (validation rejects, as the engine does). `None` animation means "no animation." Mirrors the `WireKinematicMoverState` precedent for a state payload carrying its own identity check.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1. One decision per descriptor shape: host registers iff client suppresses | Task 2 | any new placement filter or registration sweep; a new `DescriptorComponentKind` (caught at compile time by the exhaustive role `match`) | AC 2, 3 |
| I2. Display copies carry no authority component | Task 2, Task 4 | display-copy builder; any new descriptor component | AC 5, 6 |
| I3. Host records presentation overrides only under a host-only origin, set per drain call site | Task 8 (origin), Task 9 (capture) | a new drain call site; follow-up dispatch; every-machine drains sharing a `ScriptCtx` | AC 10 |
| I4. A state payload is constant between host-only writes | Task 5, Task 9 | fog and light bridges rewriting components; any per-tick field (age, sampled value) | AC 12 |
| I5. A bound update writes only the entity its binding names | Task 7, Task 10 | index shift on spawn failure; map mismatch between peers | AC 11 |
| I6. The baseline budget never delays a despawn or a delta | Task 6 | encode ordering; new record kinds | AC 13 |
| I7. Recipients resolve from audience and origin, independent of command class | Task 8 | future forwarded command classes | AC 14–19 |
| I8. Each recipient machine presents a command at most once | Task 8 | `Everyone` local + broadcast; `"everyone"` under an every-machine drain | AC 14, 16, 18 |

## Orderings

| Scenario | Ordering | Expected |
|---|---|---|
| Host change, then join | override before client participates | baseline at participation carries the override; rebased age |
| Join mid finite animation | client binds at age < duration | finishes at host finish time ±1 snapshot interval (AC 9) |
| Join after finite animation ended | age ≥ duration | settles immediately to final values |
| Two host-only writes to one light, one tick | write A, write B | one record with B (AC 12) |
| Host-only write, then every-machine write, same light | host override, client-local crossing | client shows its local write until the next host override; host records only the first |
| Override on a networked entity, then despawn | override, despawn | tombstone; no orphan binding |
| Baseline pending, entity changes | deferred, then changed | admitted baseline carries newest state |
| Baseline pending, entity despawns | deferred, then despawned | tombstone only |
| Trigger edge, activator disconnects before drain | edge at tick N, seat unbound at drain | activator audience presents nowhere; everyone/occupants unaffected for others |
| `wait` lands after activator left | origin seat unbound | activator presents nowhere; occupants re-read at landing |
| Two activators on one tick | two edges | two fires; each activator gets its own command once |
| Item picked up and dropped, one tick | unregister, re-register | fresh `NetworkId`; client despawns old copy, materializes new |
| Client hit on a dummy the host killed that tick | kill applied, HIT arrives | host rejects (terminal removal), as today |
| Level change | new install | host clears overrides and registrations; client clears bindings |

## Script syntax examples

```typescript
import { defineReaction, getMapEntities } from "postretro";
import type { TriggerEventParams } from "postretro";
import { playSound, flashScreen, screenShake } from "postretro/ui";

const alarm = defineReaction("vault.alarm", (on: TriggerEventParams) => [
  playSound("sfx/alarm"),                                // everyone (default)
  flashScreen([1, 0, 0, 0.5], 250),                      // activator (default)
  screenShake(4, 300, { audience: on.trigger }),         // players inside the volume
  playSound("sfx/click", { audience: on.activators }),   // only the stepper
]);

const rampage = defineReaction("streak.rampage", playSound("announcer/rampage", { audience: "everyone" }));
// players().on(becomes(read(streak.kills).ge(5)), [rampage]) — the host decides; every machine hears it

export function setupLevel() {
  return {
    reactions: [alarm],
    triggerEvents: getMapEntities("trigger", { tag: "vault_plate" }).map((t) => t.on("enter", [alarm])),
  };
}
```

```lua
local Postretro = require("postretro")
local UI = require("postretro/ui")

local alarm = Postretro.defineReaction("vault.alarm", function(on)
  return {
    UI.playSound("sfx/alarm"),
    UI.flashScreen({ 1, 0, 0, 0.5 }, 250),
    UI.screenShake(4, 300, { audience = on.trigger }),
  }
end)
```

Lights, fog, and emitters need no new syntax: a trigger-fired `setLightAnimation` or `setFogDensity` now reaches every client.

## Open questions

- `JOIN_BASELINE_BUDGET_BYTES` initial value (16 KiB) is a guess sized under renet's per-tick allowance; Task 11's measurement may move it.
