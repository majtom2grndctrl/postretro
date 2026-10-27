# Player Life States

## Goal

Give mod authors an engine floor for player death and respawn, so they express death mechanics as policy: Quake co-op respawn, Doom restart, or a Borderlands-style down state with bleed-out. The engine adds author-named **life states** that gate an engine-closed capability set, reads for conditions and the HUD, verbs to change state and respawn, and an author map for the state a rejoining player enters. It adds no death policy. The dev mod ships down → bleed-out → respawn as the first consumer. Connected clients feel it as smoothly as the host: no transition snaps the view except respawn, and the bleed-out bar advances every frame. This is the co-op floor E18-R (respawn and player-leave) names. Player-leave keeps the existing harvest-and-hold contract; this spec adds only the life state a rejoining player enters (AC14).

## Scope

### In scope

- `components.lifeStates` on an entity type descriptor, as a descriptor field rather than a component kind: author-named states, an `initial` state, per-state capability profiles, and an optional `rejoin` map. One validator serves both runtimes.
- A global life-state name table, sorted, that rides `TuningPayload` with the receiving pawn's profiles.
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
- Rejoin: every participation exit (disconnect or slot demotion) harvests the leaving state by name, and restore enters the author's `rejoin` target.
- Hot-reload remap of live pawns by state name.
- Dev-mod down/bleed-out policy and its HUD.
- Updates to `docs/scripting-reference.md` and `docs/teleporters.md`.

### Out of scope

Later phases build on this shape. Each is additive to this surface.

- **Next: `E18--player-revive-assist`.** Teammate assist (the `assist` input command, the `assistable` profile key, assist sources and progress) and impact-policy life-state reads and effects (`source.inLifeState`, `source.setLifeState`). It depends on this spec and lands before the phases below.
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
- [ ] **AC4.** For each capability key there are paired tests, one per direction. A state with the key `false` (or `speedScale: 0`) refuses the gated action. A state with the key `true` permits it at zero HP. This covers firing while downed, held, burst and charge actions included, and covers it on the host, in single player, and in connected-client prediction. Movement and look keys hold across reconcile replay with no correction when host and client share the state.
- [ ] **AC5.** `movement.posture: "crouch"` holds the pawn crouched while it is in that state, even with crouch released. Leaving the state returns control to the crouch input under the normal stand-up clearance rule.
- [ ] **AC6.** `touch: false` blocks new trigger Touch entries and pickups. It does not emit Exit for a volume the pawn already occupies. `occupancy: false` removes the pawn from occupant counts.

### Verbs and reads
- [ ] **AC7.** `setLifeState(name)` from a player event applies in-tick. The next tick's gates read the new profile, and `player.lifeStateMs` restarts from 0. Setting the current state again is a no-op that keeps elapsed time. `player.lifeStateMs` reads whole milliseconds in the state, floored from the tick count with no per-tick rounding: at 60 Hz, `≥ 15000` first holds on the 900th tick in the state (O15).
- [ ] **AC8.** Install rejects any reaction whose `setLifeState`, `respawn({ lifeState })` or `inLifeState` names a state absent from the mod's name table. The error names the reaction and the state. A runtime target whose descriptor lacks that state, or declares no states, warn-skips.
- [ ] **AC9.** `inLifeState(name)` holds exactly while the evaluated pawn is in `name`. This applies in player-event conditions and in local `onStateCrossing` predicates (own pawn).
- [ ] **AC10.** `player.lifeState` (local String) and `player.lifeStateMs` reach the owning client's HUD. A HUD `visibleWhen` on `player.lifeState` equal to `"down"` shows only while downed. An undeclared pawn reads `player.lifeState` as `""`.

### Respawn and rejoin
- [ ] **AC11.** `respawn()` relocates the pawn to its seat's placement at the start of the next fixed tick and applies all of these:
  - zeroes every velocity channel, ends any banked dash or slide boost, and settles ground at the placement, so no mover carry follows
  - ends the respawn tick facing the placement's yaw
  - sets health to max and re-arms the death latch, so a later zero fires `playerDied` again
  - enters `initial` (or the given `lifeState`) with elapsed time 0
  - cancels weapon activations and charge, and drops any vault
  - keeps inventory
- [ ] **AC12.** The owning client snaps through the teleport reconcile class and observers cut instead of interpolating. No teleporter arrival sound plays, even when the pawn last used a teleporter.
- [ ] **AC13.** Respawn picks the seat placement unless another live pawn overlaps it. It then picks the lowest map-order placement that no pawn overlaps. Respawns on one tick pick in seat order, each against the positions earlier picks left. With none free it uses the seat placement and logs one warning. `health` outside `(0, max]` or non-finite warn-clamps to max. A non-player target warn-skips.
- [ ] **AC14.** A pawn that leaves participation in state S and returns enters `rejoin[S]` with `player.lifeStateMs` at 0. Leaving and returning is either of:
  - a disconnect, then a reconnect inside the seat hold window
  - a slot demotion (a client suspend, or a mod-digest hold after a host hot reload), then re-promotion

  It enters `initial` instead when S has no `rejoin` entry, when S is absent from the current name table (a hot reload removed it), or when the hold expired. It returns at its seat placement, and carried health ≤ 0 restores max. A host suspend and resume keeps every held seat's carried state. A level change is not a rejoin: every pawn starts the next level in `initial`, held seats included. A pawn whose descriptor declares no states is unaffected.

