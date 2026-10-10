# Co-op: networked by default

## Goal

Whatever the host owns at runtime reaches every client, a late joiner included. A client can damage, kill, and watch the despawn of any host-owned descriptor entity — target dummies, props, invisible health-only targets — and sees the lights, fog, and emitters a host-only reaction changed. A trigger-fired one-shot (sound, rumble, screen effect) plays on the machines its audience names. Today the host sends an allow-list of entity classes and runs trigger reactions alone, so each class outside the list and each trigger-driven presentation change silently diverges per machine.

## Scope

### In scope

- One exhaustive classifier deciding which descriptor entities the host networks and which placements a connected client leaves to the host, replacing the AI-enemy and world-item predicates; a new descriptor component kind fails to compile until classified.
- Client display copies that carry the descriptor's presentation (mesh, light, emitter) and a hit volume, never authority components.
- The playtest's client-side miss against a map-placed dummy: reproduced against the real model and fixed if it is a defect.
- Replicated state for lights, fog volumes, emitters, and `prop_mesh` animation states changed by a host-only reaction, bound to each peer's local copy, rebased to the receiving clock, visible to late joiners.
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
- **Positional trigger sounds.** A trigger has no emitter, so trigger-fired sounds stay unpositioned. Host-only sounds played `at: on.emitter` (mover and AI cue events) forward with their position (Decision 8). A death event publishes no emitter, so its `at: on.emitter` sound is skipped, as today.
- **Interest management.** Every networked entity still goes to every participating client.
- **Light `radius` curves.** Engine-internal; rejected by validation already.
- **Placement overrides the state payloads do not carry** on a networked descriptor entity: `initial_range`, `initial_is_dynamic`, and the emitter's spread, lifetime, buoyancy, drag, burst, sprite, color, and velocity. Its display copy uses the descriptor's values for those fields.
- **Host-only despawn of a client-local presentation entity** (light-only or emitter-only placement, map-placed emitter, PRL light, fog volume). No shipped primitive does it: impact-policy `despawn` needs a hittable target, and these are not hittable. The draft brief `coop--bound-presentation-despawn` owns it, including the `remove()` verb no surface offers today and a removal fact a late joiner still receives. A bindable `prop_mesh` differs: a model with hit zones makes it a hit candidate, so an impact policy's `despawn` reaches it (Task 10, Task 11). An impact policy's `playAnim` on it switches the host's animation state outside both `setAnimationState` capture paths; that change stays host-local, as today.

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

1. **Exhaustive classification.** Every `DescriptorComponentKind` maps to a `ReplicationRole` in one exhaustive `match` with no wildcard arm. `behavior`, which is not a `DescriptorComponentKind`, is `NetworksEntity` by rule: `Mesh`, `Health`, `Touchable`, `behavior` → `NetworksEntity`; `Light`, `Emitter` → `ClientLocalPresentation`; `Movement` → `PawnPath`; `Weapon` → `HeldOrPawn`. A new kind fails to compile until classified. An entity is **networked iff** it has `DescriptorProvenance` with spawn path `MapPlacement`, `RuntimeSpawn`, or `DefaultWeapon` (a dropped loadout weapon regains `Touchable` and `Mesh` but keeps its loadout spawn path; held, it carries `Weapon` only) **and** currently carries a component whose role is `NetworksEntity` (live `Brain`+`Agent` stand in for `behavior`, which `owned_components` does not track). Entities with only `ClientLocalPresentation` components stay client-local; their state binds per Decision 5. Pawns and projectile presentation keep their own paths. Membership is live: losing every `NetworksEntity` component unregisters the entity (the world-item pickup rule, generalized).
2. **Descriptor twin.** A connected client suppresses a placement iff its descriptor declares a component whose role is `NetworksEntity`, read through the same `match`. The model-upload list for suppressed placements uses the same predicate.
3. **Display copy** = the descriptor's `Mesh`, `Light`, `Emitter`, plus a hit volume when the descriptor authors `health.hitbox`. Never `Health`, `Brain`, `Agent`, `Weapon`, `Touchable`, `PlayerMovement`.
4. **Hit volume** is a client-only, non-damageable component holding the descriptor's `health.hitbox`. The client hit query treats it exactly like `HealthComponent.hitbox` for target selection. No damage path, splash volume, perception, or impact-policy consumer reads it.
5. **Presentation binding.** A light, fog volume, emitter, or `prop_mesh` that is not part of a networked entity binds by `(kind, index, origin)`: `MapLight` → authored map-light index, origin the light's `origin`; `FogVolume` → PRL fog record index, origin the center of its AABB in `FogVolumeBridge` (the component carries no position); `MapEntity` → the PRL map-entity list position, stamped on `MapEntity` at load, origin `MapEntity.origin`. A descriptor light or emitter on a networked entity rides that entity's record instead.
6. **Binding check.** The client resolves the index locally and drops the update, with one warning per binding, when the index is out of range, the local kind differs, or the local origin differs from the wire origin by more than 1 cm. It never writes another entity.
7. **Override capture.** The host records a presentation override in three cases. The main one: a light, fog, or emitter primitive applies under a **host-only origin** — set per drain call site by where that drain's input is produced (*Drain origins* table), never by source kind. The other two, a `prop_mesh` animation state and a networked placement's KVP overrides, follow below. The record is the component value right after the write, plus `anim_start: Option<f64>`, the host `script_time` at which the host's bridges started the record's animation. A write that changes the animation field (light `animation`, fog `animation`, emitter `spin_animation`) sets `anim_start` to the `script_time` the host's bridges sample this frame — drains run before the frame's `script_time` advance, so the app passes the advanced value. A write that leaves the animation field unchanged keeps the previous override's `anim_start`, or `None` when no override precedes it, because the host's bridges restart an animation only when it changes. The same capture applies to `setAnimationState` on a `prop_mesh` (record: its `MeshAnimationState`, `anim_start: None`), including the in-tick `BoundTriggerCommand::AnimationState` a trigger runs, which is host-only by construction. Every-machine drains record nothing. A networked placement's light or emitter KVP overrides also count as an override, captured at its first registration (Task 10). A first override registers the bound entity; it stays registered until the level changes.
8. **Wire.** New `ComponentPayload` kinds for light, fog, and emitter state; a record-level optional presentation binding; a snapshot-header `server_script_time`; and `at: Option<[f32; 3]>` on the wire `PresentationCommand::PlaySound`, the emitter's world position resolved on the host at push. `WIRE_VERSION` 26 → 27, `SNAPSHOT_VERSION` 17 → 18, `PROTOCOL_ID` unchanged (no new message vocabulary; audience delivery reuses `ServerPresentationPayload::Command`).
9. **Clock rebase.** Light, fog, and emitter animations run on each peer's `script_time`, which advances by unclamped frame time while fixed ticks clamp after a stall. The rebase therefore measures age in host `script_time`, never in ticks. It anchors age to the client's time-sync estimate, not the snapshot's tick, so one-way latency is not lost.

   | Value | Where | Meaning |
   |---|---|---|
   | `anim_start` | each state payload (Decision 7) | host `script_time` when the host's bridges started this animation; `None` = keep the local start |
   | `server_script_time` | snapshot header | host `script_time` at serialization |
   | `host_now` | client, at apply | `server_script_time + (estimated_server_tick − server_tick) × fixed step`; `server_tick` stands in until time sync initializes |
   | age | client, at apply | `host_now − anim_start` |

   A finite light animation, any fog animation, and a spin tween start at local `script_time − age`. A looping light's `phase` shifts by `(host_now − local script_time) / period`, so host and client sample the same point at the same instant. Finite animations past their duration settle on the next bridge update.

