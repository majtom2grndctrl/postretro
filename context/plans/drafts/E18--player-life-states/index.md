# Player Life States

## Goal

Give mod authors an engine mechanism for player death and revival. They express their own death mechanics as policy over it: Borderlands down/revive, Quake co-op respawn, Doom restart. The engine adds author-named **life states** that gate an engine-closed capability set, reads for conditions and the HUD, verbs to change state and respawn, and a teammate **assist** interaction. It does not add a death policy. The dev mod ships Borderlands-style co-op death as the first consumer. Connected clients must feel it as smoothly as the host: no transition snaps the view except respawn, and the bars a player watches advance every frame. It is the first of the phases listed under Out of scope, and the co-op floor that E18-R (respawn and player-leave) names. E18-R's player-leave half keeps the existing harvest-and-hold contract; this spec adds only the life state a rejoining player gets (O11).

## Scope

### In scope

- `components.lifeStates` on an entity type descriptor: author-named states, an `initial` state, and per-state capability profiles. One validator serves both runtimes.
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
  - `player.assistProgress` (per-player Number slot)
- Impact-policy source reads and effects: `source.inLifeState(name)`, `source.setLifeState(name)`.
- Assist:
  - A held-Use input on the wire.
  - An `assistable` profile key.
  - A host accumulator.
  - Discrete player-event sources `assistReceived()` and `assistGiven()`.
- Hot-reload remap of live pawns by state name.
- Dev-mod Borderlands policy (down, bleed-out respawn, teammate revive, self-revive on kill, author-chosen bonuses) and its HUD.
- Updates to `docs/scripting-reference.md` and `docs/teleporters.md`.

### Out of scope

Later phases build on this shape. Each item below is additive to the Phase 1 surface.

- **Phase 2: respawn variety.**
  - `respawn({ loadout })` reset to the descriptor loadout.
  - Drop-on-death backpacks.
  - Checkpoints: `respawn({ at: { tag } })`, trigger-set per-player anchors, and `respawnAtCheckpoint()`.
  - Press-to-respawn: an input-edge player-event source.
  - Post-respawn invulnerability.
- **Phase 3: presentation.**
  - Spectate/follow camera and a death cam (view roll and drop).
  - A per-state avatar `animation` override. Host `update_player_animation_locomotion` and the client remote-avatar classifier reselect every tick, so both need a hold path.
  - A revive prompt showing the nearest assistable teammate.
  - A target-side "being revived" slot separate from `player.assistProgress`.
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
- **Engine mechanism vs mod policy.** The engine owns what a life state *means* to engine systems: the capability gates, the assist accumulator, and respawn relocation. The mod owns which states exist, when transitions happen, and every consequence (bonuses, timers, screens). The engine never transitions a pawn on HP.
- **Host vs client.** Transitions, assist completion and their consequences are host-authoritative. Gates run on host, single player, and client prediction from the same profile data. What the owning client *watches* (its movement through a transition, its own assist progress, its bleed-out time) is predicted locally from data it already holds, then corrected smoothly, so host authority never shows up as latency or stair-stepping.
- **Descriptor vs code.** Profiles are descriptor data because gates read them every tick, inside movement replay, where the IR cannot run.

**Prior commitments.**

- **"The engine has no built-in death policy"** (`scripting.md` §10.4) and **"death is modder policy derived from impact"** (Q3 archive, E16). Kept. Zero HP fires only what it fires today, and every transition is an authored verb.
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
- **Co-op research: E18-R needs respawn after a delay, a latch that re-arms, and dead players gating occupancy** (`context/research/co-op-triggers-trap-pools.md`).
  - Delivered by `respawn`, `player.lifeStateMs`, and the `occupancy` capability.
  - Re-arming already holds through `set_health_absolute`, which respawn and `grantHealth` both reach.

**Alternatives rejected.**

