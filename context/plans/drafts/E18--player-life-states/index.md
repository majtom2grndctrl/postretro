# Player Life States

## Goal

Give mod authors an engine floor for player death and respawn, so they express death mechanics as policy: Quake co-op respawn, Doom restart, or a Borderlands-style down state with bleed-out. The engine adds author-named **life states** that gate an engine-closed capability set, reads for conditions and the HUD, verbs to change state and respawn, and an author map for the state a rejoining player enters. It adds no death policy. The dev mod ships down → bleed-out → respawn as the first consumer. Connected clients feel it as smoothly as the host: no transition snaps the view except respawn, and the bleed-out bar advances every frame. This is the co-op floor E18-R (respawn and player-leave) names. Player-leave keeps the existing harvest-and-hold contract; this spec adds only the life state a rejoining player enters (AC14).

## Scope

### In scope

- `components.lifeStates` on an entity type descriptor: author-named states, an `initial` state, per-state capability profiles, and an optional `rejoin` map. One validator serves both runtimes.
- A global life-state name table, sorted, that rides `TuningPayload` with every declared profile.
- Per-pawn life state (state id plus elapsed time in that state). It is stamped onto the movement component and replicated to the owner and to observers.
- Capability gates, routed through one helper:
  - `movement.speedScale`, `movement.abilities`, `movement.posture`
  - `look`, `fire`, `switchWeapon`, `reload`
  - `use`, `touch`, `occupancy`, `damageable`
  - Pawns that declare no states keep today's behaviour.
- Verbs on `on.player` / `on.activators` / `players()`:
  - `setLifeState(name)`
  - `respawn({ health?, lifeState? })`
- Reads:
  - `inLifeState(name)` (Bool IR input)
  - `player.lifeStateMs` (per-player Number slot)
  - `player.lifeState` (local String slot for the HUD)
- Rejoin: the seat harvest carries the leaving state by name, and restore enters the author's `rejoin` target.
- Hot-reload remap of live pawns by state name.
- Dev-mod down/bleed-out policy and its HUD.
- Updates to `docs/scripting-reference.md` and `docs/teleporters.md`.

### Out of scope

Later phases build on this shape. Each is additive to this surface.

- **Next: `E18--player-revive-assist`.** Teammate assist (held Use, the `assistable` profile key, assist sources and progress) and impact-policy life-state reads and effects (`source.inLifeState`, `source.setLifeState`). It depends on this spec and lands before the phases below.
- **Phase 2: respawn variety.**
  - `respawn({ loadout })` reset to the descriptor loadout.
  - Drop-on-death backpacks.
  - Checkpoints: `respawn({ at: { tag } })`, trigger-set per-player anchors, and `respawnAtCheckpoint()`.
  - Press-to-respawn: an input-edge player-event source.
  - Post-respawn invulnerability.
- **Phase 3: presentation.**
  - Spectate/follow camera and a death cam (view roll and drop).
  - A per-state avatar `animation` override. Host `update_player_animation_locomotion` and the client remote-avatar classifier reselect every tick, so both need a hold path.
- **Phase 4: squad rules.**
  - Group aggregate conditions (all players down → wipe).
  - Game over and wave-end respawn.
  - Lives counters are already expressible through per-owner stores, so they need no engine work.
- **Not planned:**
  - A `collide` capability (ghost pawns).
  - Excluding a life state from AI target selection. Selection stays aliveness-free (`entity_model.md` §AI).
  - Re-keying `playerDied`. It keeps its HP-latch, host-local, param-less contract.
  - `loadLastSave` / saves.
  - PvP and frag semantics.

## Direction

**Problem.** The engine has no notion of a player being out of play. A zero-HP pawn keeps moving, using and touching. Per-player death edges exist (`players().on(becomes(read(player.health).le(0)), …)`), but nothing they fire can gate, relocate, or revive one player; `playerDied` stays a param-less host event. Authors therefore cannot build any death mechanic beyond "restart the level for everyone".

**Placement.**
- **Engine mechanism vs mod policy.** The engine owns what a life state *means* to engine systems: the capability gates, respawn relocation, and carrying the state across a rejoin. The mod owns which states exist, when transitions happen, which state a rejoin enters, and every consequence (timers, screens). The engine never transitions a pawn on HP.
- **Host vs client.** Transitions and their consequences are host-authoritative. Gates run on host, single player, and client prediction from the same profile data. What the owning client *watches* (its movement through a transition, its bleed-out time) is predicted locally from data it already holds, then corrected smoothly, so host authority never shows up as latency or stair-stepping.
- **Descriptor vs code.** Profiles are descriptor data because gates read them every tick, inside movement replay, where the IR cannot run. The `rejoin` map is descriptor data because restore runs at materialization, before any script reaction can.