### Co-op and lifecycle
- [ ] **AC15.** A late joiner sees an existing downed teammate's life state (`inLifeState` on the host, posture on the avatar) with no extra baseline work.
- [ ] **AC16.** After hot reload, a pawn that stays materialized keeps its state by name, with elapsed time continuing, and gates by that state's new profile. This covers the host's own pawn, single player, and any remote pawn the reload does not demote. If the new descriptor removed the name, the pawn enters the new `initial` state with one warning. A remote pawn whose slot the reload demotes on the mod digest is harvested and despawned. Its re-promotion is a rejoin and follows AC14 (O23).

### Client experience
- [ ] **AC17.** A connected client running into a transition at 150 ms simulated round-trip time gets no teleport-class snap. The cases are down with `speedScale: 0.3` from a full run, and back to a full-speed state.
  - The correction takes a smoothed class (`CorrectionClass::Ordinary`, `Dash` or `OversizedSmoothed`, by size), never `Teleport`.
  - The camera eases into and out of forced crouch exactly as a crouch-key press does.
  - Respawn is the only life-state path that snaps.
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
| O4 | `respawn` and `setLifeState` on one pawn in one fire list | Listed order | `setLifeState` applies in-tick. Respawn applies at the next tick's start and sets its own state, which wins |
| O5 | `setLifeState` to the current state | — | No-op; elapsed time continues |
| O6 | Pawn disconnects while `down`, reconnects inside the hold window | Harvest, reclaim, restore | Enters `rejoin.down` when declared, else `initial`, elapsed 0, at the seat placement. Carried health ≤ 0 restores max. After hold expiry: fresh seat, `initial` |
| O7 | Level change while in a non-initial state | Harvest at unload, then the unload clear | Next level starts in `initial` regardless of `rejoin`, with carried health (max if ≤ 0) |
| O8 | Hot reload mid-down, state kept / removed, pawn stays materialized | Recompose | Kept: same state, elapsed continues, the new profile's gates. Removed: `initial`, elapsed 0, one warning. A demoted remote pawn follows O23 |
| O9 | Client predicts through a transition | Host transition tick T | At most one reconcile correction at the state change. No repeated corrections while host and client share the state |
| O10 | Respawn requested on the tick the pawn uses a teleporter | Same tick | The teleporter resolves, then respawn relocates at the next tick's start. The generation bumps twice and the client snaps to the respawn |
| O11 | Client fires in the window between the host's transition to `fire: false` and the client receiving it | Host tick T, client learns at T + RTT/2 | The host refuses those shots through the same path it uses for a zero-HP shooter today. The client shows no feedback class that path does not already show |
| O12 | Client fires in the window after the host moves it to a `fire: true` state but before the client knows | Same window, reversed | The client's own gate still refuses locally. No shot is sent, so nothing is refused and nothing is shown. Firing resumes when the state arrives |
| O13 | Client merges the first record carrying a new state (`down`, `speedScale: 0.3`) | Snapshot k: merge, re-stamp, replay; then snapshot k+1 | One smoothed correction at k. The correction at k+1 is no larger than one with no transition |
| O14 | Hot reload edits the profile of the pawn's current state, name kept | Commit at frame tail; host re-stamps at commit; client re-stamps at its first merge after the new `TuningPayload` | Same state, elapsed continues. No correction beyond the one at that merge |
| O15 | First elapsed value a condition sees after each entry, at 60 Hz | Player-event `setLifeState` at T; respawn queued at T, drained at T+1 start; rejoin spawn; trigger-reaction `setLifeState` in tick T's Triggers stage | Player-event entry: the T+1 pass reads 16. Respawn: the T+1 pass reads 16 and T+1's gates read the respawn state. Rejoin: the first pass reads 16. Trigger entry: the T pass reads 0 |
| O16 | Trigger reaction `setLifeState` into a `fire: false`, `speedScale: 0` state | Tick T Triggers stage, before Weapons | T's Weapons stage refuses fire and cancels that pawn's activations, host-local and remote alike. Triggers-stage gates (Use, Touch, occupancy) read the profile as of that stage's start for the whole pass. Movement and look apply from T+1 |
| O17 | Respawn drained while the player holds forward and turns | T+1 start: teleporter drain, respawn drain, movers, fire-gate capture, movement | At the end of T+1, velocity is one tick of acceleration from rest along the placement facing and transform yaw equals the placement facing. The host camera holds and turns by the respawn delta |
| O18 | Two pawns' respawns drain at one tick start; a third pawn stands on the later seat's placement | One drain, seat order | The earlier seat takes its own placement. The later seat takes the lowest placement still free after that relocation. No two respawned pawns share a placement while one is free |
| O19 | `respawn({ lifeState: "spectating" })` queued at T; a hot reload commits at frame tail and shifts ids | Verb at T, commit, drain at T+1 | Enters `spectating` by name. If the reload removed it: `initial`, one warning. The request is never dropped |
| O20 | Respawn queued on the last tick before a level change | Verb, unload, next level install | Unload clears the queue. Nothing relocates in the next level; every pawn starts in `initial` |
| O21 | Respawn queued at T; the pawn's client disconnects or is demoted at the next frame's poll | Verb at T, harvest at poll, drain at T+1 | Harvest records the pre-respawn state. The drain skips the missing pawn with one warning. Rejoin follows AC14 from the harvested state |
| O22 | P2 disconnects while `dead` (`rejoin: { dead: "spectating" }`); the host suspends and resumes; P2 reconnects inside the hold | Harvest, hold, suspend harvest, resume, reclaim, restore | Enters `spectating`, elapsed 0. Only level unload clears carried names |
| O23 | A host hot reload changes the mod digest while P2 is `down` at 10 s | Commit, parity demote, harvest and despawn, re-promote, net-slot restore | A rejoin: `rejoin.down` else `initial`, elapsed 0, at the seat placement. Carried health ≤ 0 restores max |
| O24 | The host trims a client's command backlog past its catch-up bound while the client shows `lifeStateMs` | Host trim, snapshot acks a jumped cursor, replay | The displayed value holds, then rises. It falls only at a real reset: a change of pawn, replicated state or teleport generation, or replicated elapsed falling between consecutive snapshots |
| O25 | Render rate above tick rate, frames with zero ticks included | Per-frame display over a per-tick value | The displayed `player.lifeStateMs` rises on every frame (tick value plus alpha times the step) on host, single player and client |
| O26 | A hot reload shifts ids; the Control datagram with the new `TuningPayload` is lost and resent while snapshots arrive | Unreliable snapshots before reliable, resent Control | The client keeps its current life-state fields for records whose revision differs, and adopts once the matching payload lands. No shifted id is resolved under the old table |
| O27 | A pawn enters `x` at T and leaves at T+1, shorter than the snapshot interval | Host ticks T and T+1; snapshots every second tick | Host conditions see `x` for one pass. The owning client and observers may never see `x`, and client-local crossings on `inLifeState("x")` may not fire |
| O28 | A pawn uses a teleporter at T; respawn drains at T+1; one observer snapshot covers both | Coalesced observer sample, last teleporter none | The observer cuts once and plays no cue. The host seat records per tick and plays the T cue. The owning client plays the T cue only from its own forward prediction |
| O29 | A trigger arms a teleporter whose region contains a respawn placement, on the tick a respawn to it is queued | Both drain at T+1 start, teleporter mutations first | The respawn's latch rebuild includes the newly armed teleporter. The pawn does not teleport until it leaves and re-enters |