- **An engine-owned closed `alive`/`down`/`dead` model.** The owner chose author-named states. A closed set fixes three mechanics in engine vocabulary. Named states with closed capabilities cover the same ground and still let authors add "spectating" or "bleeding".
- **Engine features per mechanic** (a revive system, a lives system). These put policy in the engine. Revive here is `setLifeState` plus `grantHealth` in an authored fire list, and bonuses are ordinary fire-list entries.
- **A new `on.assister` token** for one `assisted()` event. A second dispatch token touches about a dozen hand-kept sites: SubjectToken, SequenceTarget, BoundTarget, DispatchScope's single event-player snapshot, `byPlayer` recognition in three installers, and five per-source rejection guards. Two discrete sources, each binding the existing `on.player` to a different pawn, deliver both bonuses with none of that.
- **Assist as a trigger volume attached to the pawn.** Trigger volumes are level-authored AABBs keyed by map identity. A pawn-carried one has no placement, tag, or install story.
- **A generic hold-to-interact mechanism with assist as its first target kind.** Pawn targets need the same accumulator either way. Assist stays player-on-player in Phase 1 because no other hold-to-interact consumer exists; generalising later adds a target kind beside pawns without changing the profile key or the sources.
- **A raw `setCapabilities({...})` verb with no named states.** Named profiles validate at load, cost one byte on the wire, replay deterministically from that byte, and give `player.lifeStateMs` and `inLifeState` something to read. A raw verb would ship the whole capability set per change and leave conditions nothing to name.
- **Bleed-out as a per-player `wait`.** This needs `on.player` after `wait` and cancellation on revive. Player events reject interruptible waits. A readable elapsed-time fact is simpler, shows on the HUD, and resets on every transition by construction.

**Foreclosures and one-way doors.**

- New wire fields (movement state, input) and a `TuningPayload` epoch bump. Pre-stable, undone with another bump.
- SDK names (`lifeStates`, `setLifeState`, `respawn`, `inLifeState`, `assistReceived`, `assistGiven`) become the modder contract. Renaming them costs a pre-stable SDK break across both runtimes.
- Task 7 widens the per-player slot bitset (`EntityScope.warned_owned_engine`, capped at 16 by a compile-time assert) to u32. This spec takes `PlayerSlot` from 14 to 16, and without widening the next per-player slot would fail to compile.
- The u8 state id caps the global name table at 255 names. That is material only for a mod with more than 255 distinct life-state names.
- An assist with no line-of-sight or facing requirement can be tightened later by adding optional profile keys. Nothing material is foreclosed.

## Acceptance criteria

### Descriptor and validation
- [ ] **AC1.** Both runtimes load a `components.lifeStates` block with `initial` and `states` into the same lowered shape. Loading is rejected, with the full key path, for each of these:
  - an `initial` that names no declared state
  - a non-identifier state name
  - an unknown profile key
  - a `speedScale` outside `[0, 1]` or non-finite
  - an `assistable.radius` ≤ 0 or non-finite
  - an `assistable.holdMs` < 0 or non-finite
  - an unknown `posture`
  - the block on a descriptor without player movement
  - a mod whose distinct names exceed 255

  A minimal valid block loads.
- [ ] **AC2.** A pawn whose descriptor declares no life states behaves exactly as today at zero HP. Fire is blocked and cancelled, vault is barred (so teleporters are skipped), the pawn is excluded from trigger occupancy and is not a spatial damage target. Movement, look, switch, reload, Use, Touch and pickups stay ungated.

### Life state and gates
- [ ] **AC3.** A pawn spawns, joins, rejoins and restarts the level in its descriptor's `initial` state with `player.lifeStateMs` at 0. Rejoin applies even after a disconnect while in another state.
- [ ] **AC4.** For each capability key there are paired tests, one per direction. A state with the key `false` (or `speedScale: 0`) refuses the gated action. A state with the key `true` permits it at zero HP. This covers firing while downed, and covers it on the host, in single player, and in connected-client prediction. Movement and look keys hold across reconcile replay with no correction when host and client share the state.
- [ ] **AC5.** `movement.posture: "crouch"` holds the pawn crouched while it is in that state, even with crouch released. Leaving the state returns control to the crouch input under the normal stand-up clearance rule.
- [ ] **AC6.** `touch: false` blocks new trigger Touch entries and pickups. It does not emit Exit for a volume the pawn already occupies. `occupancy: false` removes the pawn from occupant counts.

### Verbs and reads
- [ ] **AC7.** `setLifeState(name)` from a player event applies in-tick. The next tick's gates read the new profile, and `player.lifeStateMs` restarts from 0. Setting the current state again is a no-op that keeps elapsed time.
- [ ] **AC8.** Install rejects any reaction whose `setLifeState`, `respawn({ lifeState })`, `inLifeState`, or `source.inLifeState` / `source.setLifeState` names a state absent from the mod's name table. The error names the reaction and the state. A runtime target whose descriptor lacks that state, or declares no states, warn-skips.
- [ ] **AC9.** `inLifeState(name)` holds exactly while the evaluated pawn is in `name`. This applies in player-event conditions, in local `onStateCrossing` predicates (own pawn) and in impact-policy `source.inLifeState`. It is false for a non-player source, which reads no warning.
- [ ] **AC10.** `player.lifeState` (local String) and `player.lifeStateMs` reach the owning client's HUD. A HUD `visibleWhen` on `player.lifeState` equal to `"down"` shows only while downed. An undeclared pawn reads `player.lifeState` as `""`.