**Prior commitments.**

- **"The engine has no built-in death policy"** (`scripting.md` §10.4) and **"death is modder policy derived from impact"** (Q3 archive, E16). Kept. Zero HP fires only what it fires today, and every transition is an authored verb or authored `rejoin` entry.
- **Engine-closed component vocabulary** (`index.md` §4).
  - Kept for capabilities: the profile keys are a closed engine set, and unknown keys are rejected.
  - State *names* are authored. A name is data selecting a profile; it is not a new component kind.
- **Player-descriptor-composition draft: "closed engine state vocabulary; authors never register states".** This spec diverges in name only.
  - Life states are not movement states. They scale and gate movement but add no movement state. A downed pawn is still in `ground`/`air`/`crouch`.
  - `movement.posture: "crouch"` drives the existing crouch state rather than defining a new one.
- **E16 placed enemy "downed" and "stagger" states in modder-owned per-entity state.** This spec diverges for players: life state is an engine-owned field. Gates run inside movement replay, which cannot read per-entity state or run the IR, so the state and its profile must be engine data.
- **Declaring states hands zero-HP liveness to the author.** A mod that declares states but authors no zero-HP transition leaves a zero-HP pawn in its current state, firing included. This extends E16's "zero HP is inert" to players on purpose. The docs (AC21) call it out as the first rule of using life states.
- **Client predicts only with host numbers** (`networking.md` §What gates). Kept. Profiles and the name table ride `TuningPayload`, and clients never read local descriptors for gates.
- **Target selection holds no aliveness policy** (`entity_model.md` §AI). Kept. `damageable` gates spatial damage eligibility, the site that already gates AI attacks, and never selection.
- **Player events are host-only, and `on.player` is legal only before `wait`** (`scripting.md` §12). Kept. Bleed-out uses the `player.lifeStateMs` fact, so no per-player cancellable timer is needed.
- **Seat harvest and hold** (`networking.md`: a seat holds its carried state across a disconnect hold and level transitions). Kept. Carried health keeps `restore_carried_health`'s rule (carried ≤ 0 restores max). The carried record gains the leaving state's name; an expired hold mints a fresh seat, which carries nothing.
- **Co-op research: E18-R needs respawn after a delay, a latch that re-arms, and dead players gating occupancy** (`context/research/co-op-triggers-trap-pools.md`).
  - Delivered by `respawn`, `player.lifeStateMs`, and the `occupancy` capability.
  - Re-arming already holds through `set_health_absolute`, which respawn and `grantHealth` both reach.

**Alternatives rejected.**

- **An engine-owned closed `alive`/`down`/`dead` model.** The owner chose author-named states. A closed set fixes three mechanics in engine vocabulary. Named states with closed capabilities cover the same ground and still let authors add "spectating" or "bleeding".
- **Engine features per mechanic** (a revive system, a lives system). These put policy in the engine. A revive is `setLifeState` plus `grantHealth` in an authored fire list.
- **A raw `setCapabilities({...})` verb with no named states.** Named profiles validate at load, cost one byte on the wire, replay deterministically from that byte, and give `player.lifeStateMs` and `inLifeState` something to read. A raw verb would ship the whole capability set per change and leave conditions nothing to name.
- **Bleed-out as a per-player `wait`.** This needs `on.player` after `wait` and cancellation on revive. Player events reject interruptible waits. A readable elapsed-time fact is simpler, shows on the HUD, and resets on every transition by construction.
- **An engine-fixed rejoin state.** Always `initial` lets a downed player escape by reconnecting. Always the leaving state traps a reconnecting player in a state the author meant to be transient. A per-state map costs one validated descriptor key and lets the author pick either, or a third state such as `spectating`.
- **Carrying the state id instead of its name.** Ids are indices into a table that hot reload rebuilds during the hold. The name survives a rebuild or fails to resolve visibly.

**Foreclosures and one-way doors.**