## Tasks

New logic lands in new modules. Files already past ~800 lines (`sim/src/sim/mod.rs`, `physics/src/movement/mod.rs`, `netcode/src/lib.rs`, `netcode/src/client.rs`, `netcode/src/seat.rs`, `postretro/src/startup/lifecycle.rs`, `startup/reaction_validation.rs`, `netcode/src/state_slots.rs`, `scripting/builtins/data_archetype.rs`, `scripting/builtins/net_descriptor.rs`, `postretro/src/main.rs`) receive only call-site edits, match arms and the small additions a task names.

### Task 1: Thin slice: descriptor → state → verb → condition → wire → fire gate

This slice falsifies the boundary assumptions.

**Descriptor and validation**
- Add `lifeStates` as an `EntityTypeDescriptor` field (`life_states`) beside `movement`, not a `DescriptorComponentKind`. That enum maps every kind to a runtime `ComponentKind` and a replication role, and life-state data has no runtime component: per-pawn state lives on `PlayerMovementComponent`, and profiles ride `TuningPayload`.
- Both VMs read it into a neutral raw form, `rejoin` included. One shared validator, modelled on `validate_charge_interrupts` (sorted keys, `DescriptorError::InvalidShape` with full path), applies AC1. The lowered `rejoin` map stays host-side and does not ride `TuningPayload`.

**Global name table and profiles**
- Build a sorted union of every declared name across `DataRegistry::entities` at manifest load. Store it as `DataRegistry::life_state_names` beside `entities`, with `DataRegistry::life_state_table_revision: u8` starting at 0. Level unload keeps both; Task 4 rebuilds them at hot reload. A state id is its index. Bind-time name resolution, `TuningPayload` construction, seat harvest and the reload remap all read this one table.
- Lowering then records each declared state's id in its descriptor's lowered `life_states` block, `initial` and the `rejoin` map included. Host and single-player gates, the spawn sites and rejoin restore resolve names and profiles from that block, with no table lookup.
- `TuningPayload` is built per pawn (`tuning_payload_for_pawn`). It carries the full name table, the receiving pawn's own archetype profiles keyed by id, and the table revision (**Wire** below). Observers run no gates, so no other archetype's profiles ride it.

**Per-pawn state**
- Add `life_state: u8` (255 = undeclared), `life_state_elapsed_ticks: u32` and `life_state_table_revision: u8` to `PlayerMovementComponent`, beside `teleport`. Elapsed counts whole fixed ticks; `player.lifeStateMs` converts it per AC7 in integer arithmetic.
- **One write path.** Add `enter_life_state(component, id, profile, revision)` beside the component. It sets the id and revision, zeroes elapsed, then calls `restamp_life_state(component, profile)`. `restamp_life_state` re-derives the capability stamps Task 2 adds; this task ships it with no stamped fields. Every `life_state` write goes through this pair: spawn and the verb here, the client merge below, and later the reload remap (Task 4), the respawn drain and rejoin restore (Task 5).
- Spawn paths enter `initial`: `spawn_from_player_starts*`, and the netcode joiner spawn `host_handle_accept_descriptor_at_placement`. Task 5 extends these same sites for rejoin.
- The movement tick adds one to `life_state_elapsed_ticks`. Reconcile replay adds identically.