10. **Baseline budget.** Each per-client snapshot carries at most `JOIN_BASELINE_BUDGET_BYTES` of pending `FullBaseline` records beyond the first; a forced refresh is not pending and never counts. The first pending baseline always goes and does not count against the budget. Despawn tombstones, deltas, and forced refreshes for entities the client already holds go first and never wait on the budget. A sent baseline stays in flight until the client acknowledges its snapshot or a later one. It is not re-sent meanwhile, and it returns to pending if that acknowledgement does not cover it; otherwise every snapshot inside one round trip re-sends the same window. Pending baselines go out with the recipient's own pawn first, then in stable `NetworkId` order across snapshots.
11. **Audience.** Presentation builders (`playSound`, `rumble`, `flashScreen`, `screenShake`, `vignette`) take an optional `audience`: a subject token in the reaction's scope — `on.activators` (the edge's player), `on.trigger` (players inside that volume), `on.player` (the event player) — or `"everyone"`. A token makes the reaction scoped. TS types reject it under a source that does not publish it, and so does install wherever the binding names its source. A shared named-event site skips it at dispatch, warning once, as it skips `at: on.emitter` (`scripting.md` §12). No presentation builder can follow a `wait`: the five are system-only primitives, never sequence steps (`validate_sequence_primitives` drops a sequence naming one), and `fire` refuses a scoped reaction, so an audience token never reaches a landing and the rule that no token survives `wait` stands. `"everyone"` under an every-machine drain means each machine presents its own run locally — no broadcast. Defaults and resolution are in the *Audience resolution* table. Recipients resolve once, at the queue push, to local and/or a set of seats.
12. **UI verbs.** A `MachineLocal` reaction a trigger binding can reach (directly, after `wait`, or through `fire`/`onComplete`) warns at install in a hosted session, naming the reaction and the binding. Behavior unchanged.

### Exempt and networked kinds

| Entity | Networked | State path |
|---|---|---|
| Descriptor with mesh, health, touchable, or behavior (dummy, prop, breakable, enemy, world item, invisible health-only target) | yes, host-materialized display copy | Transform, `MeshAnimationState`; descriptor light/emitter state on the same record once a host-only override or a placement KVP override exists |
| Light-only or emitter-only descriptor placement | no | bound by `MapEntity` ordinal on first host-only change |
| PRL light | no | bound by map-light index on first host-only change |
| Fog volume | no | bound by fog record index on first host-only change |
| Map-placed `billboard_emitter` | no | bound by `MapEntity` ordinal on first host-only change |
| `prop_mesh` (built-in classname: `MeshComponent`, no `DescriptorProvenance`) | no | bound by `MapEntity` ordinal on first host-only `setAnimationState`; `MeshAnimationState` on the bound record |
| Pawn, projectile presentation, mover | own paths | unchanged |
| Held wieldable | no | unchanged |

### Drain origins

Origin is set where each drain is called (`frame_loop/mod.rs`), from where that drain's input is produced. Verified against the connected-client branch, which fills its own predicted movement events, its own weapon events, and `observer_weapon_cues` — the `activate`/`impact` cues the host forwards for other shooters and for enemy shots (`publish_observer_weapon_cues`).

| Drain | Input produced | Origin |
|---|---|---|
| Trigger residuals (`drain_frame_residuals`) | host authoritative tick | host-only: edge player, trigger, occupants at the edge's tick |
| Player-event residuals (`drain_frame_residuals`) | host | host-only: event player |
| `wait` landings (`drain_landings`) | host scheduler only: connected clients refuse enrollment (`ReactionScheduler::enroll`) | host-only, no subject — a tail runs on the host alone, whatever source parked it |
| AI cue events (`pending_ai_events` by `address`) | host tick only | host-only, no subject |
| AI shot activations (`pending_ai_events` with `shot_id`: `activate` and its `emits.activate` alias) | host tick; each client re-fires them from its observer cues | every-machine |
| Observer weapon cues (`observer_weapon_cues`) | connected client only | every-machine |
| Death events (`pending_death_events`, both sweeps) | host tick and host death sweeps | host-only, no subject |
| Mover events (`pending_mover_events`) | host tick only | host-only, no subject |
| `fire` / `onComplete` follow-ups | the dispatching fire | host-only, no subject, when the dispatching fire was host-only; else every-machine |
| Movement events | each machine's own simulation (host: every pawn; client: its own prediction) | every-machine |
| Weapon events, including the host's remote-impact events | each machine's own fire or observation | every-machine |
| levelLoad, crossings | every machine | every-machine |

### Audience resolution

| Origin | Default `playSound` | Default rumble / screen effects | `on.activators` | `on.trigger` | `on.player` | `"everyone"` |
|---|---|---|---|---|---|---|
| Trigger residual | everyone | activator | the edge's player | players inside the volume at the edge's tick | out of scope | host + every participating client |
| Player event | event player | event player | out of scope | out of scope | event player | host + every participating client |
| Host-only, no subject | everyone | host-local | out of scope | out of scope | out of scope | host + every participating client |
| Every-machine | local | local | out of scope | out of scope | out of scope | local |
| Single player, any origin | local | local | local where legal | local where legal | local where legal | local |

"Out of scope" means the token is illegal there. Install rejects it where the binding names its source, and a shared named-event site skips it with one warning (Decision 11; Task 8, Recipients). A player with no bound seat (held, disconnected) presents nowhere. `"everyone"` never presents twice on one machine and never reaches a held or demoted client.

## Acceptance criteria

Automated:

- [ ] 1. On a connected client, a hitscan hit on a map-placed `target_dummy` is declared, accepted, and applied on the host; the kill credits the client's player (`byPlayer(impact.source)` XP lands on the client's seat); the host despawn removes the client's copy by the next snapshot.
- [ ] 2. For each descriptor shape — AI enemy, world item, health-only with mesh, mesh-only prop, health-only without mesh, light-only, emitter-only, light+mesh, emitter+health — host registration and client suppression agree, and the outcome matches the *Exempt and networked kinds* table: light-only and emitter-only stay client-local; every other shape registers. The role classification is one exhaustive `match` with no wildcard arm, so adding a `DescriptorComponentKind` variant fails to build until it is classified.
- [ ] 3. After join, a connected client holds exactly one entity per placement: no networked placement has both a local copy and a host-built copy, and no networked placement is missing its display copy.
- [ ] 4. Membership follows live components: a picked-up item unregisters and its client copy despawns; dropping it re-registers it, a dropped loadout weapon (`DefaultWeapon` spawn path) included; a networked prop's `setAnimationState` from a host-only reaction reaches every client.
- [ ] 5. A display copy carries the descriptor's mesh, light, emitter, and (when authored) hit volume, and none of `Health`, `Brain`, `Agent`, `Weapon`, `Touchable`, `PlayerMovement`.
- [ ] 6. A client hit inside the authored `health.hitbox` of a networked health-only target without mesh lands on the host; the same target with no authored hitbox is not hittable; no client-side damage, splash, or impact policy reads the hit volume.
- [ ] 7. A client shot aimed at the torso zone of a map-placed entity using the dev dummy model (`content/dev/models/decraniated_low_poly_retro_pixel/scene.gltf`, real `HitZoneStore` entry) resolves as an entity hit — both for a locally spawned entity and for a host-built display copy.
- [ ] 8. A trigger-fired `setLightAnimation`, each `setFog*` (Density, Glow, EdgeSoftness, Falloff, Params, Animation), `setEmitterRate`, and `setSpinRate` (rate and animation forms) on a PRL light, a fog volume, a map-placed emitter, a light-only descriptor placement, and a networked entity's descriptor light/emitter shows the same component values on every connected client and on a client that joins afterward; so does a trigger-fired `setAnimationState` on a `prop_mesh`.
- [ ] 9. A finite light or fog animation started N seconds before a client joins finishes on that client at the host's finish time (within one snapshot interval) and settles to the host's final values; a looping light animation's sampled phase on host and client agrees within one snapshot interval at a shared instant.
- [ ] 10. A light, fog, or emitter change from an every-machine drain (levelLoad, a crossing, a movement or weapon event) before any `wait` records nothing on the host and reaches no client: a host-player low-health crossing that dims lights leaves a client's lights unchanged. The same change after a `wait` lands on the host alone and records as host-only. A change from a host-only drain with no subject records and reaches every client: an enemy-death reaction's `setLightAnimation` shows on every connected client, and so does a levelLoad reaction's `setLightAnimation` after a `wait`.
- [ ] 11. A bound update whose index is out of range, whose local kind differs, or whose local origin differs by more than 1 cm warns once and changes no entity; a matching update applies.
- [ ] 12. With no host-only presentation change, a join sends no bound presentation record; an override written twice in one tick sends only the final value; an acked, unchanged override on a bound entity sends nothing on later snapshots; a networked entity with no override carries no state payload in its baselines or deltas; a networked entity whose placement overrides its light or emitter KVPs carries its state payload from install.
- [ ] 13. With every light, emitter, and fog volume on `stress-env-volumes-baseline` changed by one trigger, no per-client snapshot carries more than `JOIN_BASELINE_BUDGET_BYTES` of pending baselines beyond its first, forced refreshes excluded; a joining client holds every baseline within ⌈total baseline bytes ÷ budget⌉ + 2 snapshots plus one round trip on a lossless link, and no snapshot re-sends an in-flight baseline; a despawn issued during the backlog arrives in the next snapshot.
- [ ] 14. Trigger defaults: `playSound` presents on the host and every participating client exactly once each; `rumble`, `flashScreen`, `screenShake`, `vignette` present only on the activator's machine (the host's when the host's player activated). Host-only drains with no subject (AI cue, death, mover events, follow-ups): `playSound` presents everywhere; the other four present on the host only; an enemy-death reaction's `flashScreen` with `audience: "everyone"` presents on every machine once; a mover event's `playSound` played `at: on.emitter` presents on every machine, positioned at the emitter.
- [ ] 15. `audience` tokens on a trigger: `on.trigger` reaches exactly the players inside the volume at the edge's tick; `on.activators` reaches only the edge's player; `"everyone"` reaches every machine once — for every presentation kind. A `fire` of a token-audience reaction is rejected at install and by TS types, like any scoped reaction.
- [ ] 16. Player events keep E16 behavior with no `audience`; `audience: "everyone"` in a player event presents on every machine once; `on.activators` or `on.trigger` as a player event's audience is rejected at install (and by TS types), naming the reaction.
- [ ] 17. An `audience` token under a source that does not publish it is rejected by scope typing in TypeScript. In both runtimes it is rejected at install, naming the reaction and source, under a trigger binding, a player event, a crossing, levelLoad, or as a `fire` step's target. Under an AI, death, mover, weapon, or movement event it is skipped with one warning per (reaction, source); `"everyone"` under an every-machine drain presents locally on each machine, once; `audience` on a non-presentation builder or an unknown value is rejected at install — identically in TypeScript and Luau.
- [ ] 18. A held or demoted client receives no presentation command; single player presents every command locally regardless of `audience`, except a token its source does not publish, which a shared named-event site skips with one warning, as in a hosted session.
- [ ] 19. A `showDialog`, `openMenu`, `closeDialog`, cell, or text verb reachable from a trigger binding warns at install in a hosted session, naming the reaction and the binding, and still runs on the host only; single player and connected clients log nothing.
- [ ] 20. A peer on `WIRE_VERSION` 26 is refused at the transport gate; `WIRE_VERSION` is 27 and `SNAPSHOT_VERSION` is 18, pinned by tests.
- [ ] 21. `docs/scripting-reference.md` documents `audience`, its defaults, what reaches clients in co-op (networked kinds, host-only presentation changes), and the UI-verb limitation; the dev `player-events.ts` example includes a `screenShake` in a player event's fire list.