- New `WirePlayerMovementState` fields and a `TuningPayload` epoch bump. Pre-stable, undone with another bump.
- SDK names (`lifeStates`, `rejoin`, `setLifeState`, `respawn`, `inLifeState`) become the modder contract. Renaming them costs a pre-stable SDK break across both runtimes.
- `PlayerSlot` goes from 14 to 15. The per-player slot bitset (`EntityScope.warned_owned_engine`, capped at 16 by a compile-time assert) has one slot left.
- The u8 state id caps the global name table at 255 names. That is material only for a mod with more than 255 distinct life-state names.
- The carried life-state name is host-local seat data, never on the wire. Nothing material is foreclosed.

## Acceptance criteria

### Descriptor and validation
- [ ] **AC1.** Both runtimes load a `components.lifeStates` block with `initial`, `states` and optional `rejoin` into the same lowered shape. Loading is rejected, with the full key path, for each of these:
  - an `initial` that names no declared state
  - a non-identifier state name
  - an unknown profile key
  - a `speedScale` outside `[0, 1]` or non-finite
  - an unknown `posture`
  - a `rejoin` key or value that names no declared state
  - the block on a descriptor without player movement
  - a mod whose distinct names exceed 255

  A minimal valid block loads, with and without `rejoin`.
- [ ] **AC2.** A pawn whose descriptor declares no life states behaves exactly as today at zero HP. Fire is blocked and cancelled, vault is barred (so teleporters are skipped), the pawn is excluded from trigger occupancy and is not a spatial damage target. Movement, look, switch, reload, Use, Touch and pickups stay ungated.

### Life state and gates
- [ ] **AC3.** A pawn spawns, joins and restarts the level in its descriptor's `initial` state with `player.lifeStateMs` at 0. Rejoin and level change follow AC14.
- [ ] **AC4.** For each capability key there are paired tests, one per direction. A state with the key `false` (or `speedScale: 0`) refuses the gated action. A state with the key `true` permits it at zero HP. This covers firing while downed, and covers it on the host, in single player, and in connected-client prediction. Movement and look keys hold across reconcile replay with no correction when host and client share the state.
- [ ] **AC5.** `movement.posture: "crouch"` holds the pawn crouched while it is in that state, even with crouch released. Leaving the state returns control to the crouch input under the normal stand-up clearance rule.
- [ ] **AC6.** `touch: false` blocks new trigger Touch entries and pickups. It does not emit Exit for a volume the pawn already occupies. `occupancy: false` removes the pawn from occupant counts.

### Verbs and reads
- [ ] **AC7.** `setLifeState(name)` from a player event applies in-tick. The next tick's gates read the new profile, and `player.lifeStateMs` restarts from 0. Setting the current state again is a no-op that keeps elapsed time.
- [ ] **AC8.** Install rejects any reaction whose `setLifeState`, `respawn({ lifeState })` or `inLifeState` names a state absent from the mod's name table. The error names the reaction and the state. A runtime target whose descriptor lacks that state, or declares no states, warn-skips.
- [ ] **AC9.** `inLifeState(name)` holds exactly while the evaluated pawn is in `name`. This applies in player-event conditions and in local `onStateCrossing` predicates (own pawn).
- [ ] **AC10.** `player.lifeState` (local String) and `player.lifeStateMs` reach the owning client's HUD. A HUD `visibleWhen` on `player.lifeState` equal to `"down"` shows only while downed. An undeclared pawn reads `player.lifeState` as `""`.

### Respawn and rejoin
- [ ] **AC11.** `respawn()` relocates the pawn to its seat's placement on the next fixed tick and applies all of these:
  - zeroes velocity and sets yaw to the placement facing
  - sets health to max and re-arms the death latch, so a later zero fires `playerDied` again
  - enters `initial` (or the given `lifeState`) with elapsed time 0
  - cancels weapon activations and charge, and drops any vault
  - keeps inventory
- [ ] **AC12.** The owning client snaps through the teleport reconcile class and observers cut instead of interpolating. No teleporter arrival sound plays, even when the pawn last used a teleporter.
- [ ] **AC13.** Respawn picks the seat placement unless another live pawn overlaps it. It then picks the lowest map-order placement that no pawn overlaps. With none free it uses the seat placement and logs one warning. `health` outside `(0, max]` or non-finite warn-clamps to max. A non-player target warn-skips.
- [ ] **AC14.** A pawn that disconnects in state S and reconnects inside the seat hold window enters `rejoin[S]` with `player.lifeStateMs` at 0. It enters `initial` instead when S has no `rejoin` entry, when S is absent from the current name table (a hot reload during the hold removed it), or when the hold expired. It rejoins at its seat placement, and carried health ≤ 0 restores max. A level change is not a rejoin: every pawn starts the next level in `initial`, held seats included. A pawn whose descriptor declares no states is unaffected.