**Wire**
- `WirePlayerMovementState` (bitcode `Encode`/`Decode`) gains the **Per-pawn state** fields, same types and order, after `aim_pitch`. `SNAPSHOT_VERSION` goes 22→23 and `WIRE_VERSION` 35→36. `movement_state_to_wire` writes all three. Late-join baselines carry them through `collect_payloads`.
- `merge_wire_into_movement_state_checked` copies them onto the predicted component before replay, which also carries them through `carry_live_tick_state`. It then calls `restamp_life_state` from the `TuningPayload` profile, so replay runs on the merged state's stamps.
- **Revision gate.** Control is reliable and Snapshot is not, so a rebuilt table's payload and the first snapshot carrying its ids land in either order. The client keeps the previous payload's life-state names and profiles until its own pawn carries the installed revision. Every client resolution of the own pawn's `life_state` (the helper, `restamp_life_state`, `player.lifeState`) uses the table whose revision equals the component's `life_state_table_revision` (O26).

  | Arrival | Merge | Resolution |
  |---|---|---|
  | Record revision equals the installed payload's | Copies the record's life-state fields, then restamps | Installed table |
  | Record revision differs (snapshot before payload, or a record in flight after it) | Keeps the component's life-state fields and stamps | Table matching the component's revision |
  | Payload installs while the component holds the previous revision | None; the tuning rebuild carries the fields through `carry_live_tick_state` and restamps | Previous table until a record at the installed revision merges, then the previous table drops |
- `TuningPayload` (`TUNING_PAYLOAD_EPOCH` 15→16, fixture updated) adds `life_state_names: Vec<String>` (sorted, ≤ 255), `life_state_profiles: Vec<(u8, LifeStateProfile)>` and `life_state_table_revision: u8`, JSON-serialized like its existing fields.

**Capability helper**
- Add one helper, `player_capability(state_or_component, Capability) -> Option<bool>`, that resolves the profile for a boolean key. Its profile source is the pawn descriptor's lowered block on host and single player, and the `TuningPayload` profiles on a client. It returns `None` for an undeclared pawn, and each gate site keeps its own HP-derived predicate on `None`. Those predicates differ per site, and AC2 holds each one exactly. `speedScale` and `posture` are values, stamped by Task 2.

  | Pawn | Helper | Site verdict |
  |---|---|---|
  | Declares states, committed to removal | not consulted | Refused |
  | Declares states, key `true` | `Some(true)` | Permitted at any HP |
  | Declares states, key `false` | `Some(false)` | Refused at any HP |
  | Declares no states (`life_state` 255), committed to removal or not | `None` | The site's existing predicate (`pawn_alive`, `is_quiescent`, `player_is_alive_for_trigger_occupancy`, `is_damage_target_eligible`), its removal handling included, unchanged |

- Wire it, with the `fire` key, into every HP-derived fire site in this task:
  - `pawn_alive` (local and remote weapon command stages)
  - `host_activations::binding_is_live`
  - the death sweep's `cancel_owned_weapon_activations` call in `sweep_deaths`, which cancels a zero-HP pawn's executions on every sweep, latched or not
  - the client weapon frame's `alive` check in `predict_with_liveness`
  - the client's `apply_owner_liveness`, which reads the replicated state plus the `TuningPayload` profile
  - the charge HUD's zero-HP check in `weapon_charge_values`, so a downed pawn that can fire shows its charge

**Verb and condition**
- Add `setLifeState(name)`:
  - SDK `playerVerbs` in `commands.ts` / `.luau`, plus the six type declarations, then regenerate.
  - A foundation payload check that `state` is an identifier.
  - Entries in `CONSEQUENTIAL_PRIMITIVES` and the debug mirror `is_trigger_consequential_primitive`.
  - `BoundTriggerCommand::SetLifeState { target, state_id }` and its executor, which calls `enter_life_state` and is a no-op when the pawn is already in that state (AC7).
  - The runtime registry handler for the post-`wait` group path.
  - Deny it on npc groups in `validate_group_kind_primitive`.
- Name validation for AC8 is one walk in `validate_reaction_bodies_pass_a`, with bind-time resolution as a fallback.
- Add the `inLifeState(name)` SDK function, lowering to `{op:"input", name:"player.inLifeState:<name>"}`. `PlayerConditionScope` resolves it at bind to (state id, Bool) and fails the bind on an unknown name.

**Test**
- A two-player host/client test: a player event downs a pawn, its fire is refused, the client reads the same state.
- A downed `fire: true` pawn at zero HP keeps a held, burst and charge execution across death sweeps.
- O15's player-event and trigger-reaction arms, and O26.

Delivers AC1, AC3, AC4 (`fire` key), AC7, AC8 (`setLifeState` and `inLifeState` arms), AC9 (condition arm), O15 (player-event and trigger arms) and O26.