### Respawn
- [ ] **AC11.** `respawn()` relocates the pawn to its seat's placement on the next fixed tick and applies all of these:
  - zeroes velocity and sets yaw to the placement facing
  - sets health to max and re-arms the death latch, so a later zero fires `playerDied` again
  - enters `initial` (or the given `lifeState`) with elapsed time 0
  - cancels weapon activations and charge, and drops any vault
  - keeps inventory
- [ ] **AC12.** The owning client snaps through the teleport reconcile class and observers cut instead of interpolating. No teleporter arrival sound plays, even when the pawn last used a teleporter.
- [ ] **AC13.** Respawn picks the seat placement unless another live pawn overlaps it. It then picks the lowest map-order placement that no pawn overlaps. With none free it uses the seat placement and logs one warning. `health` outside `(0, max]` or non-finite warn-clamps to max. A non-player target warn-skips.

### Assist
- [ ] **AC14.** Holding Use within `radius` of a pawn whose current state declares `assistable` completes after `holdMs`. The completion fires every `assistReceived()` subscription with `on.player` = target, then every `assistGiven()` subscription with `on.player` = helper. Both fire in the tick the hold completes, before that tick's condition snapshot.
- [ ] **AC15.** Paired refuse/permit edges for assist:
  - **Refused:** out of range; target in a non-assistable state; helper's state has `use: false`; release one tick before `holdMs`.
  - **Permitted:** in range on an assistable state; `holdMs: 0` completes on the first held tick.

  Progress resets when the helper releases or switches targets, when either pawn changes state, and when the target respawns. `player.assistProgress` reports the helper's progress 0..1, and the target's when it is not itself helping.
- [ ] **AC16.** Two helpers completing on one target in one tick produce one completion, for the earlier helper in `players()` order. The other helper's progress resets. Held Use survives a short input gap under the host's hold-then-neutral gap policy.

### Co-op and lifecycle
- [ ] **AC17.** A late joiner sees an existing downed teammate's life state (`inLifeState` on the host, posture on the avatar) with no extra baseline work.
- [ ] **AC18.** After hot reload, a live pawn keeps its state by name. If the new descriptor removed that name, the pawn enters the new `initial` state with one warning.

### Client experience
- [ ] **AC22.** A connected client running into a transition (down with `speedScale: 0.3`, or revive back to full speed) at 150 ms simulated round-trip time gets no teleport-class snap. The correction rides the ordinary reconcile smoothing class, and the camera eases into and out of forced crouch exactly as a crouch-key press does. Respawn is the only life-state path that snaps.
- [ ] **AC23.** The helper client's assist bar shows its own progress on the first held tick, with no round-trip wait, and advances every rendered frame. If the host resets or never confirms (target moved, revived elsewhere, out of range on the host), the bar returns to 0 with no success cue. Success consequences (state, health, bonuses, their presentation) appear only after host confirmation. The target client's received-progress bar has no stair-steps at snapshot arrival.
- [ ] **AC24.** On the owning client, the `player.lifeStateMs` HUD value advances every rendered frame and never moves backwards when a snapshot lands, except at a real transition or respawn.

### Orderings and consumer
- [ ] **AC19.** Every row of the Orderings table has a test asserting its expected outcome.
- [ ] **AC20.** The dev mod ships the Borderlands policy in TS and Luau twins, with a parity surface test:
  - zero HP → down: crawl, can fire, can't switch or use
  - bleed-out after a fixed delay → respawn
  - an enemy kill while down → self-revive, with a health and XP bonus
  - teammate hold-Use → revive, with an XP bonus to the helper
  - a bleed-out bar, a revive-progress bar, and a downed vignette on the HUD

  A two-player headless test drives down → assist → alive, down → self-revive, and down → bleed-out → respawn.
- [ ] **AC21.** The docs need these changes:
  - `docs/scripting-reference.md` documents life states, the verbs, the reads, assist, and the Doom / Quake / Borderlands recipes.
  - It drops its stale claims that per-owner + persist is rejected and that there is "no source-scoped fact vocabulary".
  - The verb tables in it and in `docs/teleporters.md` list the new verbs.
  - Every command shown runs from an SDK bundle.