Manual (owner, Mac↔Windows on `content/dev/maps/splash-damage-demo.map` and the co-op test fixture from Task 12):

- [ ] M1. Client pistol kills on dummies credit XP to the client; a dummy either player kills disappears on both machines; client impact bursts land on the dummy.
- [ ] M2. A trigger dims lights, changes fog, and stops an emitter for both players, including a client that joins after the trigger; a looping light pulses in visible sync on both screens.
- [ ] M3. A pressure-plate flash plays only for the player who stepped on it; its alarm sound plays for both.
- [ ] M4. E16 M1 (level-up and scald on the affected machine only; vignette local, no delay) and E16 M2 (flash limiter and reduce motion apply to a forwarded flash and shake on a client).

## Tasks

### Task 1: Split-first

Behavior-preserving splits of files this plan extends past ~800 lines. No logic changes; existing tests pass unchanged; each split is its own commit-sized unit and the five are independent.
- `crates/netcode/src/lib.rs` (7.5k lines): move `client_receive_and_apply`'s remote-entity materialization loop (the class dispatch to `materialize_armed_remote_projectile` / `materialize_armed_remote_player` / `materialize_armed_remote_enemy` and the follow-on animation and weapon-attachment steps) into a `snapshot_apply` module; `client_receive_and_apply` calls it.
- `crates/netcode/src/client.rs` (5.5k): move mover binding (`mover_network_ids`, `find_loaded_mover_entity`, `validate_kinematic_mover_state_binding`, `prepare_mover_apply`'s binding half) into a `client/bindings` submodule that Task 11 extends.
- `crates/net/src/wire.rs` (3.6k): move `ComponentPayload`, `RawComponentPayload`, the `Wire*` component structs, `COMPONENT_KIND_*`, and their validate/convert arms into a `wire/components` submodule, re-exported at the old paths.
- `crates/net/src/replication.rs` (2k): move `ClientReplicationState` and `encode_for_client_with_sequence` into a `replication/encode` submodule that Task 6 extends.
- `crates/sim/src/scripting/builtins/data_archetype.rs` (3.6k): move `descriptor_carries_brain`, `descriptor_materializes_ai_enemy`, `descriptor_materializes_world_item`, `filter_out_client_host_replicated_placements`, `suppressed_client_host_replicated_mesh_models`, `ai_capsule_center_from_feet_offset`, and their tests into a `host_replicated` module, re-exported from `builtins` as today.

Light touches below do not trigger a split: `light_bridge.rs`, `fog_volume_bridge.rs`, `hit_zones.rs`, `system_reactions.rs`, `trigger_system.rs`, `presentation.rs`, `endpoint.rs`.

### Task 2: Thin slice — networked target dummy

Falsifies the classifier and display-copy assumptions end to end before fan-out. Build Decisions 1–2 and the class path:
- **Roles.** In `postretro-entities` beside `DescriptorComponentKind` (`provenance.rs`): `enum ReplicationRole { NetworksEntity, ClientLocalPresentation, PawnPath, HeldOrPawn }` and `DescriptorComponentKind::replication_role(self)`, an exhaustive `match` with no wildcard arm per Decision 1. Both predicates enumerate kinds through `DescriptorComponentKind::ALL`, a hand-written array that nothing checks. Generate the enum and `ALL` from one variant list (a local `macro_rules!`), so a new variant cannot be classified yet skipped. Move `descriptor_declares` (private in scripting-core `refresh_plan.rs`) to `EntityTypeDescriptor::declares(DescriptorComponentKind)` in entities, so the refresh planner and the descriptor predicate share it. Both predicates check `behavior` directly (`behavior.is_some()`, or live `Brain`+`Agent`); no role value stands for it.
- **Descriptor predicate** in `builtins::host_replicated`: `descriptor_is_host_replicated(&EntityTypeDescriptor) -> bool` = any declared `DescriptorComponentKind` whose role is `NetworksEntity`, or `behavior` declared. Replace both predicate calls inside `filter_out_client_host_replicated_placements` and `suppressed_client_host_replicated_mesh_models` with it. Delete `descriptor_materializes_world_item`; keep `descriptor_materializes_ai_enemy` for the spawner resolver in `startup/lifecycle.rs`, which stays AI-only. Rename the `suppress_ai_enemies` field and parameters to `suppress_host_replicated` at every site (`lifecycle.rs`, `lifecycle_world_cpu.rs`, test fixtures in `observability/driver.rs`, `lifecycle_progress_install_tests.rs`).
- **Live predicate** in netcode `descriptor_class.rs`: `is_host_replicated_descriptor_entity(registry, id) -> bool` per Decision 1 — provenance spawn path, then any live component whose `DescriptorComponentKind` role is `NetworksEntity` (via `DescriptorComponentKind::component_kind`), or live `Brain`+`Agent`. `descriptor_entity_class`'s third branch uses it, replacing `is_networked_ai_enemy || Touchable`.
- **One sweep.** Replace `host_register_map_enemies` and `host_register_world_items` with `host_register_descriptor_entities` (same signature; stale rule = `!is_host_replicated_descriptor_entity`; iterate entities carrying `DescriptorProvenance`). Merge `NetEndpoint::Host::{map_enemies, world_items}` into one `descriptor_entities` set, cleared where both are cleared today; update the two `NetEndpoint::Host` destructures in `main.rs`. Collapse the `App` wrappers into `host_register_descriptor_entities_after_install` / `_after_fixed_sim_tick` at the existing call sites (install path in `startup/lifecycle.rs`, per-tick sweep in `frame_loop/mod.rs`).
- **Agreement test** replacing `classifier_agrees_with_live_predicate_one_source_of_truth`: spawn every shape listed in AC 2 through `apply_data_archetype_dispatch` and assert `descriptor_is_host_replicated(descriptor) == is_host_replicated_descriptor_entity(&reg, id)` and the expected value. Invert existing assertions that pinned props as unregistered (`enemy_replication_harness_test.rs`, `replication.rs` unit tests, `data_archetype` filter tests) — they encode the old policy.
- **Harness**: extend `ingest_hit_harness_test` with a `target_dummy`-shaped placement: host registers it, client suppresses and materializes a mesh copy, a client hitscan `LocalHitRecord` on it becomes a declared record, `apply_valid_hit_record` applies it, the impact policy credits the client's seat, and the host despawn tombstone removes the client copy. A second case pins AC 4's membership half: pickup unregisters a world item and a drop re-registers it, for a map-placed item and for a dropped loadout weapon (`DefaultWeapon` spawn path).
Proves AC 1, 2, 3, and the membership half of AC 4.

### Task 3: Client hit against the real dummy model

Reproduce the playtest miss (research.md, *The impact-burst observation*). Build a registry with a `target_dummy` descriptor entity via `attach_descriptor_components`, install the real glTF with `HitZoneStore::insert_from_load` against the `content/dev` root, fire `resolve_test_client_shot` at the torso zone with the dev pistol, and assert `resolution.hits` names the entity and `impact_contacts()` carries `Some(target)`. Repeat with a mesh-only display copy built by `materialize_net_mesh_presentation`. If either fails, trace to cause — handle-key mismatch between `mesh.model` and the sweep's `ModelHandle`, broad-phase reject from `origin_offset` or transform, pose availability, or aim ray — and fix it in the owning module; record the cause in the task's commit message. If both pass, add a test pinning the playtest's actual aim (client eye height, dummy at the map's `splash_direct` origin) before closing. Proves AC 7.