### Task 2: Movement capabilities

- **Stamping.** The movement tick reads profile-derived capability fields stamped onto `PlayerMovementComponent`. Replay cannot read the registry; `vault_barred` is the precedent for the field, not its timing: it is stamped only on forward ticks, so replay runs on the pre-merge value. Stamps are a pure function of `life_state` and its profile. Add the fields and fill the body of Task 1's `restamp_life_state`. Every `life_state` write calls it: spawn, the verb, the client merge before replay, the reload remap, the respawn drain and rejoin restore. Never stamp on forward ticks only. Replay and forward prediction then read the same stamps (O13).
- **`movement.speedScale`** multiplies the per-state wish speed.
- **`movement.abilities: false`** blocks entry to jump, dash, slide, vault and crouch toggling. Vault keeps its existing barring path, `drop_vault_if_dead`, which becomes a capability read for declared pawns.
- **`movement.posture: "crouch"`** forces crouch intent. Crouch already rewrites `capsule.eye_height`, so fire origin, hit validation and AI line of sight follow without new code.
- **`look: false`** freezes view yaw/pitch input for the pawn on host and client. Existing camera feel layers still run.
- **Client smoothness.**
  - A life-state change replicated to the owning client takes a smoothed reconcile class, never the teleport class. Nothing forces the class: it holds by magnitude, and AC17's 150 ms test pins it. The teleport class stays reserved for respawn (Task 5) and teleporters.
  - A forced posture change reaches the client a round trip late. Replay re-runs the crouch eye easing from the host transition tick, so the replayed `capsule.eye_height` lands mostly eased. The owning client therefore presents eye height through its own ease, at the installed tuning's `crouch.transition_rate`. The ease runs from the eye height presented before that reconcile toward the replayed `capsule.eye_height`.
  - Replay, fire origin and the host never read the presented value.
- Delivers AC4 (movement and look keys), AC5, AC17, O9 and O13.

### Task 3: Interaction and combat capabilities

Route each key through the Task 1 helper at its existing site. The helper returns `None` for an undeclared pawn, and the site then keeps its existing predicate unchanged (AC2).
- **`switchWeapon` and `reload`:** the switch and reload request gates on host, single player and the client weapon frame. These are ungated at zero HP today.
- **`use`:** trigger Use edges in `TriggerSystem::run_authoritative_tick_with_dispatch`.
- **`touch`:** new Touch entries there and pickups in `touch.rs`. Gate the entry, never membership, so no false Exit fires.
- **`occupancy`:** `player_is_alive_for_trigger_occupancy` / `effective_occupants`.
- **`damageable`:** the HP-derived target sites below, each branching only for pawns carrying player movement so enemies are untouched:
  - `is_damage_target_eligible` (`sim/src/scripting_systems/health.rs`): spatial eligibility and AI contact attacks
  - the AI weapon-aim target recheck in `ai/src/apply.rs`, which refuses a zero-HP target through `is_quiescent`
  - remote projectile hit validation in `netcode/src/lib.rs`, which refuses a zero-HP target

  | Target pawn | Verdict |
  |---|---|
  | Committed to removal or inventory-owned | Refused |
  | Undeclared | Today's HP test |
  | Declared | Current profile's `damageable` |

  Hitscan apply, mover crush, impact effects and script `damage` stay ungated, as they are at zero HP today.
- **Stage timing.** A trigger reaction's `setLifeState` lands in tick T's Triggers stage. Triggers-stage gates (Use, Touch, occupancy) read the profile as of that stage's start for the whole pass. T's Weapons stage reads the new profile: a `fire: false` state refuses fire and cancels the pawn's activations, host-local and remote alike. Movement and look follow from T+1 (O16).
- **Fire across a transition.** A remote shot refused by a life-state transition the client had not yet received uses the host's existing zero-HP refusal path unchanged (O11). The client weapon frame gates on its own replicated state (O12).
- Put the per-site branches in a new `sim/src/sim/player_life.rs` and edit only call sites in `sim/mod.rs`.
- Delivers AC2, AC4 (these keys), AC6, O11, O12 and O16.

### Task 4: Life-state reads and reload remap

- **`player.lifeStateMs`.** Add a `PlayerSlot` variant read from the component's elapsed ticks by AC7's conversion: catalog entry with `OwnerPrivatePlayer`, `player_slots.rs` arm, a host and single-player store publish beside `publish_swim_values`, `MIRRORED_GAME_STATE_LEAVES`, regenerated types and fixtures. This makes 15 player slots; the u16 bitset cap holds.
- **HUD value.** The UI reads a display value; the store keeps tick values.
  - **Site.** The display value is substituted into the UI's frozen value map at the store-to-UI boundary, `build_ui_display_slot_snapshot` (`main.rs`), beside `apply_timed_slot_targets`, on every role. It is never written to the store. That function gains the frame's interpolation alpha, the own pawn's elapsed ticks and its reset witness (pawn, `life_state`, teleport generation, authoritative elapsed) as inputs, plus the hold state (previous displayed value and previous witness) by mutable reference. The frame path in `main.rs` owns the hold state and passes all of them; `startup/loading_screen.rs` has no own pawn and passes none.
  - **Store.** On a connected client the store record holds the replicated owner-private samples: snapshot apply writes it, and the store and state crossings read it. Host and single player store AC7's millisecond value of the component's elapsed ticks at the publish above. The owner-private copy stays the host's per-player source for conditions.
  - **Value.** The own pawn's elapsed in milliseconds plus alpha × `TICK_DURATION`, so it rises on every rendered frame, zero-tick frames included, on every role (O25). On a connected client the source is the *predicted* elapsed, which advances every fixed tick and reconciles with movement.
  - **Monotonic hold.** The displayed value never falls below the previous frame's except at a real reset: a change of pawn, of `life_state` or of teleport generation, or the authoritative elapsed falling (host and single player: the component; client: consecutive replicated records). The host's command-queue catch-up trim drops commands the client already counted, so a replayed value can land lower with no reset (AC18, O24).