### Co-op and lifecycle
- [ ] **AC15.** A late joiner sees an existing downed teammate's life state (`inLifeState` on the host, posture on the avatar) with no extra baseline work.
- [ ] **AC16.** After hot reload, a live pawn keeps its state by name. If the new descriptor removed that name, the pawn enters the new `initial` state with one warning.

### Client experience
- [ ] **AC17.** A connected client running into a transition (down with `speedScale: 0.3`, or back to a full-speed state) at 150 ms simulated round-trip time gets no teleport-class snap. The correction rides the ordinary reconcile smoothing class, and the camera eases into and out of forced crouch exactly as a crouch-key press does. Respawn is the only life-state path that snaps.
- [ ] **AC18.** On the owning client, the `player.lifeStateMs` HUD value advances every rendered frame and never moves backwards when a snapshot lands, except at a real transition or respawn.

### Orderings and consumer
- [ ] **AC19.** Every row of the Orderings table has a test asserting its expected outcome.
- [ ] **AC20.** The dev mod ships the down/bleed-out policy in TS and Luau twins, with a parity surface test:
  - zero HP → down: crawl, can fire, can't switch, use or touch, no occupancy
  - `inLifeState("down")` and `player.lifeStateMs` ≥ 15000 → respawn
  - a bleed-out bar and a downed vignette on the HUD

  A two-player headless test drives down → bleed-out → respawn, with the other pawn unaffected throughout.
- [ ] **AC21.** The docs need these changes:
  - `docs/scripting-reference.md` documents life states, the zero-HP rule (Prior commitments), the verbs, the reads, `rejoin` with both `rejoin: { down: "alive" }` and `rejoin: { down: "spectating" }` (a profile with everything off), and the Doom / Quake / Borderlands-lite recipes.
  - The verb tables in it and in `docs/teleporters.md` list `setLifeState` and `respawn`.
  - Every command shown runs from an SDK bundle.

## Orderings

Task agents cite rows by number. Expected outcomes are binding.

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Two pawns zeroed in one tick | Same death sweep | `becomes(health ≤ 0)` fires once per pawn, each with its own `on.player`. `playerDied` fires once, unchanged |
| O2 | Zeroed and healed in one tick (any in-tick heal that applies before the sweep sees zero) | In-tick | No death edge. `playerDied` fires if the sweep latched first; positive health re-arms it |
| O3 | Two `respawn` verbs on one pawn before the next tick | Queue | One relocation; the last request's arguments win |
| O4 | `respawn` and `setLifeState` on one pawn in one fire list | Listed order | `setLifeState` applies in-tick. Respawn applies next tick and sets its own state, which wins |
| O5 | `setLifeState` to the current state | — | No-op; elapsed time continues |
| O6 | Pawn disconnects while `down`, reconnects inside the hold window | Harvest, reclaim, restore | Enters `rejoin.down` when declared, else `initial`, elapsed 0, at the seat placement. Carried health ≤ 0 restores max. After hold expiry: fresh seat, `initial` |
| O7 | Level change while in a non-initial state | Harvest at unload | Next level starts in `initial` regardless of `rejoin`, with carried health (max if ≤ 0) |
| O8 | Hot reload mid-down, state kept / removed | Recompose | Kept: same state, elapsed continues. Removed: `initial`, elapsed 0, one warning |
| O9 | Client predicts through a transition | Host transition tick T | At most one reconcile correction at the state change. No repeated corrections while host and client share the state |
| O10 | Respawn requested on the tick the pawn uses a teleporter | Same tick | The teleporter resolves, then respawn relocates the next tick. The generation bumps twice and the client snaps to the respawn |
| O11 | Client fires in the window between the host's transition to `fire: false` and the client receiving it | Host tick T, client learns at T + RTT/2 | The host refuses those shots through the same path it uses for a zero-HP shooter today. The client shows no feedback class that path does not already show |
| O12 | Client fires in the window after the host moves it to a `fire: true` state but before the client knows | Same window, reversed | The client's own gate still refuses locally. No shot is sent, so nothing is refused and nothing is shown. Firing resumes when the state arrives |

## Tasks

New logic lands in new modules. Files already past ~800 lines (`sim/src/sim/mod.rs`, `physics/src/movement/mod.rs`, `netcode/src/lib.rs`, `netcode/src/client.rs`, `netcode/src/seat.rs`, `postretro/src/startup/lifecycle.rs`, `startup/reaction_validation.rs`, `netcode/src/state_slots.rs`, `scripting_systems/data_archetype.rs`, `scripting/builtins/net_descriptor.rs`) receive only call-site edits and match arms.