## Orderings

Task agents cite rows by number. Expected outcomes are binding.

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Two pawns zeroed in one tick | Same death sweep | `becomes(health ≤ 0)` fires once per pawn, each with its own `on.player`. `playerDied` fires once, unchanged |
| O2 | Down and revived in one tick (impact self-revive applies before the sweep sees zero) | In-tick | No death edge. `playerDied` fires if the sweep latched first; health positive re-arms it |
| O3 | Assist completes the same tick bleed-out would expire | Assist in tick, condition snapshot after | Revive applies first; the bleed-out condition reads `inLifeState("down")` false and does not fire |
| O4 | Self-revive kill from a remote hit still waiting on fire authorization, landing after bleed-out expired | Bleed-out tick N, hit tick N+k | Respawn wins. The later `source.inLifeState("down")` is false and the self-revive row does not fire |
| O5 | Assist completion recorded in tick, then a post-tick remote-hit flush self-revives the target | Completion, flush, player-event pass | The completion is dropped at application because the target no longer declares `assistable`. No assist sources fire |
| O6 | Two `respawn` verbs on one pawn before the next tick | Queue | One relocation; the last request's arguments win |
| O7 | `respawn` and `setLifeState` on one pawn in one fire list | Listed order | `setLifeState` applies in-tick. Respawn applies next tick and sets its own state, which wins |
| O8 | `setLifeState` to the current state | — | No-op; elapsed time continues |
| O9 | `holdMs: 0` | First held tick | Completes that tick |
| O10 | Helper disconnects mid-hold | Seat hold | Progress dropped. The target is unchanged |
| O11 | Target disconnects while down, rejoins inside the hold window | Harvest, restore | Rejoins in `initial` at the seat placement; carried health ≤ 0 restores max |
| O12 | Level change while downed | Harvest at unload | Next level starts in `initial` with carried health (max if ≤ 0) |
| O13 | Hot reload mid-down, state kept / removed | Recompose | Kept: same state, elapsed continues. Removed: `initial`, elapsed 0, one warning |
| O14 | Client predicts through a transition | Host transition tick T | At most one reconcile correction at the state change. No repeated corrections while host and client share the state |
| O15 | Respawn requested on the tick the pawn uses a teleporter | Same tick | The teleporter resolves, then respawn relocates the next tick. The generation bumps twice and the client snaps to the respawn |
| O16 | Client fires in the window between the host's transition to `fire: false` and the client receiving it | Host tick T, client learns at T + RTT/2 | The host refuses those shots through the same path it uses for a zero-HP shooter today. The client shows no feedback class that path does not already show |
| O17 | Client fires in the window after the host revives it (`fire: true`) but before the client knows | Same window, reversed | The client's own gate still refuses locally. No shot is sent, so nothing is refused and nothing is shown. Firing resumes when the state arrives |
| O18 | Helper client predicts completion, but the host's accumulator reset one tick earlier (range edge under interpolation) | Client ahead of host | The host's reset wins. The client bar drains to 0 at the next snapshot and no success presentation plays |

## Tasks

New logic lands in new modules. Files already past ~800 lines (`sim/src/sim/mod.rs`, `physics/src/movement/mod.rs`, `netcode/src/lib.rs`, `netcode/src/client.rs`, `sim/src/impact_policy.rs`, `postretro/src/startup/lifecycle.rs`, `startup/reaction_validation.rs`, `netcode/src/state_slots.rs`, `scripting_systems/data_archetype.rs`) receive only call-site edits and match arms.

### Task 1: Thin slice: descriptor → state → verb → condition → wire → fire gate

This slice falsifies the boundary assumptions.

**Descriptor and validation**
- Add `lifeStates` as a descriptor component kind, `DescriptorComponentKind` plus `ComponentKind` / `ComponentValue`.
- Both VMs read it into a neutral raw form. One shared validator, modelled on `validate_charge_interrupts` (sorted keys, `DescriptorError::InvalidShape` with full path), applies AC1.

**Global name table and profiles**
- Build a sorted union of every declared name across `DataRegistry::entities` at manifest load. A state id is its index.
- The table and every archetype's profile, keyed by id, ride `TuningPayload`, with `TUNING_PAYLOAD_EPOCH` bumped to 16 and its fixture updated.