- **`player.lifeState`.** Add a local String catalog slot with `ReplicationScope::None` (precedent `player.weapon.current`). Every role publishes it for its own pawn from the replicated `life_state` and the name table (host and single player: `DataRegistry::life_state_names`; client: the `TuningPayload` table). A connected client returns from `tick_for_role_and_report_sampled_weapon` before `publish_swim_values`, so the write goes in its role-independent part, beside `publish_local_weapon_state`. An undeclared pawn reads `""`.
- **`inLifeState` in local crossings.** `StoreScope` resolves only slot-table names. Add a `StoreScope` input handle for `player.inLifeState:<name>` that holds while the local `player.lifeState` slot equals `<name>`, so local `onStateCrossing` predicates read the own pawn on every role. A name absent from `DataRegistry::life_state_names` fails the bind, as in `PlayerConditionScope`. Every role builds that table at manifest load, so a client binds before any `TuningPayload` arrives.
- **Hot reload remap.** The commit rebuilds `DataRegistry::life_state_names` and re-lowers each descriptor's ids. When the table changes, it bumps `DataRegistry::life_state_table_revision`, wrapping. In the descriptor refresh path (`plan_descriptor_refresh`), remap each live pawn by name.
  - The remap runs for every live pawn whose `life_state` is not 255, whatever the planner decides for its movement component.

    | Planner outcome for the movement component | Remap write |
    |---|---|
    | Replaced through `plan_movement_replace`, which resets any field it does not list | The replacement carries the remapped `life_state` id, elapsed ticks and the new revision explicitly |
    | Kept old (`keep_old`: a rejected refresh, such as a capsule that would embed the pawn) | The remap writes the same three fields onto the kept component |
    | Skipped (no previous descriptor snapshot) | Same as kept old |
    | New descriptor declares no states | `life_state` 255, revision updated, stamps cleared, one warning |
  - Kept name: elapsed continues, then `restamp_life_state` from the new profile. Stamps are never carried the way `plan_movement_replace` carries `vault_barred`.
  - Removed name: `enter_life_state` into the new `initial`, elapsed 0, with one warning.
  - `TuningPayload::same_pawn_tuning` compares the life-state fields too, so a profile-only reload re-sends tuning and the client rebuilds and restamps its pawn (O14).
  - The remap covers pawns that stay materialized (AC16). A remote pawn demoted on the mod digest is harvested and re-promoted through Task 5's rejoin path (O23).
- Delivers AC9 (crossing arm), AC10, AC16, AC18, O8, O14, O24, O25 and O27.

### Task 5: Respawn and rejoin

**SDK and binding**
- Add `respawn({ health?, lifeState? })` to `playerVerbs` and the type declarations.
- Extend the Task 1 name walk in `validate_reaction_bodies_pass_a` to `respawn`'s `state` argument, and warn-skip a runtime target per AC8 (its `respawn` arm).
- Add `BoundTriggerCommand::Respawn`. It queues a `RespawnRequest { pawn, health, state }`, where `state` names the target state (`None` = `initial`).
  - **Resolution.** The drain resolves the name through the pawn descriptor's lowered block, as rejoin restore does, never at the verb: a hot reload commits at frame tail, between the verb and the drain, and re-lowers ids. AC8's runtime arm runs at the verb.

    | Case | Where | Outcome |
    |---|---|---|
    | `state` named; target declares no states | Verb | Warn-skip; nothing queued (AC8) |
    | `state` named; target's descriptor does not declare it | Verb | Warn-skip; nothing queued (AC8) |
    | `state` is `None` | Drain | Relocates; a declaring pawn enters `initial` |
    | Descriptor still declares `state` | Drain | Enters it |
    | A reload removed `state` from the descriptor between verb and drain | Drain | Relocates and enters `initial`, one warning (O19) |
  - **Replacement.** A second request for the same pawn replaces the first (O3).
  - **Queue lifetime.** The drain skips a request whose pawn no longer exists, with one warning (O21). Level unload clears the queue beside `pending_teleporter_mutations` (`startup/lifecycle_net.rs`; O20). A hot reload keeps it, because the bleed-out edge that queued it does not re-fire.