### Task 1: Thin slice: descriptor → state → verb → condition → wire → fire gate

This slice falsifies the boundary assumptions.

**Descriptor and validation**
- Add `lifeStates` as a descriptor component kind, `DescriptorComponentKind` plus `ComponentKind` / `ComponentValue`.
- Both VMs read it into a neutral raw form, `rejoin` included. One shared validator, modelled on `validate_charge_interrupts` (sorted keys, `DescriptorError::InvalidShape` with full path), applies AC1. The lowered `rejoin` map stays host-side and does not ride `TuningPayload`.

**Global name table and profiles**
- Build a sorted union of every declared name across `DataRegistry::entities` at manifest load. A state id is its index.
- The table and every archetype's profile, keyed by id, ride `TuningPayload`, with `TUNING_PAYLOAD_EPOCH` bumped to 16 and its fixture updated.

**Per-pawn state**
- Add `life_state: u8` (255 = undeclared) and `life_state_elapsed_ms: u32` to `PlayerMovementComponent`, beside `teleport`.
- Spawn paths set `initial`: `spawn_from_player_starts*`, and the netcode joiner spawn `host_handle_accept_descriptor_at_placement`. Task 5 extends these same sites for rejoin.
- Replicate both fields through `WirePlayerMovementState`, bumping `WIRE_VERSION` 35→36 and `SNAPSHOT_VERSION` 22→23. Late-join baselines carry them through `collect_payloads`.
- The movement tick advances elapsed time by the fixed step. Reconcile replay advances it identically.

**Capability helper**
- Add one helper, `player_capability(state_or_component, Capability) -> bool`, that resolves the profile, returning the HP-derived result for undeclared pawns.
- Wire it into the fire gates in this task: `pawn_alive` (local and remote weapon command stages), `host_activations::binding_is_live`, and the client's `apply_owner_liveness`, which reads the replicated state plus the `TuningPayload` profile.

**Verb and condition**
- Add `setLifeState(name)`:
  - SDK `playerVerbs` in `commands.ts` / `.luau`, plus the six type declarations, then regenerate.
  - A foundation payload check that `state` is an identifier.
  - Entries in `CONSEQUENTIAL_PRIMITIVES` and the debug mirror `is_trigger_consequential_primitive`.
  - `BoundTriggerCommand::SetLifeState { target, state_id }` and its executor.
  - The runtime registry handler for the post-`wait` group path.
  - Deny it on npc groups in `validate_group_kind_primitive`.
- Name validation for AC8 is one walk in `validate_reaction_bodies_pass_a`, with bind-time resolution as a fallback.
- Add the `inLifeState(name)` SDK function, lowering to `{op:"input", name:"player.inLifeState:<name>"}`. `PlayerConditionScope` resolves it at bind to (state id, Bool) and fails the bind on an unknown name.

**Test**
- A two-player host/client test: a player event downs a pawn, its fire is refused, the client reads the same state.

Delivers AC1, AC3, AC7, AC8, AC9 (condition arm).

### Task 2: Movement capabilities

- **Stamping.** The movement tick reads profile-derived capability fields stamped onto `PlayerMovementComponent`. Replay cannot read the registry; `vault_barred` is the precedent. Host, single player and the client stamp from the profile table at the tick the state applies, so replay reproduces them from the replicated `life_state`.
- **`movement.speedScale`** multiplies the per-state wish speed.
- **`movement.abilities: false`** blocks entry to jump, dash, slide, vault and crouch toggling. Vault keeps its existing barring path, `drop_vault_if_dead`, which becomes a capability read for declared pawns.
- **`movement.posture: "crouch"`** forces crouch intent. Crouch already rewrites `capsule.eye_height`, so fire origin, hit validation and AI line of sight follow without new code.
- **`look: false`** freezes view yaw/pitch input for the pawn on host and client. Existing camera feel layers still run.
- **Client smoothness.** A life-state change replicated to the owning client takes the ordinary reconcile correction class, never the teleport class. The teleport class stays reserved for respawn (Task 5) and teleporters. The posture change reuses the crouch view-height easing.
- Delivers AC4 (movement and look keys), AC5, AC17 and O9.

### Task 3: Interaction and combat capabilities