**Per-pawn state**
- Add `life_state: u8` (255 = undeclared) and `life_state_elapsed_ms: u32` to `PlayerMovementComponent`, beside `teleport`.
- Spawn paths set `initial`: `spawn_from_player_starts*`, and the netcode joiner spawn `host_handle_accept_descriptor_at_placement`.
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

Delivers AC1, AC3, AC7, AC8 (verb and condition arms), AC9 (condition arm).

### Task 2: Movement capabilities

- **Stamping.** The movement tick reads profile-derived capability fields stamped onto `PlayerMovementComponent`. Replay cannot read the registry; `vault_barred` is the precedent. Host, single player and the client stamp from the profile table at the tick the state applies, so replay reproduces them from the replicated `life_state`.
- **`movement.speedScale`** multiplies the per-state wish speed.
- **`movement.abilities: false`** blocks entry to jump, dash, slide, vault and crouch toggling. Vault keeps its existing barring path, `drop_vault_if_dead`, which becomes a capability read for declared pawns.
- **`movement.posture: "crouch"`** forces crouch intent. Crouch already rewrites `capsule.eye_height`, so fire origin, hit validation and AI line of sight follow without new code.
- **`look: false`** freezes view yaw/pitch input for the pawn on host and client. Existing camera feel layers still run.
- **Client smoothness.** A life-state change replicated to the owning client takes the ordinary reconcile correction class, never the teleport class. The teleport class stays reserved for respawn (Task 5) and teleporters. The posture change reuses the crouch view-height easing.
- Delivers AC4 (movement and look keys), AC5, AC22 and O14.

### Task 3: Interaction and combat capabilities

Route each key through the Task 1 helper at its existing site.
- **`switchWeapon` and `reload`:** the switch and reload request gates on host, single player and the client weapon frame. These are ungated at zero HP today.
- **`use`:** trigger Use edges in `TriggerSystem::run_authoritative_tick_with_dispatch`.
- **`touch`:** new Touch entries there and pickups in `touch.rs`. Gate the entry, never membership, so no false Exit fires.
- **`occupancy`:** `player_is_alive_for_trigger_occupancy` / `effective_occupants`.
- **`damageable`:** `is_damage_target_eligible` (`sim/src/scripting_systems/health.rs`), branching only for pawns carrying player movement so enemies are untouched. It gates spatial eligibility and AI attack exactly where zero HP does today. Hitscan apply, mover crush, impact effects and script `damage` stay ungated, as they are at zero HP today.
- **Fire across a transition.** A remote shot refused by a life-state transition the client had not yet received uses the host's existing zero-HP refusal path unchanged (O16). The client weapon frame gates on its own replicated state (O17).
- Put the per-site branches in a new `sim/src/sim/player_life.rs` and edit only call sites in `sim/mod.rs`.
- Delivers AC2, AC4 (these keys), AC6, O16 and O17.

### Task 4: Life-state reads and reload remap

- **`player.lifeStateMs`.** Add a `PlayerSlot` variant read from the component's elapsed field: catalog entry with `OwnerPrivatePlayer`, `player_slots.rs` arm, HUD publisher beside `publish_swim_values`, `MIRRORED_GAME_STATE_LEAVES`, regenerated types and fixtures. This makes 15 player slots; the u16 bitset cap holds.
- **Client HUD value.** On a connected client, the HUD publisher writes `player.lifeStateMs` from the owning pawn's *predicted* `life_state_elapsed_ms`, which advances every fixed tick and reconciles with movement. It never writes from the stepped owner-private sample. The owner-private copy stays the host's per-player source for conditions (AC24).
- **`player.lifeState`.** Add a local String catalog slot with `ReplicationScope::None` (precedent `player.weapon.current`). Each machine publishes it for its own pawn from the replicated `life_state` and the `TuningPayload` name table; an undeclared pawn reads `""`.
- **`inLifeState` in local crossings.** Teach `StoreScope` the `player.inLifeState:` input so local `onStateCrossing` predicates read the own pawn (precedent `read(player.swimming)`).
- **Hot reload remap.** Rebuild the name table and remap each live pawn by name in the descriptor refresh path (`plan_descriptor_refresh`). A removed name enters `initial` with one warning.
- Delivers AC9 (crossing arm), AC10, AC18, AC24 and O13.

### Task 5: Respawn

**SDK and binding**
- Add `respawn({ health?, lifeState? })` to `playerVerbs` and the type declarations.
- Add `BoundTriggerCommand::Respawn`. It queues a `RespawnRequest { pawn, health, state_id }`; a second request for the same pawn replaces the first (O6).