### Task 4: Display copy presentation set and hit volume

Extend `materialize_net_mesh_presentation` (sim `net_descriptor.rs`) into the display-copy builder for Decision 3: after the mesh, attach the descriptor's `LightComponent` and `BillboardEmitterComponent`. `attach_descriptor_components` builds the light inline from the placement's `MapEntity` (its `origin`, plus `apply_light_kvp_overrides`), which a display copy does not have: extract that conversion into a helper taking the origin, and build the copy's light at the record's Transform position (the client's render-stage `LightBridge::absorb_dynamic_lights` call enrolls it); placement KVP overrides reach the copy only through Task 10's state payload. Also attach a `HitVolumeComponent { hitbox: Hitbox }` (new, in `entities/src/components/`, new `ComponentKind`) when `descriptor.health.hitbox` is set, using `HealthComponent::from_descriptor`'s `Hitbox` conversion with `ai_capsule_center_from_feet_offset` applied as today. A descriptor with no mesh no longer returns false: it materializes whatever presentation it has; transform-only stays the result when it has none. In `hit_zones.rs`, add `entity_hitbox(registry, id) -> Option<&Hitbox>` reading `HealthComponent.hitbox` then `HitVolumeComponent.hitbox`; use it at the hit query's hitbox lookup only — `damageable_volume`, splash, impact-policy anchor, and AI perception keep reading `HealthComponent` (Decision 4). `for_each_hittable_candidate` also visits `HitVolumeComponent` entities without Health or Mesh; check every caller's own liveness gate so none treats them as damageable. The client overlay anchor (`client_overlay_hitbox`) already reads the descriptor; leave it. Proves AC 5, 6.

### Task 5: Presentation state on the wire

In `wire/components` (Task 1), add three payload kinds with `COMPONENT_KIND_*` equal to the engine `ComponentKind` discriminants for `Light`, `FogVolume`, `BillboardEmitter` (`component_kind_pinned_to_engine_discriminants` extends); their `RawComponentPayload` slots append after `kinematic_mover` in engine numeric order — light (1), emitter (2), fog (5):
- `WireLightState { intensity, color, cone_direction, animation: Option<WireLightAnimation>, anim_start: Option<f64> }`.
- `WireFogState { density, glow, edge_softness, falloff, tint, saturation, min_brightness, light_range, animation: Option<WireFogAnimation>, anim_start: Option<f64> }`.
- `WireEmitterState { rate, spin_rate, spin_animation: Option<WireSpinAnimation>, anim_start: Option<f64> }`.
- `WireLightAnimation { period_ms: f32, phase: Option<f32>, play_count: Option<u32>, start_active: Option<bool>, brightness: Option<Vec<f32>>, color: Option<Vec<[f32; 3]>>, direction: Option<Vec<[f32; 3]>> }` — `LightAnimation` minus `radius`, which validation rejects.
- `WireFogAnimation { period_ms: f32, phase: Option<f32>, play_count: Option<u32>, density: Option<Vec<f32>>, saturation: Option<Vec<f32>>, min_brightness: Option<Vec<f32>>, light_range: Option<Vec<f32>> }`.
- `WireSpinAnimation { duration: f32, rate_curve: Vec<f32> }`.
State scalars are `f32`, `color` and `tint` are `[f32; 3]`, and `cone_direction` is `Option<[f32; 3]>`, matching the engine components.
- Wire `PresentationCommand::PlaySound` gains `at: Option<[f32; 3]>` after `bus`: the emitter's world position, resolved on the host at push (Decision 8). `None` plays unpositioned.
Add `presentation_binding: Option<WirePresentationBinding { kind: u8, index: u32, origin: [f32; 3] }>` to `FullBaseline` and `Delta`, beside `projectile_presentation`, and `server_script_time: f64` (the host's `script_time` at serialization) to the snapshot message. Validation: every float finite; curve vectors non-empty and at most `MAX_WIRE_CURVE_KEYS` (new const, 64) entries — engine curve validation sets no length cap (`validate_and_normalize` in `lighting/src/script_primitives.rs` rejects only empty channels), and one invalid record rejects the client's whole snapshot (`RawSnapshotMessage::validate` short-circuits), so Task 10's capture skips a write whose curve exceeds the cap, warning once per entity and naming the primitive; `period_ms > 0`; `kind` one of the *Boundary inventory* values; a record with a binding carries no `entity_class` and no Transform, and at least one state payload — light, fog, emitter, or, for a `MapEntity` binding, `MeshAnimationState` (a bound `prop_mesh`; validation sees only the binding kind) (a light-and-emitter placement carries two); a record without a binding may carry state payloads only beside a Transform. Bump `WIRE_VERSION` to 27 and `SNAPSHOT_VERSION` to 18; update the version-pin tests in `handshake.rs` and `wire.rs`; add a gate test refusing 26. Round-trip tests per payload, including `None` animations and maximum curve length, and for `PlaySound` with and without `at`. Proves AC 20 and the wire half of AC 8.

### Task 6: Join baseline budget

In `replication/encode` (Task 1), make `encode_for_client_with_sequence` budgeted per Decision 10. Order: despawn tombstones, then deltas and forced refreshes for entities the client holds, then pending `FullBaseline`s — the recipient's own pawn first, then ascending `NetworkId` — admitting baselines while the encoded size of admitted baselines stays within `JOIN_BASELINE_BUDGET_BYTES` (const in `crates/net`, initial value 16 KiB; the first pending baseline always goes and does not count against the budget, so one oversized entity cannot stall). Track, per client, the snapshot sequence each admitted baseline last went out in. That baseline is in flight, neither re-sent nor counted, until the client acknowledges that snapshot or a later one, and it returns to pending if that acknowledgement does not cover it. Measure with the same encoder `host_replicate` uses. An entity whose baseline is deferred gets its newest state when admitted; an entity despawned while pending gets only its tombstone. Add a `debug`-level log in `host_replicate` reporting per-client snapshot bytes and deferred-baseline count when any baseline is deferred. Unit tests over `ServerReplication` with synthetic payload sizes pin ordering, the first-record exception, own-pawn-first, the despawn-during-backlog case, a forced refresh during backlog, no re-send of an in-flight baseline before its snapshot's acknowledgement, re-pend after a lost snapshot, and convergence. Proves the budget half of AC 13.

### Task 7: Stable presentation identity

Give each bindable kind a resolver usable on both peers (Decision 5):
- Stamp `map_ordinal: u32` on `MapEntity` (sim `map_entity.rs`) from its position in the PRL map-entity list at load; carry it through `apply_classname_dispatch` and `spawn_descriptor_instance` into a new `MapOrdinal(u32)` component on the spawned entity.
- Make `LightBridge::entity_for_map_index` live (drop `#[allow(dead_code)]`) and add `LightBridge::map_index_for_entity`; add the same pair on `FogVolumeBridge` over `entity_ids`.
- Define `PresentationBindingKey { kind: PresentationBindingKind, index: u32 }` with `PresentationBindingKind::{MapLight, FogVolume, MapEntity}` in `postretro-entities` (both netcode and the app can name it), and a resolver trait the app implements over the two bridges and a registry `MapOrdinal` scan, returning `(EntityId, origin)`.
- Record that a `try_spawn` failure during population shifts no later index: populate keeps the index of every record it attempts, storing `None` for a failed slot.
Unit tests: resolution round-trips per kind; a failed spawn leaves later indices intact. Feeds Tasks 10 and 11.

### Task 8: Fire origin and recipients

Build Decision 11's routing half and the *Drain origins* and *Audience resolution* tables. Task 9 builds the authoring surface concurrently; the shared contract is the descriptor spelling of `audience` in the *Boundary inventory*.
- **Audience field.** Add `Audience::{Activators, Trigger, Player, Everyone}` (entities, beside `SystemReactionCommand`) and `audience: Option<Audience>` on the five presentation variants; the system-reaction args structs in `system_reactions.rs` parse it from the descriptor spelling (`"@activators"`, `"@trigger"`, `"@player"`, `"everyone"`). Legality is Task 9's; this task resolves whatever value arrives.
- **Fire origin.** Replace `SystemCommandFireContext.presentation_seat` with `origin: FireOrigin` — `EveryMachine`, or `HostOnly(FireSubject)` with `FireSubject::{None, Trigger { player: Option<Seat>, trigger: EntityId, occupants: Vec<Seat> }, PlayerEvent { player: Option<Seat> }}`. The variant, not its fields, picks the defaults: an unseated player event and a no-subject drain both lack a seat but default differently. An unseated player event — every single-player pawn, E16's `PlayerKey::Unseated` — has `player: None` and resolves `Local`. `FireOrigin::default()` is `EveryMachine`, because `route_player_presentation`'s host-seat re-push and `ingest_presentation_commands` push under the ambient context and must resolve `Local`. Any context no call site sets — the crossing stage, levelLoad, UI and console reactions, a `SystemCommandFireContext::default()` reset — records nothing and presents locally. A call site that sets an origin restores the previous context afterward, as the player-event drain does today. `fire_named_event_with_sequences` builds each named fire's context from the origin its caller passes, never from a fresh default. Set it at each drain call site in `frame_loop/mod.rs` per the *Drain origins* table: `drain_frame_residuals` sets trigger origins (the edge's player through `seat_for_pawn` for `PlayerId::Local`; for `PlayerId::Remote`, a client→seat lookup the app passes in from its `SeatTable`; occupants captured at the edge's tick — extend the residual tuple and add `TriggerSystem::occupant_players(trigger)`) and keeps player-event seats; the AI cue-address, death, and mover `drain_named_events_with_sequences` calls pass `HostOnly` with no subject; the movement, AI shot-activation, observer-cue, and weapon calls pass `EveryMachine` — a client already runs an enemy shot's `activate` reactions from its observer cue, so a host-only origin there would present a defaulted `playSound` twice on every client. Pass the origin as an argument, not by setting the context around the call. `fire_named_event_with_sequences` installs a fresh `SystemCommandFireContext` per fire, so `drain_named_events_with_sequences` takes the origin and `NamedEventDispatchContext` carries it into that install. `drain_landings` installs `HostOnly` with no subject for every landing; the scheduler's `InstanceKey` origin (`Option<(EntityId, PlayerId)>`, `None` for every sourceless enrollment) stays the dedup and Exit-cancel key it is. Follow-ups carry their dispatching fire's origin, subject cleared to `FireSubject::None` (follow-ups are context-free). `NamedEventDispatchContext` installs the origin whether or not an emitter is present. Follow-up entries — these drains' returns, `drain_frame_residuals`' `follow_ups`, and `pending_trigger_follow_ups` — become `(String, FireOrigin)`, and `dispatch_deferred_named_events_with_sequences` fires each entry under its own origin and gives every name it chains the same origin.
- **Recipients.** `SystemCommandQueue::push` resolves each `Presentation`-class command to `Recipients::{Local, Seats(Vec<Seat>), Everyone, Skip}` from its audience and the origin; `routed` holds `(Recipients, command)`. No other class changes behavior (Invariant I7).

  | Origin | Audience | Recipients |
  |---|---|---|
  | `EveryMachine` | none, or `"everyone"` | `Local` |
  | `EveryMachine` | a token | `Skip`, warning once per (reaction, source); only a shared named-event site (weapon, movement, AI shot activation, observer cue) reaches this |
  | `HostOnly`, any subject | `"everyone"` | `Everyone` |
  | `HostOnly(Trigger)` | none, `playSound` | `Everyone` |
  | `HostOnly(Trigger)` | none, rumble or screen effect; or `on.activators` | `Seats([edge player])`; `Seats([])` when the edge player has no bound seat |
  | `HostOnly(Trigger)` | `on.trigger` | `Seats(occupants)` |
  | `HostOnly(PlayerEvent)` | none, or `on.player` | `Seats([event player])`; `Local` when the event pawn has no seat (E16's rule) |
  | `HostOnly(None)` | none, `playSound` (with or without `at`) | `Everyone` |
  | `HostOnly(None)` | none, rumble or screen effect | `Local` |
  | `HostOnly`, any subject | a token its subject does not carry | `Skip`, warning once per (reaction, source), as a missing `at: on.emitter` is skipped |

  `route_player_presentation` delivers. With no server (single player), every routed command except `Skip` presents locally once, `Seats([])` included. When hosted: `Local` presents locally; `Everyone` presents locally once and sends one `ServerPresentationPayload::Command` per participating client; `Seats` presents locally once if the host's seat is listed, sends once per bound remote seat, and drops unbound seats; `Skip` does nothing. Every local presentation of a routed command re-pushes it with its `audience` cleared, so the ambient `EveryMachine` push resolves `Local`, never `Skip`. A forwarded `playSound` carries its emitter's world position, resolved at push (Task 5 wire field): `presentation_command_to_wire` writes it, `presentation_command_from_wire` reads it, and the client plays the sound spatialized there.
Tests per *Recipients* row through `SystemCommandQueue` and `route_player_presentation` with a fake server, single player included; a test that a connected client's frame never fills the AI, death, or mover queues and that an AI shot's `activate` reaction `playSound` presents once per machine (pins the *Drain origins* table); `PlaySound` position conversion round-trip. Proves AC 14, 15 (routing), 18, the drain half of AC 10, and the routing halves of AC 16 and 17.

### Task 9: Audience authoring and install checks

Build Decision 11's authoring half and Decision 12. Runs concurrently with Task 8; the shared contract is the descriptor spelling of `audience` in the *Boundary inventory*.
- **Builders.** Add `audience?` to the five builders in `sdk/lib/ui/reactions.{ts,luau}`: the subject-token types `ActivatorsTarget`, `TriggerTarget`, `PlayerTarget` (`sdk/lib/data_script/commands.ts` and Luau twin), or `"everyone"`. A token-valued audience makes the builder's result scoped, so the reaction must take `on` and its source must publish that token — reuse the existing scoped-reaction typing; no new legality table. Update typedef templates, generated `sdk/types`, and the build-time mirror in `script-compiler/src/light_membership.rs`. The descriptor spelling is pinned in the *Boundary inventory*; Task 8 owns the engine field that parses it. Partition (`trigger_bindings/partition.rs`) needs no change: it drops a presentation step only for a sentinel `target`, and `audience` rides in `args`. A presentation builder never follows a `wait` (Decision 11), so V4a in `startup/reaction_validation.rs` keeps "no token survives `wait`" unchanged. No existing gate reads `audience`: V4a and V4b read only `at` and IR owners in `args`, and the trigger binder reads only `target`. Add these rows:
  - Pass A (`startup/reaction_validation.rs`) drops a reaction whose `audience` is not one of the four values, or sits on any other builder, as `invalid_at_value` does for `at`.
  - V4b's `scoped_fire_targets` counts a token `audience` as a scope requirement, so a `fire` step targeting that reaction is dropped.
  - The trigger binder rejects `@player`. Player-event validation (`player_events/validate.rs`) rejects `@activators` and `@trigger`. Crossing and levelLoad installs reject all three. Each names the reaction and source.
  - A shared named-event site cannot know its reactions' sources at install, so push resolves an unpublished token to `Skip` (Recipients).

  TS builders recognize a token by its non-enumerable `__wire`, which only `PLAYER_TARGET` carries today. Add it to `ACTIVATORS_TARGET` and `TRIGGER_TARGET`, and pass the wire spelling to the Luau twins' `opaqueTarget`.
- **UI-verb warning** (Decision 12): at trigger-binding install in a hosted session, walk the binding's reactions (direct, post-`wait`, follow-ups — reuse the transitive `fire`/`onComplete` walker from player-event validation) and warn once per (binding, reaction) reaching a `MachineLocal` kind.
Tests: TS type tests in `sdk/type-tests/` (a token under a source that does not publish it fails to type-check; a `fire` of a token-audience reaction fails); TS/Luau install-parity fixtures for every validation row above; the UI-verb warning in a hosted session and its absence in single player and on a connected client. Proves AC 15 (`fire` rejection), 19, and the install and type halves of AC 16 and 17.

### Task 10: Host override capture and registration

Build Decision 7 on the host.
- **Chokepoint.** In scripting-core reaction dispatch (`primitive_dispatch.rs` for tag-targeted fog and emitter primitives; the sequence-step dispatch that runs `setLightAnimation` handlers), after a successful apply of a primitive classified as replicated presentation (`setLightAnimation`, `setFogDensity`, `setFogGlow`, `setFogEdgeSoftness`, `setFogFalloff`, `setFogParams`, `setFogAnimation`, `setEmitterRate`, `setSpinRate` — one `const` list, with a test asserting it matches the registered light/fog/emitter primitives), and only when `ScriptCtx`'s current fire origin is `HostOnly`, write `PresentationOverride { component snapshot, anim_start }` (Decision 7) for each target into a `PresentationOverrides` table on `ScriptCtx`, keyed by `(EntityId, ComponentKind)` (`Light`, `FogVolume`, or `BillboardEmitter`). Last write wins per key, so a light-and-emitter entity keeps both overrides. `ScriptCtx` already reaches both dispatch paths (`primitive_dispatch` and `dispatch_sequence` each take `script_ctx: &ScriptCtx`); before each drain, the app sets the `script_time` the host's bridges will sample this frame on `ScriptCtx`. `setAnimationState` on a `prop_mesh` records too, at both of its paths: the reaction-dispatch chokepoint under a host-only origin, and the `BoundTriggerCommand::AnimationState` arm in `trigger_commands.rs` (reached through `execute_with_script_ctx`, which threads `ScriptCtx`), which runs in the host tick and always records. Key `(EntityId, ComponentKind::Mesh)`. Both paths record only for a target without `DescriptorProvenance`. A descriptor entity's animation needs no capture: `MeshAnimationState` already rides its record while it is networked, and an override record there would shadow its live state in `collect_payloads`.
- **Registration.** A host sweep `host_register_presentation_overrides` (netcode `replication.rs`, beside `host_register_descriptor_entities`, called on the host after `drain_landings` and before host serialization) walks the override table and decides once per entity, across all of its overridden kinds. It visits entities in ascending `EntityId` order — the table is a `HashMap`, and registration order fixes `NetworkId` order, which the baseline budget sends in.

  | Overridden entity | Sweep action |
  |---|---|
  | despawned | drop its overrides; unregister it if bound, so the host sends its tombstone (a hittable `prop_mesh`; Task 11) |
  | networked now (Decision 1's live predicate) | nothing; its overrides ride its own record |
  | descriptor entity whose `DescriptorProvenance` owns a `NetworksEntity` kind, not networked now (a held item) | nothing; its overrides wait and ride its record when it re-registers |
  | already registered as bound | nothing |
  | anything else (PRL light, fog volume, light-only or emitter-only placement, map-placed `billboard_emitter`, `prop_mesh`) | stamp a `NetworkId`, register, record its `PresentationBindingKey` from Task 7's resolver |

  The override table and registrations clear at level install.
- **Production.** `collect_payloads` emits the matching state payload from the override record (never from the live component, which fog and finite lights rewrite each frame), and the snapshot producer sets `presentation_binding` for bound entities. A bound entity emits only its state payloads: no Transform and no `entity_class`. Its `MeshAnimationState` goes out only for a bound `prop_mesh`, from that entity's `ComponentKind::Mesh` override record. Today `collect_payloads` emits a Transform for every entity that has one. The producer checks each state payload against Task 5's validation before encoding, because one invalid record rejects the client's whole snapshot. A payload that fails is dropped with one warning naming the entity; capture already skips an over-cap curve (Task 5), so this check is a backstop. A bound entity left with no valid state payload stays out of the snapshot. A networked descriptor entity with no override carries no state payload. Its display copy takes the descriptor's authored light and emitter values at materialization and starts their authored animations locally, as client-local presentation does today. Deltas resend every component of a dirty entity, so an overridden networked entity's state payload rides each of its deltas. That cost is accepted because overrides on moving entities are rare. A placement whose map KVPs override its descriptor light or emitter (`DescriptorMapOverride`: `initial_intensity`, `initial_color`, emitter `initial_rate`, `initial_spin_rate`) counts as an override captured at its first registration with `anim_start: None`, because a client never spawned that placement and cannot apply its KVPs.
Tests: every-machine drains record nothing, a host-only drain with no subject records, and a levelLoad tail's post-`wait` write records (AC 10); double write in one tick yields one final record (AC 12); zero records with no override, no state payload on a networked entity with no override, and a payload from install on one whose placement overrides its light KVPs (AC 12); a light-and-emitter entity keeps both overrides; a trigger-fired `setAnimationState` on a `prop_mesh` records through the bound-command arm, and a death-event one through the chokepoint (AC 8); fog override captures the authored write, not a later sample; a write whose curve exceeds `MAX_WIRE_CURVE_KEYS` records nothing and warns once (Task 5). Proves AC 10, 12 and the host half of AC 8.

### Task 11: Client binding and clock rebase

Build Decisions 6 and 9 in `client/bindings` (Task 1).
- On a `FullBaseline` with `presentation_binding`, resolve the local entity via Task 7's resolver; apply Decision 6's checks; on success map `NetworkId` → `EntityId` (no materialization, no Transform write) and remember the binding; on failure warn once per `(kind, index)` and ignore further records for that `NetworkId`. Clear bindings where `mover_network_ids` clears.
- Apply each state payload by writing the component, then rebasing per Decision 9: age = `host_now − anim_start`, skipped when `anim_start` is `None`. Lights: finite → seed `LightBridge`'s `animation_start_time` to local `script_time − age` (add a seeding entry point); looping → set `phase` to `phase + (host_now − local script_time) / period` (mod 1), whatever `anim_start` holds. Fog: seed `FogVolumeBridge`'s anim slot start to local `script_time − age`. Emitter: seed `EmitterBridgeState.spin_elapsed` to `age`. A finite animation whose age exceeds its duration settles on the next bridge update.
- Unbound records carrying state (descriptor display copies) apply the same way onto the materialized copy.
- Each seeding entry point also records the written component as that bridge's last-seen value: the light bridge's per-light snapshot component, the fog anim slot's cached animation, and the emitter's last spin animation. Snapshot apply runs at frame start, before the emitter, light, and fog bridges. Each bridge restarts an animation it sees change, so without the recorded value the seed is replaced by a fresh start in that same frame. Test: apply a finite baseline at age 2 s of 5 s, run each bridge once in the same frame, and assert the animation is still 2 s old.
- A bound `MeshAnimationState` applies through `apply_mesh_animation_state`, as for a display copy.
- A despawn tombstone for a bound `NetworkId` removes the mapping and despawns the local entity. Only a hittable `prop_mesh` reaches this here, through an impact policy's `despawn`; no path despawns the other bound kinds (Out of scope). A client that joins after that despawn never receives the tombstone (the host pre-acks active tombstones for a joiner) and keeps its own spawned copy; the persistent removal record that closes this belongs to `coop--bound-presentation-despawn`. Bindings clear at level change.
Tests: binding mismatch cases (AC 11); late-join rebase for finite and looping animations against a synthetic host clock (AC 9), including a host stall whose frame time exceeds the tick clamp, a one-way latency above one snapshot interval, a fog density write and an emitter rate write during a running animation (start unchanged), and an identical animation rewrite (no restart); a bound `prop_mesh` animation state applies, and its tombstone despawns the local copy. Proves AC 9, 11 and the client half of AC 8.

### Task 12: Content, docs, end-to-end proof

- **Fixture.** Add `content/dev/maps/coop-presentation-test.map` with a pressure-plate trigger whose fire list runs three reactions: an alarm (`playSound`, default audience), a flash (default), and a shake with `audience: on.trigger`; an enemy whose death reaction runs `setLightAnimation` and a `flashScreen` with `audience: "everyone"`; a second trigger that animates a PRL light, sets fog density and animation, stops a map-placed emitter and a light-only descriptor placement, and switches a `prop_mesh`'s animation state; target dummies; a mesh-only prop with an animation state; an invisible health-only target with an authored hitbox; a door whose mover event plays a positioned sound (`playSound` `at: on.emitter`, as the dev `door.open` reaction in `content/dev/scripts/positional-sound.ts` does). Add `screenShake` to a player event in `content/dev/scripts/player-events.ts` and its Luau twin.
- **Fog and emitter primitives have no typed SDK builder**: `setFog*`, `setEmitterRate`, and `setSpinRate` are reachable only as raw tag-keyed descriptors, so the fixture's scripts and the docs examples use that form.
- **Docs.** `docs/scripting-reference.md`: `audience` on each presentation builder, the defaults table in author terms, a *Co-op* subsection listing what reaches clients (networked kinds, host-only presentation changes and their late-join behavior) and the UI-verb limitation.
- **End-to-end harness** (netcode, extending the conditioned-link harness): host + two clients, one joining after both triggers fire; assert AC 8 and 9 on the late joiner, AC 14–15 per client (the door's sound plays positioned at the door on both clients), AC 1 over the conditioned link, and AC 4's animation half (a host-only `setAnimationState` on the fixture's mesh-only prop reaches both clients).
- **Resource bound** (AC 13, `testing_guide.md` §Resource bounds): fixture `stress-env-volumes-baseline` with a test-only trigger reaction overriding every light, emitter, and fog volume; inputs one joining client; metric per-snapshot baseline bytes and snapshots-to-converge from the Task 6 log; machine class any (byte counts are platform-independent); upstream limits named — renet fragmentation and `available_bytes_per_tick`. Record the measured totals in the test.
Proves AC 13, 21 and drives M1–M4.

## Sequencing

**Phase 0 (concurrent):** Task 1 — five independent splits.
**Phase 1 (concurrent):** Task 2 — thin slice, falsifies the classifier and display-copy assumptions across host registration, wire class, client suppression, materialization, and host HIT ingestion; Task 3 — independent of Task 2's files.
**Phase 2 (concurrent):** Task 4, Task 5, Task 6, Task 7, Task 8, Task 9 — disjoint files: Task 4 sim `net_descriptor.rs`/`hit_zones.rs`/entities components; Task 5 net `wire/components`; Task 6 net `replication/encode`; Task 7 sim `map_entity.rs` + bridges; Task 8 entities `system_commands.rs`, sim `residual_drain.rs`/`trigger_system.rs`/`system_reactions.rs`/the scheduler's `drain_landings`, scripting-core `reaction_dispatch/mod.rs`, postretro `main.rs`/`frame_loop/mod.rs`, netcode `presentation_commands.rs`; Task 9 SDK `sdk/lib/ui/reactions.*`/`sdk/lib/data_script/commands.*`/typedef templates/`sdk/types`, script-compiler `light_membership.rs`, postretro `startup/reaction_validation.rs`, sim `player_events/validate.rs` and the trigger binder. One shared seam: Task 4's `HitVolumeComponent` and Task 7's `MapOrdinal` each add a `ComponentKind` variant, editing `ComponentKind` and its `COUNT` list (entities `registry.rs`), `component_kind_discriminant` (netcode `lib.rs`), and the exhaustive kind matches in entities `ffi.rs` and postretro `observability/mod.rs`. Task 4 takes discriminant 22; Task 7 rebases onto it and takes 23. Task 8's `PlaySound` position conversion consumes Task 5's wire `at` field, so Task 8 rebases onto Task 5 before that edit.
**Phase 3 (concurrent):** Task 10, Task 11 — both consume Task 5's payloads and Task 7's identity; Task 10 consumes Task 8's fire origin.
**Phase 4 (sequential):** Task 12 — consumes everything.

## Rough sketch

- Classifier pair lives beside the existing pair it replaces; the agreement test stays in netcode because it needs both crates.
- `FireOrigin` replaces a field E16 added; E16's player-event path sets `HostOnly(FireSubject::PlayerEvent { player })` where it set `presentation_seat`.
- `PresentationOverrides` is a host-side `HashMap<(EntityId, ComponentKind), PresentationOverride>` on `ScriptCtx`; clients and single player never read it. Single player still writes it harmlessly; the registration sweep runs only on a host.
- The looping-light rebase works in host `script_time`: the host's GPU samples `t_h / period + phase` against its own `script_time`, so the client picks `phase'` so `t_c / period + phase'` equals that sample at the same instant (`t_h ≈ host_now`).

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

Builders with no options object today gain one after every existing positional, optional ones included: `rumble(strong, durationMs, weak?, options?)`, `flashScreen(color, durationMs, options?)`, `vignette(strength, durationMs, color?, options?)`, `screenShake(amplitude, durationMs, frequency?, options?)`, with `options = { audience? }`. A caller skipping an optional positional passes `undefined` (`nil` in Luau). `playSound`'s existing options object gains the field.

## Wire format

bitcode, pinned; layout follows the existing `RawComponentPayload` pattern — one new `Option` slot per payload kind, appended after `kinematic_mover`, and one `Option<WirePresentationBinding>` record field appended after `projectile_presentation` on `RawEntityRecord`, and `server_script_time: f64` appended to `RawSnapshotMessage`. Field order within each struct is as listed in Task 5. Curves (`Option<Vec<f32>>`, `Option<Vec<[f32; 3]>>`) encode as bitcode options of vectors; empty curve vectors are invalid (validation rejects, as the engine does). `None` animation means "no animation." Mirrors the `WireKinematicMoverState` precedent for a state payload carrying its own identity check.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1. One decision per descriptor shape: host registers iff client suppresses | Task 2 | any new placement filter or registration sweep; a new `DescriptorComponentKind` (caught at compile time by the exhaustive role `match`) | AC 2, 3 |
| I2. Display copies carry no authority component | Task 2, Task 4 | display-copy builder; any new descriptor component | AC 5, 6 |
| I3. Host records presentation overrides only under a host-only origin, set per drain call site; from a trigger's bound `setAnimationState` on a `prop_mesh` in the host tick; or from a networked placement's light or emitter KVPs at first registration | Task 8 (origin), Task 10 (capture) | a new drain call site; follow-up dispatch; every-machine drains sharing a `ScriptCtx`; a client applying bound trigger commands | AC 8, 10 |
| I4. A state payload is constant between host-only writes | Task 5, Task 10 | fog and light bridges rewriting components; any per-tick field (age, sampled value) | AC 12 |
| I5. A bound update writes only the entity its binding names | Task 7, Task 11 | index shift on spawn failure; map mismatch between peers | AC 11 |
| I6. The baseline budget never delays a despawn, a delta, or a forced refresh | Task 6 | encode ordering; new record kinds; in-flight tracking | AC 13 |
| I7. Recipients resolve from audience and origin, independent of command class | Task 8 | future forwarded command classes | AC 14–19 |
| I8. Each recipient machine presents a command at most once | Task 8 | `Everyone` local + broadcast; `"everyone"` under an every-machine drain | AC 14, 16, 18 |

## Orderings

| Scenario | Ordering | Expected |
|---|---|---|
| Host change, then join | override before client participates | baseline at participation carries the override; rebased age |
| Join mid finite animation | client binds at age < duration | finishes at host finish time ±1 snapshot interval (AC 9) |
| Join after finite animation ended | age ≥ duration | settles immediately to final values |
| Two host-only writes to one light, one tick | write A, write B | one record with B (AC 12) |
| Host-only write and every-machine write, same light, one snapshot interval | host override A, then client-local crossing B — or B, then A | host shows whichever it applied last and records only A; a client applies the snapshot carrying A at frame start, before that frame's crossings, so it ends on B when its B fires in the frame A arrives or later, and on A when its B fired earlier |
| Override on a networked entity, then despawn | override, despawn | tombstone; no orphan binding |
| Baseline pending, entity changes | deferred, then changed | admitted baseline carries newest state |
| Baseline pending, entity despawns | deferred, then despawned | tombstone only |
| Trigger edge, activator disconnects before drain | edge at tick N, seat unbound at drain | activator audience presents nowhere; everyone/occupants unaffected for others |
| `wait` tail `fire`s a presentation reaction | follow-up after the landing | host-only, no subject: `playSound` everyone, rumble and screen effects host-local; light, fog, and emitter writes in the tail record as host-only (Task 10) |
| Two activators on one tick | two edges | two fires; each activator gets its own command once |
| Item picked up on tick N, dropped on tick N+1 | sweep after N unregisters; sweep after N+1 re-registers | fresh `NetworkId`; client despawns old copy, materializes new |
| Item picked up and dropped within one tick | end-of-tick sweep sees `Touchable` present | same `NetworkId`; no membership change; client sees a Transform delta |
| Client hit on a dummy the host killed that tick | kill applied, HIT arrives | host rejects (terminal removal), as today |
| Level change | new install | host clears overrides and registrations; client clears bindings |
| levelLoad or crossing reaction with a `wait`, then a light step | host lands the tail; a connected client never enrolls it | host records; every client shows the change; no client runs the tail (Task 8, Task 10) |
| AI shot `activate` reaction with a default `playSound` | host AI shot-activation drain; client observer-cue drain | each machine presents once; no broadcast (Task 8) |
| Two players enter one trigger in one tick; `audience: on.trigger` | occupancy updates for both before either edge dispatches | both edges capture both players; each occupant presents twice (Task 8) |
| Exit edge with `audience: on.trigger` | leaver leaves occupancy before its exit dispatches | remaining occupants only; the leaver presents nothing (Task 8) |
| Dead player inside the volume at the edge's tick | `on.trigger` captured | excluded, matching the trigger's alive-filtered occupancy count (Task 8) |
| Host-only positioned `playSound`, emitter entity despawned the same tick | position resolved at push | sound plays at the last resolved position on every machine (Task 8) |
| Late-join baseline applied; bridges run the same frame | apply at frame start; emitter, light, and fog bridges later | animation keeps its seeded age; no restart (Task 11) |
| `setFogAnimation` at T, `setFogDensity` at T+2 s, join at T+3 s | second write leaves `animation` unchanged | client fog animation age 3 s (Task 10, Task 11) |
| Spin tween at T, `setEmitterRate` at T+2 s, join at T+3 s | second write leaves `spin_animation` unchanged | client `spin_elapsed` 3 s (Task 10, Task 11) |
| Identical `setLightAnimation` rewrite mid-animation | host bridge sees no change | host does not restart; `anim_start` kept; late joiner finishes at the host's time (Task 10, Task 11) |
| Host frame stall longer than the tick clamp between write and join | host `script_time` outruns ticks | late joiner finishes within one snapshot interval of the host (Task 11) |
| One-way latency above one snapshot interval | snapshot applied after its `server_tick` | age uses `estimated_server_tick`; finish within one snapshot interval (Task 11, Task 12) |
| Same override delta delivered twice (late ack) | re-apply | same seeded start; no visible jump (Task 11) |
| Display copy and its state payload in one record | materialize, then apply the payload | payload values win over authored values (Task 4, Task 11) |
| Networked moving entity with no override | Transform deltas every snapshot | no state payload in any delta (Task 10) |
| Mid-session trigger overrides more baseline bytes than the budget | registrations reach connected clients | converge within ⌈bytes ÷ budget⌉ + 2 snapshots plus one round trip; deltas and despawns undelayed (Task 6) |
| Baseline in flight; next snapshot due before its ack | sent at sequence q | not re-sent until a snapshot at or after q is acknowledged; re-pends if that acknowledgement does not cover it (Task 6) |
| Forced refresh during a join backlog | refresh requested | refresh `FullBaseline` goes in the next snapshot, outside the budget (Task 6) |
| Join backlog and the joiner's own pawn | pawn holds the newest `NetworkId` | own pawn's baseline goes first (Task 6) |
| Host-only landing, then the crossing stage, same frame | landing sets `HostOnly` and restores; crossing fires after | crossing write records nothing (Task 8, Task 10) |
| Every-machine `onComplete` follow-up and a death-event follow-up in one deferred batch | one deferred dispatch | first records nothing; second records (Task 8, Task 10) |
| Overrides on several entities in one drain | registration sweep | `NetworkId`s assigned in ascending `EntityId` order (Task 10) |

## Script syntax examples

```typescript
import { defineReaction, getMapEntities } from "postretro";
import type { TriggerEventParams } from "postretro";
import { playSound, flashScreen, screenShake } from "postretro/ui";

// One effect per reaction; the trigger's fire list runs several.
const alarm = defineReaction("vault.alarm", playSound("sfx/alarm"));             // everyone (default)
const flash = defineReaction("vault.flash", flashScreen([1, 0, 0, 0.5], 250));   // activator (default)
const shake = defineReaction("vault.shake", (on: TriggerEventParams) =>
  screenShake(4, 300, undefined, { audience: on.trigger }),                       // players inside the volume
);
const click = defineReaction("vault.click", (on: TriggerEventParams) =>
  playSound("sfx/click", { audience: on.activators }),                            // only the stepper
);

const rampage = defineReaction("streak.rampage", playSound("announcer/rampage", { audience: "everyone" }));
// players().on(becomes(read(streak.kills).ge(5)), [rampage]) — the host decides; every machine hears it

export function setupLevel() {
  return {
    reactions: [alarm, flash, shake, click],
    triggerEvents: getMapEntities("trigger", { tag: "vault_plate" }).map((t) =>
      t.on("enter", [alarm, flash, shake, click]),
    ),
  };
}
```

```lua
local Postretro = require("postretro")
local UI = require("postretro/ui")

-- One effect per reaction; the trigger's fire list runs several.
local alarm = Postretro.defineReaction("vault.alarm", UI.playSound("sfx/alarm"))
local flash = Postretro.defineReaction("vault.flash", UI.flashScreen({ 1, 0, 0, 0.5 }, 250))
local shake = Postretro.defineReaction("vault.shake", function(on)
  return UI.screenShake(4, 300, nil, { audience = on.trigger })
end)
```

Lights, fog, and emitters need no new syntax: a trigger-fired `setLightAnimation` or `setFogDensity` now reaches every client.

## Open questions

- `JOIN_BASELINE_BUDGET_BYTES` initial value (16 KiB) is a guess sized under renet's per-tick allowance; Task 12's measurement may move it.