Route each key through the Task 1 helper at its existing site.
- **`switchWeapon` and `reload`:** the switch and reload request gates on host, single player and the client weapon frame. These are ungated at zero HP today.
- **`use`:** trigger Use edges in `TriggerSystem::run_authoritative_tick_with_dispatch`.
- **`touch`:** new Touch entries there and pickups in `touch.rs`. Gate the entry, never membership, so no false Exit fires.
- **`occupancy`:** `player_is_alive_for_trigger_occupancy` / `effective_occupants`.
- **`damageable`:** `is_damage_target_eligible` (`sim/src/scripting_systems/health.rs`), branching only for pawns carrying player movement so enemies are untouched. It gates spatial eligibility and AI attack exactly where zero HP does today. Hitscan apply, mover crush, impact effects and script `damage` stay ungated, as they are at zero HP today.
- **Fire across a transition.** A remote shot refused by a life-state transition the client had not yet received uses the host's existing zero-HP refusal path unchanged (O11). The client weapon frame gates on its own replicated state (O12).
- Put the per-site branches in a new `sim/src/sim/player_life.rs` and edit only call sites in `sim/mod.rs`.
- Delivers AC2, AC4 (these keys), AC6, O11 and O12.

### Task 4: Life-state reads and reload remap

- **`player.lifeStateMs`.** Add a `PlayerSlot` variant read from the component's elapsed field: catalog entry with `OwnerPrivatePlayer`, `player_slots.rs` arm, HUD publisher beside `publish_swim_values`, `MIRRORED_GAME_STATE_LEAVES`, regenerated types and fixtures. This makes 15 player slots; the u16 bitset cap holds.
- **Client HUD value.** On a connected client, the HUD publisher writes `player.lifeStateMs` from the owning pawn's *predicted* `life_state_elapsed_ms`, which advances every fixed tick and reconciles with movement. It never writes from the stepped owner-private sample. The owner-private copy stays the host's per-player source for conditions (AC18).
- **`player.lifeState`.** Add a local String catalog slot with `ReplicationScope::None` (precedent `player.weapon.current`). Each machine publishes it for its own pawn from the replicated `life_state` and the `TuningPayload` name table; an undeclared pawn reads `""`.
- **`inLifeState` in local crossings.** Teach `StoreScope` the `player.inLifeState:` input so local `onStateCrossing` predicates read the own pawn (precedent `read(player.swimming)`).
- **Hot reload remap.** Rebuild the name table and remap each live pawn by name in the descriptor refresh path (`plan_descriptor_refresh`). A removed name enters `initial` with one warning.
- Delivers AC9 (crossing arm), AC10, AC16, AC18 and O8.

### Task 5: Respawn and rejoin

**SDK and binding**
- Add `respawn({ health?, lifeState? })` to `playerVerbs` and the type declarations.
- Add `BoundTriggerCommand::Respawn`. It queues a `RespawnRequest { pawn, health, state_id }`; a second request for the same pawn replaces the first (O3).