**Placements**
- Netcode depends on sim, so the app publishes resolved placements into a sim-readable `RespawnPlacements` table: seat → placement transform, plus map-order spawn transforms.
- Single player publishes at install. The host publishes at install and on every `SeatTable::assign_placement`.

**Relocation, in the next fixed tick's movement stage** (through the `pending_teleporter_mutations` seam, so the host camera hold window sees it)
- Pick the placement per AC13. The overlap test is origin distance under the sum of capsule radii with overlapping vertical extents, against other live pawns. No physics query exists for pawn-vs-pawn.
- Set position, yaw and zero velocity.
- Bump the teleport generation with `last_teleporter = NO_TELEPORTER`, rebuild `arrival_latches` at the destination, and adjust `accumulated_yaw`.
- Set health through `set_health_absolute`.
- Set the state and reset elapsed time.
- Cancel owned activations and charge, drop any vault, and reset any assist progress that names the pawn.

Delivers AC11, AC12, AC13, O6, O7 and O15.

### Task 6: Impact-policy source reads and effects

- **`source.inLifeState(name)`.** Add `EntityInputHandle::SourceLifeState`, resolved by `EntityScope::resolve_owned_input` for `owner:"@impact.source"`. It returns false, without a warning, for a non-player source.
- **`source.setLifeState(name)`.** Intercept it in `apply_planned` the way `SetOwnerSlot` is, not in `apply_effect`, which lacks the tick and pawn checks.
- **SDK.** Add `source` handle methods to `sdk/lib/data_script.ts` / `.luau` and the sdk_lib templates.
- **Name validation.** Unknown names fail at load through the AC8 walk. Without it, `bind_policy` would silently drop the whole policy.
- Delivers AC8 (impact arm), AC9 (impact arm), O2 and O4.

### Task 7: Assist

**Input**
- Add a held-Use bit `use_held` to `WireMovementInput`, inside the same `WIRE_VERSION` 36 as Task 1. It follows `command_queue.rs`'s hold-then-neutral gap policy like other held inputs.
- Thread it to the host and the single-player input.

**Accumulator**
- A host and single-player accumulator in new module `sim/src/sim/assist.rs`. It runs in the fixed tick after the weapon stage and death sweep.
- Each helper whose state allows `use` and who holds Use accumulates on the nearest pawn within its target's `assistable.radius`. Distance is between pawn origins; ties go to the earlier pawn in `players()` order.
- Progress resets per AC15. One completion per target per tick (AC16).
- Completions go into a tick-events `assist_completions` list (precedent `tick_events.trigger_exit_fires`).
- **One rule, two runners.** Write the accumulator as a pure step over (helper input, helper state, candidate pawn positions and states, profiles, previous progress). The host runs it authoritatively. The owning client runs the same step each predicted tick for its own pawn as helper only, using interpolated remote pawns, replicated life states and `TuningPayload` profiles. It writes only its local `player.assistProgress` for presentation and never emits completions.

**Client presentation**
- When host samples of the helper's own progress arrive, the client keeps its predicted value unless the host reports a reset or a different target. In that case the predicted progress drops to the host's value (O18).
- The target client's received progress ramps instead of stepping. Add `player.assistProgress` to the timed-slot display projection's enumerated slots (`done/coop--timed-slot-projection`; it projects only listed slots) as an upward ramp bounded at 1. The helper's own predicted value overrides the projection on the helper's machine.
- Success presentation (sounds, flashes) belongs to the authored `assistReceived` / `assistGiven` fire lists. Those run on host confirmation and route to each player's machine, so a prediction never plays one (AC23).

**Sources**
- `PlayerEventTable::run_tick` applies completions before `evaluate()`. Each completion is re-checked first and dropped if the target no longer declares `assistable` (O5).
- `BoundPlayerEvent` gains a source kind: `Edge { condition, edge, … }` or `AssistReceived` or `AssistGiven`.
- `player_event_from_json` accepts `edge: "assistReceived" | "assistGiven"` with no condition.
- Fire-list validation is the player-event rule set unchanged. Each fire binds `on.player` to the target or the helper respectively.

**Progress slot**
- Add `player.assistProgress` as a `PlayerSlot` (16 slots). Widen the `warned_owned_engine` bitset in `ir/scopes.rs` from u16 to u32 and update its compile-time assert.

Delivers AC14, AC15, AC16, AC23, O3, O5, O9, O10 and O18.