**Placements**
- Netcode depends on sim, so the app publishes resolved placements into a sim-readable `RespawnPlacements` table on `ScriptCtx`: seat → placement transform, plus the map-order transforms of `player_spawn::joinable_spawns` (the join path's eligibility, so respawn and join agree on which spawns exist).
- Single player and the host publish at install, after `bind_installed_first_player` binds seat 0's placement through `bind_pawn`. The host publishes again after every `SeatTable::assign_joiner_placement`; the app's participation seam receives each index from it. No accessor exposes a seat's placement, so `SeatTable` gains `placement_for_seat(seat) -> Option<usize>`.

**Relocation, at the next fixed tick's start.** A new `ScriptCtx::pending_respawns` queue sits beside `pending_teleporter_mutations`, under the same `group_commands_apply_here` gate. It drains inside `simulate_tick_with_presentation_aim`, right after `apply_pending_teleporter_mutations` and before movers, the host-local fire-gate capture and movement, so the host camera hold window sees it (O17).
- One drain relocates its requests in seat order.
- Pick the placement per AC13. The overlap test is origin distance under the sum of capsule radii with overlapping vertical extents, against other live pawns. No physics query exists for pawn-vs-pawn. Each pick tests against positions after the earlier relocations in the same drain, so pawns respawning on one tick never share a placement while a free one exists (O18).
- Set position. Zero `velocity`, `knockback_velocity` and `push_velocity`, and leave `Dash` or `Sliding` for `Normal`, so no banked boost survives.
- Settle ground at the placement as a teleporter arrival does (`settle_arrival_ground`, exposed from `physics/src/movement/teleport.rs`), so mover carry from the death spot never follows the pawn.
- **Yaw.** The delta is the placement facing minus this tick's resolved facing. Remote commands (`host_resolve_remote_commands`) and the local command are resolved in the pre-respawn frame before the tick starts. The drain therefore turns that pawn's movement input facing, its remote aim and the host presentation aim by the delta before movement runs, as the movement stage does for a `TeleportEvent`'s `yaw_delta`. The respawn tick integrates from rest along the placement facing and ends with transform yaw at the placement facing (O17).
- Bump the teleport generation with `last_teleporter = NO_TELEPORTER`, and adjust `accumulated_yaw` by the delta. Rebuild `arrival_latches` at the destination after this tick's teleporter mutations, so a newly armed teleporter counts (O29).
- Set health through `set_health_absolute`.
- Enter the state through Task 1's `enter_life_state`, which zeroes elapsed and restamps.
- Cancel owned activations and charge, and drop any vault.

**Rejoin** (AC14)
- `CarriedState` (`combat-model/src/carried_loadout.rs`) gains `life_state: Option<String>`, the leaving state's *name*.
- `SeatTable::harvest_pawn` records it on every participation exit. `host_handle_lifecycle` routes `SlotEvent::Closed` and `SlotEvent::Demoted` alike through `host_handle_transport_disconnect`, which harvests before despawn. A demoted slot's re-promotion therefore re-enters through the `rejoin` map at its seat placement, like a reconnect (AC14, O23).
- Both `harvest_pawn` cfg variants gain a name-table parameter (`&[String]`, `DataRegistry::life_state_names`). The host builds `TuningPayload` per pawn from descriptors, so no disconnect-path function holds the table. `host_handle_lifecycle` and `host_handle_transport_disconnect` in `netcode/src/host.rs` gain the parameter, and each `main.rs` caller passes it from `DataRegistry`. An undeclared pawn, or an id the table does not name, records `None`. The id → name lookup lives in `player_life.rs`. The `seat.rs` edits are the new `CarriedField` arm, the `harvest_pawn` and `harvest_bound_pawns` parameters, `placement_for_seat`, and the level-unload clear below.
- `SeatTable::harvest_bound_pawns` runs on suspend (`main.rs`) as well as at level unload (`startup/lifecycle_net.rs`). It gains the same name-table parameter and passes it to `harvest_pawn` like any harvest; both callers pass `DataRegistry::life_state_names`. The level-unload caller clears `DataRegistry` before it harvests, so the table must survive that clear (Task 1). A held seat has no pawn binding, so its loop never reaches one.
- The level-change clear lives in `SeatTable::clear_pawn_bindings_for_level_unload`, which runs only at actual level unload, after `harvest_bound_pawns` and the registry clear. It clears `life_state` on every carried record, held seats included, so the next level starts in `initial` (O7). Suspend never calls it, so a suspend and resume leaves every carried name intact (O22).
- Either drain order of a level-change demotion holds. One drained before unload records a name this clear removes. One drained after finds its pawn despawned and records nothing.
- Hold expiry already releases the seat and mints a fresh one with no carried record, so a late reconnect starts in `initial` with no new code.
- Restore runs at the Task 1 spawn sites beside each `restore_carried_health` call (`data_archetype.rs` player-start path, `net_descriptor.rs` net-slot path). A new `restore_carried_life_state` in `player_life.rs` resolves a carried name through the pawn descriptor's lowered block, which lowering rebuilt from the current table, then maps it through the lowered `rejoin` map. A name the descriptor does not declare, or a missing `rejoin` entry, yields `initial`. It enters the result through `enter_life_state`, so elapsed starts at 0. Health keeps `restore_carried_health` unchanged.

**Tests**
- Every AC and row in the Delivers line below, including a rejoin across a hot reload that removed the carried name.

Delivers AC8 (`respawn` arm), AC11, AC12, AC13, AC14, AC16 (demotion arm), O3, O4, O6, O7, O10, O15 (respawn and rejoin arms), O17, O18, O19, O20, O21, O22, O23, O28 and O29.

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

Delivers AC15, AC19 (residual rows O1, O2 and O5), AC20, AC21, and the manual proof for AC17 and AC18.

## Sequencing

**Phase 1 (sequential):** Task 1. The thin slice falsifies the descriptor, wire, verb and condition boundary assumptions, and every later task consumes its component fields, write pair, name table and helper.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4. Each extends a different seam off Task 1's helper, fields and write pair. Task 2 owns movement files, the client eye-height ease and the body of `restamp_life_state`; Task 3 owns sim gate sites; Task 4 owns the slot catalog, the UI display seam and refresh.
**Phase 3 (sequential):** Task 5. It consumes Task 1's elapsed reset and write pair and Task 4's name-table rebuild (its rejoin-across-reload test), and adds to Task 3's `player_life.rs`.
**Phase 4 (sequential):** Task 6. It consumes every surface.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| Life-state block | `EntityTypeDescriptor::life_states` | `"lifeStates"` (`initial`, `states`, `rejoin`) | `components.lifeStates` | `components.lifeStates` |
| Profile keys | `LifeStateProfile` fields | `movement.speedScale`, `movement.abilities`, `movement.posture` (`"stand"`/`"crouch"`), `look`, `fire`, `switchWeapon`, `reload`, `use`, `touch`, `occupancy`, `damageable` | same | same |
| Rejoin map | lowered map of state ids, host-only | `rejoin: { <fromState>: "<toState>" }` | same | same |
| Set verb | `BoundTriggerCommand::SetLifeState` | primitive `"setLifeState"`, args `{ state }` | `setLifeState(name)` | `setLifeState(name)` |
| Respawn verb | `BoundTriggerCommand::Respawn` | primitive `"respawn"`, args `{ health?, state? }` | `respawn({ health?, lifeState? })` | `respawn({ health = , lifeState = })` |
| Condition input | parsed in each scope | `{op:"input", name:"player.inLifeState:<name>"}` | `inLifeState(name)` | `inLifeState(name)` |
| Slots | `PlayerSlot::LifeStateMs` | `"player.lifeStateMs"`, `"player.lifeState"` | `player.lifeStateMs`, `player.lifeState` | same |

## Wire format

Both surfaces, `WirePlayerMovementState` and `TuningPayload`, are pinned in Task 1's **Wire** block; no other task changes them.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1. One capability source: every gate on a declared pawn reads its current profile; undeclared pawns keep today's HP-derived gates | Task 1 (helper, fire sites) | Task 2, 3 gate sites; the death sweep's activation cancel; any new gate added later | AC2, AC4, AC6, O16 |
| I2. Host, single player and client prediction derive capabilities from the same descriptor data: host and single player read the pawn descriptor's lowered block; the client reads the replicated `life_state` + `TuningPayload` profiles built from that block. Stamps are recomputed from them at every `life_state` write | Task 1 (write pair, revision gate), Task 2 (stamps) | Client merge before replay; Task 4 reload remap; Task 5 respawn drain and restore; a snapshot whose table revision differs from the installed payload's; client weapon frame | AC4, O9, O13, O14, O26 |
| I3. The engine never changes life state on HP. Only `initial` at spawn/join/restart, the `rejoin` map at restore, `setLifeState`, `respawn` and hot-reload remap move it | Task 1 | Task 4 (remap), Task 5 (respawn, rejoin, demotion re-promotion) | AC3, AC7, AC14, AC16 |
| I4. Elapsed time resets on every actual transition, on respawn and on rejoin; a same-state set and a kept-name reload preserve it | Task 1 | Task 4 (remap carries elapsed through the movement rebuild), Task 5 | AC7, AC14, AC16, O5, O8, O15 |
| I5. A respawn relocates exactly once per request batch through the teleport generation, with no teleporter cue | Task 5 | Same-tick teleporter use; hot reload, level unload or harvest between verb and drain | AC12, O3, O10, O19, O20, O21, O28 |
| I6. The death latch contract is unchanged: `playerDied` fires once per latch, re-armed by positive health | Existing | Task 5 health write | AC11, O1, O2 |
| I7. Client smoothness: no life-state transition reaches the owning client as a teleport-class snap except respawn; the bleed-out time the client watches is predicted locally and never falls between real resets | Task 2, 4 | Task 5 must keep the teleport class to respawn only; any later state-driven relocation; the host's command-queue trim | AC17, AC18, O9, O24, O25 |

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

- **Decided by default, owner to confirm: a hot-reload demotion is a rejoin.** A host hot reload that changes the mod digest demotes clients, so their pawns re-enter through the `rejoin` map at the seat placement (AC14, AC16, O23). Keeping live state across a demotion instead needs a carry path beside the harvest.
- **Player-descriptor-composition draft** is live but needs a rebase: only its file split shipped; per-state movement tuning, modifiers under states and layered viewFeel are unbuilt. When it lands, `movement.speedScale` may become a reference to a movement modifier instead of a bare scale. Neither spec blocks the other.
- **`coop--spawn-placement-occupancy` draft** is mostly shipped (`0d0aa24`, `40d39a5`); it does not block respawn placement, which uses the same `joinable_spawns` eligibility the join path uses.