**Placements**
- Netcode depends on sim, so the app publishes resolved placements into a sim-readable `RespawnPlacements` table: seat → placement transform, plus the map-order transforms of `player_spawn::joinable_spawns` (the join path's eligibility, so respawn and join agree on which spawns exist).
- Single player publishes at install. The host publishes at install and on every `SeatTable::assign_placement`.

**Relocation, in the next fixed tick's movement stage** (through the `pending_teleporter_mutations` seam, so the host camera hold window sees it)
- Pick the placement per AC13. The overlap test is origin distance under the sum of capsule radii with overlapping vertical extents, against other live pawns. No physics query exists for pawn-vs-pawn.
- Set position, yaw and zero velocity.
- Bump the teleport generation with `last_teleporter = NO_TELEPORTER`, rebuild `arrival_latches` at the destination, and adjust `accumulated_yaw`.
- Set health through `set_health_absolute`.
- Set the state and reset elapsed time.
- Cancel owned activations and charge, and drop any vault.

**Rejoin** (AC14)
- `CarriedState` (`combat-model/src/carried_loadout.rs`) gains `life_state: Option<String>`, the leaving state's *name*.
- `SeatTable::harvest_pawn` records it on the disconnect path. It gains a name-table parameter (`&[String]`, the Task 1 table); its disconnect caller in `netcode/src/host.rs` passes the table the host already holds to build `TuningPayload`. An undeclared pawn records `None`. The id → name lookup lives in `player_life.rs`; `seat.rs` gets only the new `CarriedField` match arm.
- `SeatTable::harvest_bound_pawns` is the level-change harvest. It records no name and clears `life_state` on every carried record, held seats included, so the next level starts in `initial` (O7).
- Hold expiry already releases the seat and mints a fresh one with no carried record, so a late reconnect starts in `initial` with no new code.
- Restore runs at the Task 1 spawn sites beside each `restore_carried_health` call (`data_archetype.rs` player-start path, `net_descriptor.rs` net-slot path). A new `restore_carried_life_state` in `player_life.rs` resolves a carried name through the pawn descriptor's `rejoin` map, then resolves the target against the current name table. A missing name, a missing `rejoin` entry, or an unresolvable target yields `initial`. Elapsed time starts at 0. Health keeps `restore_carried_health` unchanged.

**Tests**
- AC11–AC14 and O3, O4, O6, O7, O10, including a rejoin across a hot reload that removed the carried name.

Delivers AC11, AC12, AC13, AC14, O3, O4, O6, O7 and O10.

### Task 6: Dev-mod down/bleed-out consumer, co-op lifecycle tests and docs

**Content** (new `content/dev/scripts/player-death.ts` + `.luau` twin, registered in `start-script.ts`)
- **Player descriptor** (`content/dev/scripts/player.ts`):
  - `alive {}`
  - `down { movement: { speedScale: 0.3, abilities: false, posture: "crouch" }, switchWeapon: false, use: false, touch: false, occupancy: false }`
  - No `rejoin` entry, so a rejoin enters `alive`.
- **Player events:**
  - zero HP → `down`
  - `inLifeState("down")` and `lifeStateMs ≥ 15000` → `respawn()`
- **HUD:** a bleed-out bar on `player.lifeStateMs` visible while `player.lifeState` equals `"down"`, and a downed vignette from a local crossing on `inLifeState("down")`.

**Tests**
- A parity surface test (`*_surface_tests.rs` pattern).
- The two-player test on `player_events::tests::World`, driving the AC20 path.
- Lifecycle rows: AC3, AC15, O1, O2, O5.
- **Manual proof**, recorded in the PR: a two-client playtest of the dev mod at 150 ms RTT (`POSTRETRO_NET_LAG`) on `coop-presentation-test`. Check AC17 and AC18 from the client's seat, covering the cases below. Record each run's lag settings and what was seen.
  - going down mid-sprint
  - crawling and shooting
  - bleed-out → respawn

**Docs**
- Update the docs per AC21.

Delivers AC15, AC19 (residual rows), AC20, AC21, and the manual proof for AC17 and AC18.

## Sequencing

**Phase 1 (sequential):** Task 1. The thin slice falsifies the descriptor, wire, verb and condition boundary assumptions, and every later task consumes its component fields, name table and helper.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4. Each extends a different seam off Task 1's helper and fields. Task 2 owns movement files, Task 3 owns sim gate sites, Task 4 owns the slot catalog and refresh.
**Phase 3 (sequential):** Task 5. It consumes Task 4's elapsed reset semantics and adds to Task 3's `player_life.rs`.
**Phase 4 (sequential):** Task 6. It consumes every surface.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| Life-state component | `DescriptorComponentKind::LifeStates` | `"lifeStates"` (`initial`, `states`, `rejoin`) | `components.lifeStates` | `components.lifeStates` |
| Profile keys | `LifeStateProfile` fields | `movement.speedScale`, `movement.abilities`, `movement.posture` (`"stand"`/`"crouch"`), `look`, `fire`, `switchWeapon`, `reload`, `use`, `touch`, `occupancy`, `damageable` | same | same |
| Rejoin map | lowered map of state ids, host-only | `rejoin: { <fromState>: "<toState>" }` | same | same |
| Set verb | `BoundTriggerCommand::SetLifeState` | primitive `"setLifeState"`, args `{ state }` | `setLifeState(name)` | `setLifeState(name)` |
| Respawn verb | `BoundTriggerCommand::Respawn` | primitive `"respawn"`, args `{ health?, state? }` | `respawn({ health?, lifeState? })` | `respawn({ health = , lifeState = })` |
| Condition input | parsed in each scope | `{op:"input", name:"player.inLifeState:<name>"}` | `inLifeState(name)` | `inLifeState(name)` |
| Slots | `PlayerSlot::LifeStateMs` | `"player.lifeStateMs"`, `"player.lifeState"` | `player.lifeStateMs`, `player.lifeState` | same |

## Wire format

- **`WirePlayerMovementState`** appends `life_state: u8` (255 = undeclared) then `life_state_elapsed_ms: u32`, little-endian. Field order follows the existing `teleport` record. `SNAPSHOT_VERSION` goes 22→23 and `WIRE_VERSION` 35→36.
- **`TuningPayload`** (epoch 16) adds `life_state_names: Vec<String>` (sorted, ≤ 255) and per-archetype `life_state_profiles: Vec<(u8, LifeStateProfile)>`, serialized like existing tuning entries.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1. One capability source: every gate on a declared pawn reads its current profile; undeclared pawns keep today's HP-derived gates | Task 1 (helper) | Task 2, 3 gate sites; any new gate added later | AC2, AC4, AC6 |
| I2. Host, single player and client prediction derive capabilities from the same data: replicated `life_state` + `TuningPayload` profiles | Task 1 | Task 2 stamping in replay; client weapon frame | AC4, O9 |
| I3. The engine never changes life state on HP. Only `initial` at spawn/join/restart, the `rejoin` map at restore, `setLifeState`, `respawn` and hot-reload remap move it | Task 1 | Task 4 (remap), Task 5 (respawn, rejoin) | AC3, AC7, AC14, AC16 |
| I4. Elapsed time resets on every actual transition, on respawn and on rejoin; a same-state set preserves it | Task 1 | Task 4, 5 | AC7, AC14, O5, O8 |
| I5. A respawn relocates exactly once per request batch through the teleport generation, with no teleporter cue | Task 5 | Same-tick teleporter use | AC12, O3, O10 |
| I6. The death latch contract is unchanged: `playerDied` fires once per latch, re-armed by positive health | Existing | Task 5 health write | AC11, O1, O2 |
| I7. Client smoothness: no life-state transition reaches the owning client as a teleport-class snap except respawn; the bleed-out time the client watches is predicted locally | Task 2, 4 | Task 5 must keep the teleport class to respawn only; any later state-driven relocation | AC17, AC18, O9 |

## Script syntax examples

**Borderlands-lite (dev mod, TS).** Down, crawl and shoot, bleed out to respawn.

```ts
// player descriptor
components: {
  health: { max: 100 },
  lifeStates: {
    initial: "alive",
    states: {
      alive: {},
      down: {
        movement: { speedScale: 0.3, abilities: false, posture: "crouch" },
        switchWeapon: false, use: false, touch: false, occupancy: false,
      },
    },
  },
}

// player-death.ts
const { player } = getGameState();
export const playerEvents = [
  players().on(becomes(read(player.health).le(0)), [goDown]),        // goDown: on.player.setLifeState("down")
  players().on(becomes(inLifeState("down").and(read(player.lifeStateMs).ge(15000))),
               [bleedOut]),                                          // bleedOut: on.player.respawn()
];
```

**Quake co-op (TS).** Dead players respawn after 2 s. A player who reconnects while dead spectates until the author moves them on.

```ts
lifeStates: { initial: "alive", rejoin: { dead: "spectating" }, states: {
  alive: {},
  dead: { movement: { speedScale: 0, abilities: false, posture: "crouch" },
          fire: false, switchWeapon: false, reload: false, use: false, touch: false,
          occupancy: false, damageable: false },
  spectating: { movement: { speedScale: 0, abilities: false }, look: false,
                fire: false, switchWeapon: false, reload: false, use: false, touch: false,
                occupancy: false, damageable: false },
} }

players().on(becomes(read(player.health).le(0)), [die]);   // on.player.setLifeState("dead")
players().on(becomes(inLifeState("dead").and(read(player.lifeStateMs).ge(2000))), [respawnNow]);
```

**Doom single player (Luau).** A local crossing opens the death screen on the dying player's own machine.

```lua
players():on(becomes(read(player.health):le(0)), { die })   -- on.player:setLifeState("dead")
onStateCrossing(inLifeState("dead"), { openMenu("deathScreen") })  -- buttons: restartLevel(), returnToFrontend()
```

## Open questions

- **Player-descriptor-composition draft** is live but needs a rebase: only its file split shipped; per-state movement tuning, modifiers under states and layered viewFeel are unbuilt. When it lands, `movement.speedScale` may become a reference to a movement modifier instead of a bare scale. Neither spec blocks the other.
- **`coop--spawn-placement-occupancy` draft** is mostly shipped (`0d0aa24`, `40d39a5`); it does not block respawn placement, which uses the same `joinable_spawns` eligibility the join path uses.