### Task 8: Dev-mod Borderlands consumer, co-op lifecycle tests and docs

**Content** (new `content/dev/scripts/player-death.ts` + `.luau` twin, registered in `start-script.ts`)
- **Player descriptor** (`content/dev/scripts/player.ts`):
  - `alive {}`
  - `down { movement: { speedScale: 0.3, abilities: false, posture: "crouch" }, switchWeapon: false, use: false, touch: false, occupancy: false, assistable: { radius: 1.5, holdMs: 3000 } }`
- **Player events:**
  - zero HP → `down`
  - `inLifeState("down")` and `lifeStateMs ≥ 15000` → `respawn()`
  - `assistReceived` → `setLifeState("alive")` + `grantHealth(30)`
  - `assistGiven` → XP via `addSlot` on `progression.xp`
- **Impact event:** a separate `defineImpactEvent("second-wind", { tag: "enemy" })`, not a row inside `enemyDeath`, which the zombie override would evict. On a kill with `source.inLifeState("down")`: `source.setLifeState("alive")`, `source.grantHealth(40)`, and an XP bump via `update(progression.xp.byPlayer(impact.source), …)`.
- **HUD:** a bleed-out bar on `player.lifeStateMs` visible while `player.lifeState` equals `"down"`, an assist bar on `player.assistProgress`, and a downed vignette from a local crossing on `inLifeState("down")`.

**Tests**
- A parity surface test (`*_surface_tests.rs` pattern).
- The two-player test on `player_events::tests::World` with the `impact_policy` test helpers, driving the AC20 paths.
- Lifecycle rows: AC3, AC17, O1, O8, O11, O12.
- **Manual proof**, recorded in the PR: a two-client playtest of the dev mod at 150 ms RTT (`POSTRETRO_NET_LAG`) on `coop-presentation-test`. Check AC22 to AC24 from the client's seat, covering the cases below. Record each run's lag settings and what was seen.
  - going down mid-sprint
  - crawling and shooting
  - reviving and being revived
  - a self-revive kill
  - bleed-out → respawn

**Docs**
- Update the docs per AC21.

Delivers AC17, AC19 (residual rows), AC20, AC21, and the manual proof for AC22 to AC24.

## Sequencing

**Phase 1 (sequential):** Task 1. The thin slice falsifies the descriptor, wire, verb and condition boundary assumptions, and every later task consumes its component fields, name table and helper.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4, Task 6. Each extends a different seam off Task 1's helper and fields. Task 2 owns movement files, Task 3 owns sim gate sites, Task 4 owns the slot catalog and refresh, Task 6 owns impact policy.
**Phase 3 (concurrent):** Task 5, Task 7.
- Task 5 consumes Task 4's elapsed reset semantics.
- Task 7 consumes Task 3's `use` gate and Task 4's slot catalog.
- Both edit `playerVerbs`/types, Task 7 for its sources. Merge-coordinate.
**Phase 4 (sequential):** Task 8. It consumes every surface.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| Life-state component | `DescriptorComponentKind::LifeStates` | `"lifeStates"` (`initial`, `states`) | `components.lifeStates` | `components.lifeStates` |
| Profile keys | `LifeStateProfile` fields | `movement.speedScale`, `movement.abilities`, `movement.posture` (`"stand"`/`"crouch"`), `look`, `fire`, `switchWeapon`, `reload`, `use`, `touch`, `occupancy`, `damageable`, `assistable.radius`, `assistable.holdMs` | same | same |
| Set verb | `BoundTriggerCommand::SetLifeState` | primitive `"setLifeState"`, args `{ state }` | `setLifeState(name)` | `setLifeState(name)` |
| Respawn verb | `BoundTriggerCommand::Respawn` | primitive `"respawn"`, args `{ health?, state? }` | `respawn({ health?, lifeState? })` | `respawn({ health = , lifeState = })` |
| Condition input | parsed in each scope | `{op:"input", name:"player.inLifeState:<name>"}` | `inLifeState(name)` | `inLifeState(name)` |
| Impact source read | `EntityInputHandle::SourceLifeState` | `{op:"input", owner:"@impact.source", name:"player.inLifeState:<name>"}` | `source.inLifeState(name)` | `source:inLifeState(name)` |
| Impact source effect | planned effect intercepted in `apply_planned` | `{ effect:"setLifeState", owner:"@impact.source", state }` | `source.setLifeState(name)` | `source:setLifeState(name)` |
| Slots | `PlayerSlot::LifeStateMs`, `PlayerSlot::AssistProgress` | `"player.lifeStateMs"`, `"player.assistProgress"`, `"player.lifeState"` | `player.lifeStateMs` etc. | same |
| Assist sources | `PlayerEventSourceKind::{AssistReceived, AssistGiven}` | `edge: "assistReceived"` / `"assistGiven"`, no `condition` | `assistReceived()` / `assistGiven()` | same |

## Wire format

- **`WirePlayerMovementState`** appends `life_state: u8` (255 = undeclared) then `life_state_elapsed_ms: u32`, little-endian. Field order follows the existing `teleport` record. `SNAPSHOT_VERSION` goes 22→23.
- **`WireMovementInput`** appends `use_held: bool` after `use_pressed`. `WIRE_VERSION` goes 35→36 for both changes.
- **`TuningPayload`** (epoch 16) adds `life_state_names: Vec<String>` (sorted, ≤ 255) and per-archetype `life_state_profiles: Vec<(u8, LifeStateProfile)>`, serialized like existing tuning entries.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1. One capability source: every gate on a declared pawn reads its current profile; undeclared pawns keep today's HP-derived gates | Task 1 (helper) | Task 2, 3 gate sites; any new gate added later | AC2, AC4, AC6 |
| I2. Host, single player and client prediction derive capabilities from the same data: replicated `life_state` + `TuningPayload` profiles | Task 1 | Task 2 stamping in replay; client weapon frame | AC4, O14 |
| I3. The engine never changes life state on HP. Only `initial` at spawn/rejoin/restart, `setLifeState`, `respawn`, `source.setLifeState` and hot-reload remap move it | Task 1 | Task 5, 6, 4 (remap) | AC3, AC7, AC18 |
| I4. Elapsed time resets on every actual transition and on respawn; a same-state set preserves it | Task 1 | Task 5, 4 | AC7, O8, O13 |
| I5. At most one assist completion per target per tick, applied before that tick's condition snapshot, and only while the target still declares `assistable` | Task 7 | Remote-hit flush between tick and player-event pass | AC14, AC16, O3, O5 |
| I6. A respawn relocates exactly once per request batch through the teleport generation, with no teleporter cue | Task 5 | Same-tick teleporter use | AC12, O6, O15 |
| I8. Client smoothness: no life-state transition reaches the owning client as a teleport-class snap except respawn; presentation the client watches (assist progress, bleed-out time) is predicted locally and corrected without success cues the host did not confirm | Task 2, 4, 7 | Task 5 must keep the teleport class to respawn only; any later state-driven relocation | AC22, AC23, AC24, O18 |
| I7. The death latch contract is unchanged: `playerDied` fires once per latch, re-armed by positive health | Existing | Task 5 health write, Task 6 self-revive | AC11, O1, O2 |

## Script syntax examples

**Borderlands (dev mod, TS).**

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
        assistable: { radius: 1.5, holdMs: 3000 },
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
  players().on(assistReceived(), [revived]),  // on.player.setLifeState("alive"), on.player.grantHealth(30)
  players().on(assistGiven(), [assistBonus]), // on.player.addSlot(progression.xp, 25)
];

export const secondWind = defineImpactEvent("second-wind", { tag: "enemy" }, (impact) => {
  const { target, source } = impact;
  const killed = target.healthBefore.gt(0).and(target.healthAfter.le(0));
  return [{
    when: killed.and(source.inLifeState("down")),
    do: [source.setLifeState("alive"), source.grantHealth(40),
         update(progression.xp.byPlayer(source), (xp) => xp.add(50))],
  }];
});
```

**Quake co-op (TS).** Dead players respawn after 2 s.

```ts
lifeStates: { initial: "alive", states: {
  alive: {},
  dead: { movement: { speedScale: 0, abilities: false, posture: "crouch" },
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

- **Player-leave policy (E18-R).** Decided by recommendation: rejoin in `initial` at the seat placement (O11). The owner may prefer rejoining downed. That would need `life_state` in `CarriedState`, which is additive.
- **Player-descriptor-composition draft.** It is stale: its planned split already shipped. It does not conflict at the key level. If it later revives per-state movement tuning, `movement.speedScale` may want to become a modifier reference instead of a scale.
- **`coop--spawn-placement-occupancy` draft.** Source refutes its premise: since commit `0d0aa24`, install spawns one pawn and the first joiner takes the first free spawn. That draft should be re-evaluated or dropped. It no longer blocks respawn placement.
