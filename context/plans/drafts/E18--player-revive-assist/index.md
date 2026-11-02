# Player Revive and Assist

## Goal

Let mod authors build revive mechanics (Borderlands second wind, L4D teammate revive) with author-chosen bonuses. The engine adds a teammate **assist** interaction, driven by its own `assist` input command, and life-state reads and effects on an impact's source. The dev mod upgrades its down/bleed-out policy to full Borderlands. Builds on `E18--player-life-states` (Spec A, landed in `done/`): its life states, capability gates, verbs, name table and slots are the base this spec extends. `assist` has no engine default. The dev mod binds it as a `hold` on Use's inputs, a shared key legal under today's rule; `input-contextual-priority` later adds an engine default with availability-ranked sharing and consumes this spec's candidate query.

## Scope

### In scope

- The `assistable: { radius, holdMs }` profile key on a Spec A life state, stamped per pawn, replicated on the pawn's movement state, and carried in the own pawn's `TuningPayload` profiles under a new `TUNING_PAYLOAD_EPOCH`.
- A gameplay input command `assist`, with no engine default, and its held bit on the wire.
- A host assist accumulator, with an owning-client prediction of the helper's own progress corrected at the acknowledged input tick, and a ramped display of received progress on the target's client. Target selection is a standalone pure query.
- Client retention of remote pawns' life-state id and `assistable` stamp.
- Discrete player-event sources `assistReceived()` and `assistGiven()`.
- Two per-player Number slots: `player.assistProgress` (progress as helper) and `player.assistReceivedProgress` (progress of the assist being received).
- Impact-policy source read and effect: `source.inLifeState(name)`, `source.setLifeState(name)`.
- Dev-mod upgrade: teammate revive, second-wind self-revive on kill, XP bonuses, "Reviving" and "Being revived" HUD bars.
- Updates to `docs/scripting-reference.md`.

### Out of scope

- Phases 2–4 as listed in Spec A's Out of scope.
- **Contextual command priority**, including `assist` grouped with `use` on one key so a press does whichever is available. `input-contextual-priority` owns it, along with the E-key press-time resolution and its HUD prompt. It consumes this spec's candidate query.
- **Phase 3 presentation for assist:** a revive prompt showing the nearest assistable teammate.
- **Not planned:**
  - Line-of-sight or facing requirements on assist. Optional profile keys can add them later.
  - Assist on non-pawn targets. Deferred until a non-pawn consumer exists; it would need its own source (see Alternatives).

## Direction

**Problem.** Spec A lets authors down and respawn a player, but nothing lets a teammate bring a downed player back, and impact policy cannot read or change its source's life state. Revive mechanics therefore cannot be authored.

**Placement.**
- **Engine mechanism vs mod policy.** The engine owns the assist accumulator: who is in range, how long `assist` is held, and one completion per target. These run per tick over pawn positions and held input, which the IR cannot see. The mod owns every consequence: the state a revive enters, health, bonuses, cues.
- **Host vs client.** Completion is host-authoritative. The helper's own progress bar is predicted on its client so host authority never shows up as latency. The prediction reads interpolated remote pawns, the remote pawns' life-state id and `assistable` stamp (replicated on their movement state, below), and the helper's own `use` capability. It reaches only the HUD's display map and is corrected toward the host. Success presentation plays only on host confirmation.
- **What the client knows about a teammate.** The host stamps each pawn's current `assistable` (or none) onto its movement component and replicates it on the pawn's movement state, beside the life-state id that already rides there. A connected client keeps both fields for remote pawns, which it drops today. The value is per pawn, so mixed player descriptors work and the client needs no other pawn's descriptor or profile table. The client's own pawn takes its `assistable` from its own profiles in `TuningPayload`, restamped like every other profile field.
- **Input resolution is client-side.** On a key shared by `assist` (`hold`) and `use` (`press`), as the dev mod binds it, the client's activator resolution decides: holding past the hold threshold drives `assist`; releasing before it fires Use. The client sends only the resolved command. The host never turns `assist_held` into Use or Use into an assist. If host and client disagree about range, the assist does not progress on the host and no trigger fires.
- **Impact policy vs player events.** Second wind is a consequence of a kill, so it is authored where kills are: an impact policy that reads and sets the *source's* life state.

**Prior commitments.**

- **Life states, capability gates, `setLifeState`, `respawn`, `inLifeState` and the name table** (Spec A). Kept. `assistable` is a closed profile key added to Spec A's set. Unknown names in the new read and effect fail at bind the way Spec A's `inLifeState` does: the mod name table refuses the name (`resolve_life_state_id` for the read, `EntityScope`'s name check for the effect), and the policy is skipped with a warning. Spec A's I3 (the engine never changes life state on HP) gains `source.setLifeState` as an authored transition source.
- **The client restamps only its own pawn's profile, from `TuningPayload`; movement payloads for pawns without a local `PlayerMovementComponent` are ignored** (`IgnoredPayload::MovementWithoutLocalComponent`, Spec A). Diverges, narrowly. The client keeps remote pawns' `life_state` and `assistable` fields from the payload it already receives. Gates still never run on remote pawns. The divergence is needed because the helper's prediction reads its target's state. `TuningPayload` gains no field: the pawn's own profiles it carries include `assistable` because they serialize `LifeStateProfile`, so `TUNING_PAYLOAD_EPOCH` bumps (Wire format).
- **Player events are host-only, and `on.player` is legal only before `wait`** (`scripting.md` §12). Kept. `assistReceived` and `assistGiven` are discrete sources that bind the existing `on.player` at completion; no `wait` is involved.
- **Every player event is an edge over a condition, and a fire's writes are seen next tick** (`scripting.md` §12 Player events). Diverges, narrowly. The assist sources carry no condition. Their fires apply before the pass's condition snapshot, so that tick's conditions see their writes (O1). Completions apply in `players()` order of the target; each runs its `assistReceived` subscriptions, then its `assistGiven` subscriptions, each in composed order. Condition-edge events still read the snapshot and fire after it. The divergence is needed because a revive must land before a same-tick bleed-out condition reads `down`.
- **Command relevance is an exhaustive match over descriptors; while participating in co-op it also follows the installed host tuning** (`input.md` §Relevance, `derived_relevance`, `tuning_facts`). Kept. `assist` gains an arm: relevant exactly when some loaded descriptor declares an `assistable` profile key, or, in co-op, when a life-state profile in the installed host tuning does.
- **Shared keys (Steam rule): one short binding plus one `hold` per key in a context** (`input.md` §Shared keys). Kept. The dev mod's `assist` (`hold`) beside `use` (`press`) on `KeyE` / `west` is a legal pair today (`conflicts` in `input/binding_table.rs`), the same pattern as its hold-Shift sprint beside tap-Shift dash. In that mod, Use on those keys fires on release before the hold threshold rather than on press. Kept as today: when a player's own layer binds Use on that input, `collides` drops the dev mod's lower-layer `hold` there, and that player has no `assist` on the key until they bind it.
- **Engine default bindings are all `press`** (`default_bindings_for`). Kept. `assist` gets no engine default here.
- **"Engine features per mechanic (a revive system, a lives system) put policy in the engine"** (Spec A, Alternatives rejected). Diverges, narrowly. The accumulator carries a small fixed rule set: nearest other pawn as target with ties to `players()` order, a required count of held ticks, reset on release, target switch, or either pawn changing or re-entering a state, advance only on real-command ticks, one completion per target per tick, and a completion ending the hold until a real release. The divergence is needed because range and hold time over per-tick pawn positions and held input are invisible to the IR, so no authored policy can compute them. Everything a revive *does* stays authored.
- **"Death is modder policy derived from impact"** (Q3 archive, E16). Kept. Self-revive is an authored impact effect, never an engine rule.

**Alternatives rejected.**

- **Assist driven by held Use.** A Use press beside a downed teammate would also fire any Use-activated trigger or switch in reach, and the host cannot tell which the player meant. A separate command resolved on the client keeps the two apart.
- **Teammates' `assistable` from `TuningPayload`.** The payload carries the client's own pawn's profiles. Serving teammates from it, the client would also need each remote pawn's descriptor identity and life-state id, and would carry profile tables it mostly never reads. A per-pawn stamp delivers the one answer the prediction needs.
- **No client prediction; the helper's bar shows the host's value.** The bar would start about one round trip late, against the smooth co-op presentation Spec A set as the governing preference.
- **An interim engine default (`hold` on Use's inputs).** Every mod declaring `assistable` would get Use on key-up, single player included, and the per-command default activators it needs would serve nothing once `input-contextual-priority` groups the two commands. The dev mod binds the hold itself, as it binds hold-Shift sprint.
- **A dedicated key in the dev mod.** Every gamepad face button already carries a gameplay command, and the free keyboard keys sit far from Use. L4D2, Back 4 Blood and Darktide all put revive on the use key.
- **One progress slot for helper and target.** Per-player slots are owner-private, so a target's client cannot read its helper's slot. A merged slot would also be ambiguous for a pawn that is helping while being helped. Two slots give each client its own reading.
- **A new `on.assister` token** for one `assisted()` event. A second dispatch token touches about a dozen hand-kept sites: SubjectToken, SequenceTarget, BoundTarget, DispatchScope's single event-player snapshot, `byPlayer` recognition in three installers, and five per-source rejection guards. Two discrete sources, each binding the existing `on.player` to a different pawn, deliver both bonuses with none of that.
- **Assist as a trigger volume attached to the pawn.** Trigger volumes are level-authored AABBs keyed by map identity. A pawn-carried one has no placement, tag, or install story.
- **A generic hold-to-interact mechanism with assist as its first target kind.** Pawn targets need the same accumulator either way. Assist stays player-on-player because no other hold-to-interact consumer exists; generalising later could reuse the accumulator and `assistGiven` (which binds `on.player` to the helper), but `assistReceived` binds `on.player` to a target pawn, so a non-pawn target would need its own source. That is why non-pawn assist stays Not planned rather than deferred.
- **Second wind without impact-source effects.** The kill policy could write a per-player flag through `update(store.byPlayer(source), …)`, and a player event on `inLifeState("down")` plus the flag could revive. Without `source.inLifeState` the policy cannot restrict the flag to kills made while down, so the author must clear it on every down transition. The revive also lands a tick later than an in-tick `source.setLifeState`. Task 1 is cheaper for authors and faster.
- **Revive as an engine verb.** A revive is `setLifeState` plus `grantHealth` in an authored fire list; an engine verb would fix its health and state.
- **A respawn hook in `sim::respawn::drain`.** `drain` takes only the `ScriptCtx` and registry and cannot reach the session-owned accumulator state. The accumulator's elapsed check already sees every respawn into a declared state, since respawn enters it at elapsed zero.

**Foreclosures and one-way doors.**

- `assist_held` on `WireMovementInput` and `assistable` on `WirePlayerMovementState` take the next `WIRE_VERSION` (39 at `b6fc0c4`), `assistable` on `WirePlayerMovementState` also takes the next `SNAPSHOT_VERSION` (25), and `assistable` on `LifeStateProfile` takes the next `TUNING_PAYLOAD_EPOCH` (17). Pre-stable, each undone with another bump.
- SDK names (`assistable`, `assistReceived`, `assistGiven`, `player.assistProgress`, `player.assistReceivedProgress`, `source.inLifeState`, `source.setLifeState`) and the command ID `assist` become the modder contract; the command ID is also written into player bindings and manifests. Renaming any of them costs a pre-stable SDK break across both runtimes.
- `assist` has no engine default until `input-contextual-priority` adds one. The dev mod's hold-E binding is content, replaced when that spec lands; a player who rebound `assist` keeps their binding.
- Task 2a widens the per-player slot bitset (`EntityScope.warned_owned_engine`, capped at sixteen by a compile-time assert) to u32. The slot count is fifteen (`PlayerSlot::ALL` at `b6fc0c4`); the two progress slots take it to seventeen, so the widening is required for this spec to compile.
- An assist with no line-of-sight or facing requirement can be tightened later by adding optional profile keys. `holdMs` belongs to the target's state, so a helper-side speed modifier (a revive perk) needs a later helper-profile key; it is additive. Nothing material is foreclosed.

## Acceptance criteria

### Descriptor and validation
- [ ] **AC1.** A life-state profile accepts `assistable: { radius, holdMs }` in both runtimes into the same lowered shape. Loading is rejected, with the full key path, for an `assistable.radius` ≤ 0 or non-finite, an `assistable.holdMs` < 0 or non-finite, and an unknown key inside `assistable`. A connected client refuses a tuning payload whose profile carries the same faults (non-finite values are checked by calling `validate_life_states` directly, since JSON cannot carry them), and a movement state whose `assistable` carries a non-finite float fails validation. On every role, a pawn's movement component carries its current state's `assistable` (or none), restamped at every life-state write, and a connected client reads the same value for a remote pawn.
- [ ] **AC2.** An impact policy whose `source.inLifeState` or `source.setLifeState` names a state absent from the mod's name table is skipped at bind with a warning naming the policy and the state; the mod's other policies still bind. At runtime, `source.setLifeState` on a source whose descriptor lacks that state, or declares no states, warn-skips.

### Impact source read and effect
- [ ] **AC3.** In an impact policy, `source.inLifeState(name)` holds exactly while the source pawn is in `name`. It is false for a non-player source, which reads no warning. `source.setLifeState(name)` applies to the source pawn in-tick with Spec A's `setLifeState` semantics: the next tick's gates read the new profile, elapsed time restarts, and the current state is a no-op. A live source that is not a player pawn warn-skips through `set_life_state`'s missing-state warning; an impact with no live source skips silently, as every source effect does.

### Assist
- [ ] **AC4.** Holding `assist` within `radius` of another pawn whose current state declares `assistable` completes on the held tick that reaches the required count (AC5). The completion fires every `assistReceived()` subscription with `on.player` = target, then every `assistGiven()` subscription with `on.player` = helper. Both fire in the player-event pass of the tick the hold completes, before that pass's condition snapshot. Completion itself changes no life state or health; only the authored fire lists do.
- [ ] **AC5.** Paired refuse/permit edges for assist:
  - **Refused:** out of range; target in a non-assistable state; helper's state has `use: false`; helper and target are the same pawn (a pawn whose state permits `use` and declares `assistable` never assists itself); release one held tick before the required count.
  - **Permitted:** in range on an assistable state; `holdMs: 0` completes on the first held tick.

  Progress counts held ticks. The **required count** is `ceil(holdMs × 1000 / TICK_DURATION)`, with `postretro_foundation::TICK_DURATION` (16 667 µs) in microseconds, and at least one. A hold completes on the held tick whose count reaches it, and progress reads the count over the required count, capped at 1. Progress resets when the helper releases or switches targets, when either pawn changes or re-enters a life state (respawn included), and when the hold completes. After a completion the helper accumulates again only after releasing and holding `assist` anew. `player.assistProgress` reports the pawn's progress as helper, 0..1. `player.assistReceivedProgress` reports the progress of the assist the pawn is receiving, 0..1: the highest among its helpers, and 0 with none. A pawn helping while being helped reports both.
- [ ] **AC6.** Two helpers completing on one target in one tick produce one completion, for the earlier helper in `players()` order. The other helper's progress resets. Held `assist` survives a short input gap under the host's hold-then-neutral gap policy: gap-held ticks keep progress without advancing it, and only ticks driven by a real command advance it.

### Input command
- [ ] **AC7.** Absent a manifest `show` override, `assist` is listed and bindable exactly when some loaded descriptor declares `assistable` or, while participating in co-op, a life-state profile in the installed host tuning does; otherwise it is unbound, unlisted and never part of a conflict. It has no engine default. Bound to a key of its own (`press` or `hold`), holding the key sets `assist_held` on every command built while it is down, and releasing clears it.
- [ ] **AC8.** With `assist` bound as a `hold` on a key that also carries Use's `press` (the dev mod's binding on `KeyE` and gamepad `west`), holding the key past the hold threshold drives assist and fires no Use-activated trigger or switch, with or without a teammate in range. Releasing before the threshold fires Use once. The pair raises no binding conflict. Engine default bindings are unchanged: with the engine defaults alone, Use fires on press exactly as today.

### Client experience
- [ ] **AC9.** The helper client's "Reviving" bar shows its own `player.assistProgress` on the first held tick, with no round-trip wait, and advances every rendered frame. If the host resets or never confirms (target moved, revived elsewhere, out of range on the host), the bar returns to 0 with no success cue. Success consequences (state, health, bonuses, their presentation) appear only after host confirmation; this clause and "no success cue" are a review gate plus the manual proof, since cues live only in host-run fire lists. After the first nonzero sample of each assist (a sample above 0 following a 0), which may step up from 0, the target client's displayed `player.assistReceivedProgress` never decreases between rendered frames unless a received sample is below its predecessor, and rises by at most twice the target's slope (one increment of its required count per server tick) over the frame's timeline advance.

### Orderings and consumer
- [ ] **AC10.** Every Orderings row in each task section has a test asserting its expected outcome; the test names the row id in its name or doc comment. A row whose halves span tasks is tested by each task for its half. Checked by review.
- [ ] **AC11.** The dev mod upgrades Spec A's down/bleed-out policy, with `player-death.ts` / `player-death.luau` as twins (the descriptor, HUD and input rows are TS-only) and the parity surface test extended:
  - `down` declares `assistable: { radius: 1.5, holdMs: 3000 }`
  - teammate hold-`assist` → `alive` with 30 health, and an XP bonus to the helper
  - an enemy kill while down → `alive` with a health and XP bonus
  - a "Reviving" HUD bar on `player.assistProgress` and a "Being revived" HUD bar on `player.assistReceivedProgress`

  The two-player test adds down → assist → alive and down → self-revive beside down → bleed-out → respawn.
- [ ] **AC12.** The docs need these changes:
  - `docs/scripting-reference.md` documents `assistable`, the `assist` command, its default and when it is relevant, the assist sources, `player.assistProgress`, `player.assistReceivedProgress`, `source.inLifeState` / `source.setLifeState`, and the full Borderlands and L4D-style recipes.
  - It drops its stale claims that per-owner + persist is rejected and that there is "no source-scoped fact vocabulary". The `SourceHandle` doc comments in `sdk/lib/data_script.ts`, its `.luau` twin, the `sdk_lib` typedef templates and the regenerated types drop the latter claim too (grep gate).
  - Every command shown runs from an SDK bundle.

## Orderings

Each Orderings row lives in an Orderings table inside the task section that owns and tests it; row ids are unique across the spec, and every row's expected outcome is binding.

## Tasks

New logic lands in new modules. Files already past ~800 lines (`sim/src/sim/mod.rs`, `sim/src/impact_policy.rs`, `netcode/src/lib.rs`, `netcode/src/endpoint.rs`, `netcode/src/client.rs`, `netcode/src/command_queue.rs`, `ir/scopes.rs`, `netcode/src/state_slots.rs`, `startup/reaction_validation.rs`, `main.rs`, `frame_loop/mod.rs`) receive only call-site edits and match arms. Symbol grounding: `research.md` in this folder.

### Task 1: Impact-policy source reads and effects

- **`source.inLifeState(name)`.** `EntityScope::resolve_owned_input` gains a life-state branch for `owner:"@impact.source"` names with `LIFE_STATE_INPUT_PREFIX`, resolving the id through `resolve_life_state_id`. `seed_owner_seat_from_registry` also captures the source pawn's `PlayerMovementComponent.life_state`, which the owner snapshot does not hold today. The read is false, without a warning, for a non-player source.
- **`source.setLifeState(name)`.** `bind_effect` accepts it for `IMPACT_SOURCE_TOKEN` only, reading the name from `args.state`. `resolve_life_state_id` is `pub(crate)` in `scripting-core`, so `EntityScope` gains a public method reporting whether the mod name table holds a name. The method reads `data_registry.life_state_names` without logging, so only `bind_effect`'s error, surfaced through `bind_policy`'s warning, reports an unknown name; `bind_effect` returns that error, naming the state, when the method reports the name absent. Add `SetLifeState { state }` to `BoundEffect` in `impact_policy.rs`, with its `plan_effect` arm and `evaluate_dispatch`'s consequential lane, and to `ImpactEffect` in `crates/sim/src/impact_effects.rs`. `apply_planned` intercepts it the way it intercepts `SetOwnerSlot` (resolving the recipient through `resolve_command_recipient`) and calls `life_state_commands::set_life_state` with its `ctx` and registry, so elapsed reset, the same-state no-op and the weapon-activation cancel hold. `apply_effect` (`impact_effects.rs`) marks it `unreachable!` like `SetOwnerSlot` and `AdjustSentiment`.
- **SDK.** Add `inLifeState` and `setLifeState` to `SourceHandle` in `sdk/lib/data_script.ts` / `.luau` and the `sdk_lib` typedef templates (`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` / `sdk_lib.luau`), and a `setLifeState` arm to `ImpactEffectWire`. `source.inLifeState(name)` lowers to `{ op: "input", owner: "@impact.source", name: "player.inLifeState:<name>" }` and `source.setLifeState(name)` to `{ primitive: "setLifeState", target: "@impact.source", args: { state } }`; Luau spells them `source:inLifeState(name)` / `source:setLifeState(name)`. Drop the now-false "no source-scoped fact vocabulary" sentence from the `SourceHandle` doc comment in the same files, so the regenerated types are right at this task's merge. Then regenerate types and fixtures.
- **Unknown names.** An unknown state fails `bind_policy`, which skips the policy with its existing warning (AC2). The warning names the policy and the state.
- **Merge coordination.** This task runs concurrently with Task 2a. Both edit `crates/scripting-core/src/ir/scopes.rs` (this task the life-state branch and name check, Task 2a the `warned_owned_engine` bitset width), `sdk/lib/data_script.ts` / `.luau` and the `sdk_lib` typedef templates, and both regenerate `sdk/types/postretro.d.*` and the typedef `expected.d.*` fixtures (`crates/sim/src/scripting/typedef/tests/fixtures/`). Hand-merge only the hand-written files. After merging the other task, re-run the SDK type and fixture regeneration rather than merging generated output.
- **Test** AC2 and AC3 with the `impact_policy` test helpers. Test O2 and O7 in a harness built on `player_events::tests::World` and `life_state_commands::tests::install_states`, as `player_events/life_state_ordering_tests.rs` does, with an `ImpactPolicyRuntime` sharing that `ScriptCtx`. For O7, dispatch the self-revive impact between the sim tick and `PlayerEventTable::run_tick`. For O2, dispatch it after the bleed-out fire and the next tick's respawn drain.

**Orderings**

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O2 | Self-revive kill from a remote hit still waiting on fire authorization, landing after bleed-out expired | Bleed-out fires in tick N's player-event pass, respawn drains at tick N+1 start, hit flushed by tick N+k's post-tick flush (k ≥ 1, in the same frame or a later one) | Respawn wins. The later `source.inLifeState("down")` is false and the self-revive row does not fire |
| O7 | Self-revive remote hit flushed in the same tick bleed-out expires | Tick N: sim, then `host_flush_pending_hit_declarations` runs `source.setLifeState("alive")`, then the player-event pass whose bleed-out condition would turn true at N | Self-revive wins. The bleed-out row reads `inLifeState("down")` false and does not fire, and no respawn is queued |

Delivers AC2, AC3, the `SourceHandle` doc-comment change in AC12, O2 and O7.

### Task 2a: Assist host, input and wire

This task builds host and single-player assist and every surface the client presentation reads. Task 2b, which runs after it, calls `assist::step`, `assist::assist_candidate`, `ClientReplication::remote_assist_facts` and the accumulator's per-helper advanced flag unchanged, so keep them `pub`.

**Profile key and stamp**
- Add `assistable: Option<Assistable { radius, hold_ms }>` to `LifeStateProfile` (`foundation/src/movement/life_state.rs`). The new struct uses serde `rename_all = "camelCase"`, so the payload spells `holdMs`, and `deny_unknown_fields` like `LifeStateProfile`, so a tuning payload with an unknown key inside `assistable` fails to deserialize on the client. Add the key with its range checks to Spec A's shared validator, `crates/scripting-core/src/data_descriptors/life_states.rs`, whose key allow-lists give each rejection its full key path (AC1).
- `TuningPayload.life_state_profiles` serializes `LifeStateProfile` itself (`tuning_payload_for_pawn`), so `assistable` rides it with no payload code. `LifeStateProfile` denies unknown fields, so a reader on the old epoch rejects the payload: bump `TUNING_PAYLOAD_EPOCH` (`combat-model/src/carried_loadout.rs`) with a history line, and extend `validate_life_states` (`netcode/src/tuning_payload.rs`) to refuse a non-finite or non-positive `radius` and a non-finite or negative `holdMs` (AC1). The client restamps its own pawn's `assistable` from these profiles like the other profile fields.
- Stamp the current state's `assistable` onto `PlayerMovementComponent` in `restamp_life_state`, so every Spec A stamp site sets it, and copy it in `carry_life_state_stamps` across a table rebuild. Carry it on `WirePlayerMovementState` beside `life_state` in `movement_state_to_wire`, as `assistable: Option<WireAssistable { radius: f32, hold_ms: f32 }>`, `None` when the current state declares none. Extend `WirePlayerMovementState::all_finite` (`net/src/wire/components.rs`) to require both floats finite when present. The local pawn's wire merge ignores the wire field: its stamp stays derived from the installed profile, so `gate_merged`'s preserve path keeps the pre-merge stamp when a record's table or profile is unavailable. Only the remote retention map below reads the wire field. Bump `WIRE_VERSION` (`net/src/handshake.rs`) and `SNAPSHOT_VERSION` (`net/src/wire.rs`), each with a history line.
- **Remote retention.** On a connected client, keep `life_state` and `assistable` for remote pawns from the movement payload `client.rs` drops today as `MovementWithoutLocalComponent`. Retain them in a map keyed by `NetworkId` on `ClientReplication`, beside `remote_player_locomotion`. Each accepted payload overwrites its entry. `clear_identity_state`, local-pawn arming and both full clears (`reset_for_level_unload`, `reset_for_demotion`) remove or clear this map as they do `remote_player_locomotion`. `cache_remote_player_locomotion` leaves it alone: its removal for a meshless descriptor is a presentation choice, not the end of an identity. No `PlayerMovementComponent` is constructed and no gate runs on them. The payload still reports `MovementWithoutLocalComponent`, since no component merge happens. Retained ids are compared only for equality, never resolved through a name table.
- **Retention accessor.** Add `pub fn remote_assist_facts(&self, entity_id: EntityId)` to `ClientReplication`, returning a remote pawn's retained life-state id and `assistable` stamp. It resolves the `EntityId` to its `NetworkId` through the replication's reverse map, as `network_id_for_entity` does, and returns nothing for an unmapped entity or one with no entry. The app reaches it through `NetEndpoint::Client { replication, .. }`, as `main.rs` reads `entity_network_ids()`.

**Input command**
- Add `Command::Assist` and `Action::Assist`: `Command::ALL`, `id`, `context`, `kind`, an `accepts` arm for `Press | Hold` (the accumulator reads a level), `gameplay_target`, the `activator_acceptance_per_command` expectation in `input/commands.rs` (`Press, Hold`), and the `CommandId` list in `sim/src/scripting/primitives/manifest.rs` (then regenerate types). Leave the legacy default tables in `input/defaults_tests.rs`, and so `legacy_actions`, unchanged: `assist` had no pre-command default.
- Add a relevance fact and its `derived_relevance` arm: relevant exactly when some loaded descriptor declares `assistable` (AC7). `registry_facts` derives it from local descriptors, and `tuning_facts` from the installed host tuning's `life_state_profiles`, which carry `assistable`, per `input.md`'s co-op relevance rule.
- Add `assist_held: bool` to `WireMovementInput` and to `SimCommand` beside its top-level `use_pressed` (not `MovementInput`; both runners read it from `SimCommand`), threaded through `build_sim_command` (`main.rs`), `sim_command_to_input` / `input_command_to_sim` and `neutral_sim_command` (false). `frame_loop` reads `Action::Assist`'s level from the input snapshot, as it reads `Action::Sprint` for `resolve_sprint_intent`, and passes it to `build_sim_command`. `held_gap_sim_command` carries it through a gap like other levels, not clearing it as it clears edges. The accumulator advances progress only on a tick whose command is real: a remote pawn's `RemotePawnCommand::real_command`, while the local pawn's command, in single player and on a listen host, is always real. A gap-held tick keeps progress without advancing it, so a release whose command arrives late never completes on held ticks, and a frontier freeze that consumes one client tick twice counts it once (O11). A neutral command after the hold grace resets progress as a release does but leaves an ended hold ended (the step table's row 2; O12, O29). A remote pawn with no resolved command holds nothing.
- `engine_default_inputs(Command::Assist, class)` returns no inputs for either device class. Test AC8 with an author binding of `assist` as `hold` on `KeyE` beside the default Use.

**Accumulator**
- A host and single-player accumulator in new module `sim/src/sim/assist.rs`. It runs in the fixed tick after the Weapons stage and `run_death_sweep`. Its cross-tick state is session-owned, beside `touch_system`, and passed to the sim tick as `&mut` through `simulate_started_tick_with_presentation_aim`'s parameters. `simulate_tick` and the test wrappers that build `TouchSystem::default()` build a default accumulator state the same way. The state is level-scoped: `unload_level` clears it beside its `respawn_placements::clear_for_level_unload` call (which takes only the `ScriptCtx`), so no progress carries into the next level when an entity id recurs or a target arrives carried into its assistable state (O21). The surface teardown that clears `touch_system` also runs on suspend, so it does not clear this state: the suspended world stays live.
- Each helper whose state allows `use` (`player_life::permits` with `Capability::Use`) and who holds `assist` accumulates on the nearest other pawn within that pawn's `assistable.radius`. A pawn is never its own candidate, so a pawn whose state permits `use` and declares `assistable` never assists itself (AC5). Distance is between pawn origins; ties go to the earlier pawn in `players()` order.
- Target selection is its own pure function (`assist_candidate`): the helper's id, position and capability, and the candidates' ids, positions, life-state ids and `assistable` stamps in, chosen target out. It skips the helper's own id. `input-contextual-priority` calls it for availability.
- **Required count.** Progress counts held ticks, not summed fractions. A target's required count is `ceil(holdMs × 1000 / TICK_DURATION)`, with `postretro_foundation::TICK_DURATION` (16 667 µs, the engine's fixed step) in microseconds, and at least one. This is the conversion `wait` enrollment uses (`startup/reaction_validation.rs`). A hold completes on the held tick whose count reaches the required count, so `holdMs: 0` completes on the first held tick. Reported progress is the count over the required count, capped at 1. **One increment** is one held tick. Host and client use this one function.
- Progress resets per AC5. The state stores, per pawn, its life-state id and `life_state_elapsed_ticks` at the last pass. Elapsed advances once per host tick for every pawn in a declared life state, and entering a state, respawn included, sets it to zero. A pawn whose descriptor declares no states stays at zero, so the check compares its id alone; such a pawn can only be a helper, and a respawn mid-hold keeps its progress. When a pawn's id differs from the stored one, or it is in a declared state and its elapsed is not greater than the stored value, the accumulator resets that pawn's progress in both roles: its own progress as helper, and every helper's progress on it as target. This covers a respawn into the same state (O25) and a leave-and-re-enter between passes (O9), with no hook in `sim::respawn::drain`. The step's elapsed input is optional: with none, the check compares ids alone (Task 2b's client runner retains no remote elapsed). A pawn removed from the registry (seat harvest, O5, O19) drops its progress in both roles. A pawn that appears as a new entity starts from 0 (O20).
- Completions go into a `TickEvents` `assist_completions` list (precedent `trigger_exit_fires`), each carrying the target's life-state id and `life_state_elapsed_ticks` at record. One completion per target per tick (AC6): the earlier helper in `players()` order wins and the others reset, unlatched (O37). A completion ends the winning helper's hold by setting its latch (below): its progress resets to 0 and accumulates again only after `assist` is released and held anew (O4, O10). Only a real command with `assist_held` false counts as that release. A neutral command synthesized after the hold grace resets progress (O12) but leaves the latch set (O29).
- **One rule, two runners.** The accumulator is a pure `pub` step, `assist::step`, over the runner (host or client), the helper's `assist` level, whether that tick's command is real, the helper's capability, the candidates, the previous target and held-tick count, the helper's latch, and each pawn's life-state id and optional elapsed at the previous pass. The latch marks an ended hold. With `assist` held and the latch clear, the step first chooses a target with `assist_candidate`, then resets the count to 0 when that target differs from the previous one or the reset rule above fires for either pawn. The first matching row then applies:

  | # | `assist` held | Command real | Latch | Target | Step |
  |---|---|---|---|---|---|
  | 1 | No | Yes | Any | Any | Progress resets to 0; the latch clears (a release) |
  | 2 | No | No | Any | Any | Progress resets to 0; the latch is kept (a neutral command after the hold grace, or a remote pawn with no resolved command; O12, O29) |
  | 3 | Yes | Any | Set | Not chosen | Progress does not advance. The host's stays at the 0 its completion left; the client's holds its current value until the correction settles it |
  | 4 | Yes | Any | Clear | None | Progress resets to 0 |
  | 5 | Yes | No (gap-held) | Clear | Chosen | Progress kept, not advanced (O11) |
  | 6 | Yes | Yes | Clear | Chosen | The count advances one increment. On reaching the target's required count the host records a completion, resets progress to 0 and sets the latch; the client holds at 1 and sets the latch |

  The **step's reset** is a reset the step itself makes: rows 1, 2 and 4, and the reset before the table. The host's completion reset in row 6 is not one. The step returns the new count, latch and target, the row it applied, whether the step's reset fired, and whether it completed.

  The host runs the step authoritatively, once per tick for each helper. Task 2b runs it on the owning client for its own pawn; rows 3 and 6 name the client runner's behavior for it. The accumulator records, per helper, whether its last pass ran row 6 without completing; Task 2b's listen-host and single-player display reads that flag.

**Sources**
- `PlayerEventTable::run_tick` takes the tick's `assist_completions` and applies them before `evaluate()`, in `players()` order of the target. Each completion runs its `assistReceived` subscriptions, then its `assistGiven` subscriptions, each in composed order. Each completion is re-checked immediately before its own application, not all before the first, and dropped unless the target still holds the life-state id recorded with it, its `life_state_elapsed_ticks` still equals the value recorded beside that id, and that state declares `assistable` (I1, O23). A move to another assistable state between record and apply drops it, matching AC5's reset on a state change. Elapsed does not advance between the accumulator and the pass, so a leave and re-entry in the flush shows as elapsed reset to zero and drops it too (O34). A completion recorded at elapsed zero, possible only when one increment reaches the required count (a required count of one, `holdMs: 0` included) on a target that entered its state that tick, cannot tell the two apart and applies. A dropped completion still ends the winning helper's hold, since the accumulator latched it at record: that helper accumulates again only after a release and re-press, as after a confirmed one (O33). The helper is not re-checked: a helper downed in the flush still earns its `assistGiven` (O38). Test the re-check with a direct state change between record and apply; Task 3 runs the flush case end to end. With no completions, `run_tick` behaves as before (O24).
- The production call site of `run_tick` is the player-event pass in `frame_loop/mod.rs`, fed from that tick's `tick_events`. The other callers (`startup/reaction_validation.rs`, `host_activations/conditioned_tests/life_states.rs`, `netcode/src/ingest_hit_harness_test.rs` and the sim test wrappers) pass an empty list.
- `BoundPlayerEvent` gains a trigger kind: `Edge { condition, edge, program, memory, … }` or `AssistReceived` or `AssistGiven`. Discrete kinds carry no condition, program or memory. Its existing `source` field is the entry's provenance (`PlayerEventSource`) and keeps that name and meaning.
- `PlayerEventDescriptor` (`entities/src/data_descriptors/types/player_events.rs`) carries the same trigger kind in place of its required `edge` and `condition`. `player_event_from_json` accepts `edge: "assistReceived" | "assistGiven"` with no condition; today it accepts only `becomes` or `ceases`, with a condition. Every descriptor consumer follows the kind: player-event install, staged-manifest transfer, the manifest typedef and `data_registry.rs`. In install, a discrete entry's duplicate-binding key is (kind, address), so one reaction bound twice under `assistReceived()` binds once with the existing warning; discrete kinds keep no edge memory across a recompose.
- SDK `assistReceived()` / `assistGiven()` in both runtimes (exported from `sdk/lib/data_script.ts` / `.luau` beside `becomes` and `ceases`) and the type declarations, then regenerate.
- Fire-list validation is the player-event rule set unchanged. Each fire binds `on.player` to the target or the helper respectively.

**Progress slots**
- Add `player.assistProgress` and `player.assistReceivedProgress` as `PlayerSlot`s (seventeen slots), at the sites `PlayerSlot::LifeStateMs` touched: `player_slots.rs` arms, an `engine_state_catalog.rs` entry with `OwnerPrivatePlayer`, the `ui_proxy.rs` publisher (writing both only off a connected client, as it gates `player.lifeStateMs`, so a client's unwritten component never overwrites a host sample), `MIRRORED_GAME_STATE_LEAVES`, regenerated types and `expected.d.*` fixtures.
- Both values live on the pawn. `player_slot_value` (`player_slots.rs`) reads every non-weapon slot straight from the pawn's components, and owner-private replication (`owner_private_source_value`), the host HUD publisher, player-event reads and `byPlayer(source)` reads all go through it; no slot-table write reaches them. Add `assist_progress` and `assist_received_progress` to `PlayerMovementComponent`. The accumulator writes both every tick per AC5, and the two new `player_slot_value` arms read them the way the `LifeStateMs` arm reads `life_state_elapsed_ticks` (`reads_active_weapon` false). A pawn with no assist activity reads 0 for both. The fields stay off `WirePlayerMovementState`; clients receive the values as owner-private samples.
- Widen the `warned_owned_engine` bitset in `ir/scopes.rs` from u16 to u32, with its compile-time assert and the `1u16 << slot.index()` shift in `read_owned_engine`.

**Merge coordination.** This task runs concurrently with Task 1. Both edit `crates/scripting-core/src/ir/scopes.rs` (this task the bitset width, Task 1 a life-state branch and name check), `sdk/lib/data_script.ts` / `.luau` and the `sdk_lib` typedef templates (`crates/scripting-core/src/typedef/templates/`), and both regenerate `sdk/types/postretro.d.*` and the typedef `expected.d.*` fixtures (`crates/sim/src/scripting/typedef/tests/fixtures/`). Hand-merge only the hand-written files. After merging the other task, re-run the SDK type and fixture regeneration rather than merging generated output.

**Tests**
- Accumulator: AC4, AC5, AC6 and rows O4, O5, O8, O9, O10, O11, O20, O21, O25, O29 and O37, the host halves of O12, O19 and O22 (Task 2b tests their bar halves), and the helper half of O33, with each step-table row driven directly under both runners and with an absent elapsed comparing ids alone. Also: a helper whose own state permits `use` and declares `assistable` never selects itself; the required count for `holdMs` 0, 3000 and a value that is not a whole number of ticks, with a release one held tick before it refused; and the host half of Task 2b's O28, where after a completion on A the host accumulates nothing on a second downed target B in range until a release and re-press. O8 and O9 drive each self-revive through Spec A's `set_life_state` directly, since Task 1's `source.setLifeState` lands concurrently.
- Player-event pass: rows O1 (including several consecutive ticks with no frame-end drain between them, asserting the revive lands in the first tick's pass), O23, O24, O33, O34 and O38.
- Input: AC7, AC8 and row O26, and `held_gap_sim_command` carries `assist_held` and still clears `use_pressed`.
- Profile and stamp: AC1, including validation in both runtimes, a tuning payload `validate_life_states` refuses (non-finite values by calling `validate_life_states` directly, since JSON cannot carry them), a payload with an unknown key inside `assistable`, a movement state whose `assistable` carries a non-finite float failing `all_finite`, the own pawn's stamp restamped from its profile, kept across a table rebuild and never taken from the wire, and a remote pawn's stamp retained, read back by `EntityId` through `remote_assist_facts`, kept when `cache_remote_player_locomotion` drops locomotion, and cleared at the identity and full-clear sites.
- Progress slots: `ui_proxy` on a connected client writes neither `player.assistProgress` nor `player.assistReceivedProgress`, so a committed host sample survives the next frame's publish, as `ui_proxy_life_state_tests.rs` tests `player.lifeStateMs`.

**Orderings**

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Assist completes the same tick bleed-out would expire | Assist in tick, completions applied before the player-event snapshot; the tick may be the first of several in one frame | Revive applies first, in that tick's pass rather than the frame's last tick; the bleed-out condition reads `inLifeState("down")` false and does not fire |
| O4 | `holdMs: 0` | First held tick, helper keeps holding | Completes that tick, once. Holding on completes nothing more until `assist` is released and held anew, even when the fire list leaves the target assistable |
| O5 | Helper disconnects mid-hold | Seat harvest between frames | Progress dropped. The target is unchanged |
| O8 | Self-revive by an in-tick hit in the tick a helper would complete | Tick N: Projectiles or Weapons stage self-revive, then the accumulator after `run_death_sweep` | The target is `alive` at the accumulator pass. No completion is recorded, every helper's progress on it resets, and no assist source fires |
| O9 | Target leaves and re-enters its assistable state between accumulator passes | Tick N accumulator (helper near `holdMs`), then the flush self-revives the target, a lethal late hit and the death sweep follow, then the pass's down row re-enters `down` | Tick N+1: the target's elapsed has not advanced, so every helper's progress on it restarts from 0. No completion comes before a further full required count (AC5) |
| O10 | Completion whose fire list leaves the target assistable | Tick N completion, heal-only fire list, helper keeps holding | One completion at N. None while the helper keeps holding; after a release and re-press, the next comes after another full required count of held ticks (AC5) |
| O11 | Late release command across an input gap | Helper releases one tick before `holdMs`. The host frontier-freezes on the previous held command, then consumes the release | No completion. Gap-held ticks keep progress without advancing it, so host and client tick counts agree |
| O12 | Gap longer than the hold grace | `INPUT_HOLD_TICKS` held ticks, then neutral, then the real stream resumes held | The neutral tick resets progress as a release does but leaves the latch as it was. The resumed hold starts from 0. The helper's bar drops through the correction table's drop row at the acknowledged tick, then rebases from the first host sample above 0 |
| O19 | Target disconnects mid-hold | `net_poll_and_apply` harvests and despawns the target before tick N | The helper has no target at tick N; its progress resets and nothing is recorded. The helper's bar drops through the host sample |
| O20 | Target rejoins mid-hold, restored into `down` | Despawn, then rejoin spawns a new pawn carried into `down` while the helper holds | Progress on the new pawn starts from 0, as a target switch |
| O21 | Level change mid-hold | Unload, then install with the target carried into `down` and the helper holding | First tick of the new level: progress 0, and no completion before a full required count (AC5) |
| O22 | Hot reload removes `assistable` from the target's state mid-hold | Reload restamps between frames, then the next tick runs | The next accumulator pass finds no target and resets progress. The client's retained stamp updates from the next movement payload and its prediction stops |
| O23 | Completions on different targets in one tick | The accumulator records C1 and C2, then the pass applies them before `evaluate()` | Applied in `players()` order of the target. C1's `assistReceived` and then `assistGiven` subscriptions fire before C2 is re-checked |
| O24 | No completions in a tick | Empty `assist_completions` | `run_tick` behaves exactly as before this spec, and existing player-event tests pass with only an empty completions argument added |
| O25 | Helper in a declared life state respawns within radius of its target mid-hold | Tick-start drain relocates the helper, still within `radius` and holding | The helper's elapsed has not advanced, so its progress restarts from 0. A helper whose descriptor declares no life states keeps its progress across the same respawn |
| O26 | Focus loss mid-hold | `cancel_all` lifts `assist` without a pulse | Progress resets as on a release, and Use does not fire |
| O29 | Gap past the hold grace after a completion | Completion at N, helper holds; `INPUT_HOLD_TICKS` gap-held ticks, then synthesized neutrals; the real stream resumes held | No second completion. The neutrals reset progress but leave the latch set; progress accumulates only after a real release command and a re-press |
| O33 | Completion dropped while the target stays assistable | Tick N records helper H's completion on T in `down`; the flush moves T to another assistable state; H and a losing helper keep holding | No assist source fires. H accumulates only after a release and re-press; the losing helper accumulates from 0 |
| O34 | Leave and re-entry of the recorded state inside the flush | Tick N records a completion on T in `down`; the flush runs `source.setLifeState("alive")`, then `source.setLifeState("down")`, on T | The re-check sees elapsed reset to zero and drops the completion; no assist source fires |
| O37 | Two completions on one target from successive holds | Helpers A and B hold on T and both reach `holdMs` at N; A wins at N with a heal-only fire list; B, reset at N per AC6, keeps holding | B completes no earlier than N plus the required count (AC5); A completes nothing more without a release and re-press |
| O38 | Helper downed in the flush of its completion tick | Tick N records helper H's completion on T; a late hit kills H in the flush; the pass applies completions, then H's down row | T's `assistReceived` and H's `assistGiven` fire, then H goes down in the same pass |

Delivers AC1, AC4, AC5, AC6, AC7, AC8, O1, O4, O5, O8, O9, O10, O11, O12, O19, O20, O21, O22, O23, O24, O25, O26, O29, O33, O34, O37 and O38.

### Task 2b: Assist client presentation

This task builds the connected client's assist bars on Task 2a's surfaces, which it calls unchanged:
- `assist::step` (`sim/src/sim/assist.rs`), the pure accumulator step. It takes the runner (host or client), the helper's `assist` level, whether the command is real, the helper's capability, the candidates (id, position, life-state id, `assistable` stamp, optional elapsed), the previous target and held-tick count, and the latch. It returns the new count, latch and target, the row it applied, whether the step's reset fired, and whether it completed. Its rows: 1, a release (reset, latch clears); 2, a non-real release (reset, latch kept; never on the client); 3, held while latched (no advance; the client runner holds its value until the correction settles it); 4, held with no target (reset); 5, gap-held (kept; never on the client); 6, held on a real command with a chosen target (one increment; on reaching the required count the client runner holds at 1 and sets the latch). The **step's reset** is a reset the step itself makes: rows 1, 2 and 4, and the reset on a target switch or either pawn's state change before the rows.
- The **required count**, `ceil(holdMs × 1000 / TICK_DURATION)` held ticks with `TICK_DURATION` in microseconds, at least one. Progress is the count over it, capped at 1. **One increment** is one held tick.
- `ClientReplication::remote_assist_facts(entity_id)`: a remote pawn's retained life-state id and `assistable` stamp, or nothing. Retained ids are compared only for equality.
- The accumulator's per-helper flag recording that its last pass ran row 6 without completing.
- The owner-private slots `player.assistProgress` (the pawn's progress as helper) and `player.assistReceivedProgress` (the highest progress among its helpers), which `ui_proxy` writes only off a connected client, and the local pawn's `PlayerMovementComponent` `assistable` stamp, restamped from `TuningPayload`.

**Client step**
- **Where it runs.** The client step, its records, its follow state and the correction live in new module `crates/postretro/src/client_assist.rs`, not in netcode: netcode's forward prediction takes no `EntityRegistry` by design, and the step reads interpolated remote pawns from the registry. It runs inside the `predict` closure that `frame_loop` passes to `run_client_predicted_tick`, beside `predict_client_weapon_command`, with that tick's command and `input_tick`. `input-contextual-priority` reads its candidate inputs from this module. Its state clears where client prediction resets: beside the app's calls of `reset_level_scoped_client_state` (in `unload_level`) and `demote_client_state`, and when the apply outcome reports `armed_local_pawn`. No hold carries into a new level or onto a new pawn when an entity id recurs (O35).
- **Runner.** The owning client runs `assist::step` with the client runner once per forward-predicted tick, never during reconcile replay, for its own pawn as helper only, over interpolated remote pawns and the facts `remote_assist_facts` returns for them. The client's commands are all real, so rows 2 and 5 never apply there. Reaching the required count latches the client as a completion latches the host, so a client holding on after a revive accumulates nothing on a second downed target until a release and re-press (O28). The client never emits completions. It passes no elapsed, so its step resets when a target's id changes but not when the target re-enters the same state; that reset reaches the bar through the host sample. Retaining elapsed would not help: remote movement payloads arrive once per `SNAPSHOT_TICK_INTERVAL`, so between payloads a target's retained elapsed would not advance and the elapsed rule would reset every predicted tick.
- **Host samples.** The host's sample is one owner-private Number on the state-slot channel and names no target, so a host target switch reaches the client only as a reset. For each accepted snapshot whose local-pawn record carries an acknowledged input tick (`acked_tick`, surfaced by `capture_local_reconcile` from `last_processed_client_tick`), netcode's `lib.rs` receive loop appends the pair (acknowledged tick, sample) to a new list on `ClientApplyFrameOutcome`. The sample is the committed `player.assistProgress` store value after that snapshot's state batch applies, read as `accept_timed_slot_sample` reads the weapon timers. The channel omits a value the client already holds, so a batch with no record for the slot still yields a sample (O30). A snapshot whose state batch was not applied, or that holds `player.assistProgress` for a baseline refresh, yields no sample. `client_assist` consumes the list in order after the receive step.
- **Records.** The client records its progress at each forward-predicted tick on which the step runs with `assist` held and is not following. An idle tick records nothing. Records belong to one hold segment: the step's reset discards every record and ends follow, so a sample acknowledging a tick before the step's last reset finds no record (O14).
- **Correction.** Each sample is compared with the record at its acknowledged tick, never with the current prediction. The tolerance is one increment's progress capped at one half, so a hold whose required count is under two, `holdMs: 0` included, still drops on a host reset or confirmation (O31). Outside follow, the first matching row applies:

  | # | Sample vs the record at the acknowledged tick | Client action |
  |---|---|---|
  | 1 | No sample (state batch not applied, or slot held for a baseline refresh) | Keep the prediction |
  | 2 | No record at that tick (before the step's last reset, an idle tick, or pruned) | Keep the prediction (O13, O14) |
  | 3 | Sample 0, record above the tolerance (a host reset, a confirmed completion included) | Drop: the prediction becomes 0 and the step enters follow; every record is discarded (O6) |
  | 4 | Within the tolerance of the record | Keep the prediction |
  | 5 | Higher or lower than the record by more than the tolerance, sample above 0 | Rebase: the prediction's count becomes the sample times the required count, rounded to the nearest whole tick, plus one for each tick after the later of the acknowledged tick and the step's last reset, capped at the required count; records after the acknowledged tick are rewritten to the rebased values (O32) |

  A record of 1 held at the required count meets a confirmation's 0 in row 3, so a confirmed hold returns the bar to 0 until the next press.
- **Follow.** Follow is a state of the client step, entered only by the correction table's drop row. While following, the step holds its progress at 0, advances nothing and records nothing. With its latch clear it still chooses targets, so a target switch or state change still resets it. A latched step chooses no target, so it leaves follow only through a release or a host sample above 0 (O6, O28). Follow replaces the correction table:

  | # | Event while following | Client action |
  |---|---|---|
  | 1 | No sample, or a sample of 0 | Stay at 0; keep following |
  | 2 | Sample above 0 | Rebase as in the correction table's rebase row, counting from that sample's acknowledged tick; stop following; the latch clears, since a host accumulating progress has no ended hold |
  | 3 | The step's reset | Stop following; the step predicts from 0 under its own table |

**Display**
- The prediction never writes the slot store. Host samples land in the store as they arrive, so state crossings, conditions and store reads see only host samples (I2, O16). The prediction replaces `player.assistProgress` only in the UI display map that `App::build_ui_display_slot_snapshot` builds, the way `LifeStateDisplay` projects the life-state clock; a store write would be pulled back by a snapshot applied on a frame that runs no tick (O15). `build_ui_display_slot_snapshot` gains an `Option<client_assist::UiProjection>` parameter modelled on `life_state_display::UiProjection`, carrying the prediction and step state on a connected client, or the host accumulator's per-helper row-6 flag on a listen host or in single player, and the frame's render alpha. Its callers in `main.rs`, `loading_screen.rs`, `life_state_display.rs` and tests pass it.

  | Role and step state | Displayed `player.assistProgress` |
  |---|---|
  | Connected client, `assist` not held | 0. A release reset the prediction, so host samples from the released hold, still arriving for one round trip, never reach the bar (O36) |
  | Connected client, following | 0, with no alpha term (O6) |
  | Connected client, last forward-predicted tick ran row 6 without reaching the required count | The prediction plus render alpha times one increment's progress, capped at 1, so the bar advances every rendered frame (AC9, O15) |
  | Connected client, any other held state | The prediction |
  | Listen host or single player, the accumulator's last tick ran row 6 for that pawn without completing | The authoritative slot plus render alpha times one increment's progress, capped at 1 |
  | Listen host or single player, otherwise | The authoritative slot |

- **Received progress.** The target client's displayed `player.assistReceivedProgress` never decreases between rendered frames unless a received sample is below its predecessor (AC9). `Ramp` is `pub(super)` in netcode's `state_slots/timed_slot_targets` module and runs on that module's sample timeline, so the projection lives there as its own stream beside the four weapon-fed ones, outside `NAMES`. It is a new type beside `Ramp`, which stays unchanged: `Ramp` ramps only after successive samples agree on a slope, so it would step on the first samples of every assist (O17). `accept_timed_slot_sample` feeds it the committed slot value from each accepted state batch with the snapshot's server tick, under the guards it already applies; `advance_timed_slot_targets` advances it, and `apply_timed_slot_targets`, which `App::build_ui_display_slot_snapshot` already calls, substitutes it in the display map. `reset_timed_slot_targets` clears it with the other streams.
- The stream's slope is known in advance: one increment of the target's own required count per server tick, the rule Task 2a's step counts by. `accept_timed_slot_sample` takes the target's `holdMs` as a new parameter, since it holds no registry, and computes the required count with `TICK_DURATION`, not the module's own `TICKS_PER_SECOND`. Netcode's `lib.rs` receive path, which holds the registry, reads `holdMs` from the local pawn's `PlayerMovementComponent` stamp, restamped from `TuningPayload`. The stream keeps an anchor (the newest sample s and its server tick t) and a displayed value. Its line at timeline time T is s plus the slope times (T − t), with T capped at t plus twice `SNAPSHOT_TICK_INTERVAL` server ticks, and at most 1. The first matching row applies:

  | # | Event | Stream action |
  |---|---|---|
  | 1 | No stamp, or a required count of one | Show each sample raw; nothing divides by zero |
  | 2 | A sample above 0 after a sample of 0, or the stream's first sample | Show it at once (it may step up from 0) and anchor the line at it (O17) |
  | 3 | A sample below the previous sample (a helper released or reset, or a lower helper is now highest) | Show it at once and anchor the line at it (O18) |
  | 4 | A sample at or above the previous sample, whose line is at or above the displayed value | Anchor the line at it; the display keeps rising under the advance row |
  | 5 | A sample at or above the previous sample, whose line is below the displayed value (the host kept a gap-held tick while the stream extrapolated) | Anchor the line at it; hold the displayed value until the line overtakes it, never stepping down |
  | 6 | Timeline advance | When the line is above the displayed value, the displayed value rises toward it by at most twice the slope times the advance, then follows it; when the line is below, the displayed value holds |

- Success presentation (sounds, flashes) belongs to the authored `assistReceived` / `assistGiven` fire lists. Those run on host confirmation and route to each player's machine, so a prediction never plays one (AC9).

**Tests**
- AC9 and rows O6, O13, O14, O15, O16, O17, O18, O28, O30, O31, O32, O35 and O36, driving the client step, the acknowledged-tick samples, each correction and follow row, each received-stream row, and the display map directly. Include a snapshot whose state batch was not applied, and the listen-host and single-player display rows.
- The bar halves of Task 2a's rows: O12 (a gap past the hold grace drops the helper's bar through the correction table's drop row at the acknowledged tick, then it rebases from the first host sample above 0), O19 (a target disconnect drops the helper's bar through the host sample) and O22 (a reload that removes `assistable` from the retained stamp stops the prediction).
- The (acknowledged tick, sample) list on `ClientApplyFrameOutcome` in netcode's `predict_reconcile_relay_harness`.

**Orderings**

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O6 | Helper client predicts completion, but the host's accumulator reset one tick earlier (range edge under interpolation) | Client ahead of host; the host keeps refusing on later ticks | The host's reset wins. At the first snapshot acknowledging an input tick after the reset, the bar drops to 0 and shows exactly 0 on every rendered frame, with no alpha term and no sawtooth between samples, while the host keeps refusing, until the helper releases or a host sample shows progress. No success presentation plays |
| O13 | Hold onset on a lagged client | Client predicts from tick C0. Snapshots acknowledging ticks before C0 arrive for about one round trip, reading 0 | Those ticks precede the hold and have no record, so the bar keeps the prediction and never drops to 0 on those samples |
| O14 | Release then re-press within one round trip | The host reset or confirmed the old hold at Th. Release at client tick C1, re-press at C2. Samples acknowledging ticks from Th through the tick before C2 read 0 and arrive after C2 | The release discarded the old hold's records and the idle ticks recorded nothing, so those samples find no record. The new hold's prediction is untouched: no drop to 0 and no rebase |
| O15 | Zero-tick frame with a snapshot during a held assist | Frame runs no tick; a snapshot applies before the tick loop, then the UI snapshot is built | The displayed helper bar advances by render alpha and never shows the host's older store value |
| O16 | Client-side state crossing on `player.assistProgress` | Prediction reaches 1 before the host completes; the host then resets (O6) | The crossing fires only from host samples, never from the prediction, and no cue plays |
| O17 | Target bar onset | First, second and third received samples of an assist | The first nonzero sample may step up from 0. From it the displayed value rises continuously at the rate the target's own `holdMs` gives, with no step at any later sample's arrival |
| O18 | Highest helper releases while a lower helper continues | Received samples step down from the higher progress to the lower | The bar drops at once to the lower value, then rises at the known rate |
| O28 | Hold carried from a revived target to a second downed target | Completion on A while the helper holds; A becomes `alive`; B, also downed, is in range | Neither host nor client accumulates on B until a release and re-press. Once the confirmation drops the helper's bar to 0, it stays at 0 |
| O30 | Host never accepts a fresh hold after an acknowledged 0 | Release; the host's 0 is acknowledged while the step is idle, and later batches omit the unchanged slot; re-press; the host refuses every tick | Each later snapshot samples the committed 0. The bar drops to 0 at the first snapshot acknowledging a tick of the new hold whose record is above the tolerance |
| O31 | `holdMs: 0` confirmation on the helper's client | First held tick C0; the host completes at C0; the helper keeps holding | The bar shows 1 from C0, then 0 from the sample acknowledging C0 until a release and re-press |
| O32 | Host ahead of the client by the interpolation lead | The host's target enters range k ticks before the client's interpolated copy does | The prediction rebases up to the host value plus later increments; the bar reaches 1 by confirmation rather than dropping from mid-bar |
| O35 | Client hold across a level change | The helper's client holds at 0.8; unload; install with the target carried into `down` under a recurring entity id; the helper still holds | The client bar starts at 0 on the new level and never shows the old progress |
| O36 | Release while host samples are in flight | Release at client tick C1; samples acknowledging held ticks before C1 arrive, still rising | The displayed bar is 0 from C1; the rising samples reach only the store |

Delivers AC9, O6, O13, O14, O15, O16, O17, O18, O28, O30, O31, O32, O35 and O36.

### Task 3: Dev-mod Borderlands upgrade, tests and docs

This task consumes Task 1's `source.inLifeState` and `source.setLifeState`; Task 2a's `assistable` key, `assist` command, `assistReceived()` / `assistGiven()` sources, `player.assistProgress` and `player.assistReceivedProgress` slots and `SimCommand.assist_held`; and Task 2b's bar presentation, which the manual proof checks.

**Content** (`content/dev/scripts/player-death.ts` + `.luau` twin, `content/dev/scripts/player.ts`, `content/dev/scripts/hud.ts`, `content/dev/scripts/input.ts`, and `content/dev/start-script.ts`, which registers the new impact event in `events` and both HUD trees in `uiTrees` beside `bleedOutMeterTree`)
- **Player descriptor:** `down` gains `assistable: { radius: 1.5, holdMs: 3000 }`.
- **Player events** (both twins):
  - `assistReceived` → `setLifeState("alive")` + `grantHealth(30)`
  - `assistGiven` → `addSlot(progression.xp, 25)`
- **Impact event** (both twins): a separate `defineImpactEvent("second-wind", { tag: "enemy" })`, not a row inside `enemyDeath`, which the zombie override would evict. On a kill with `source.inLifeState("down")`: `source.setLifeState("alive")`, `source.grantHealth(40)`, and an XP bump of 50 via `update(progression.xp.byPlayer(impact.source), (xp) => xp.plus(50))`.
- **HUD:** a "Reviving" bar on `player.assistProgress` and a "Being revived" bar on `player.assistReceivedProgress`, built like `bleedOutMeterTree`.
- **Input:** the dev mod's `input.ts` adds an `assist` row with label and category, bound `hold` on `KeyE` and gamepad `west` at the default hold threshold (the `ShiftLeft` sprint row is the pattern). Use keeps its default. Progress starts when `assist` resolves, not at key-down, so a dev-mod revive takes the hold threshold plus `holdMs` (O27).

**Tests**
- Extend `player_death_twins_emit_identical_policy_and_down_vignette_wire` so both twins emit identical `assistReceived` / `assistGiven` player events and an identical `second-wind` impact event.
- Extend `player_death_dev_registration_and_hud_consume_the_authored_policy` to assert `second-wind` and both bars are registered.
- Extend `player_death_surface_tests.rs` beside `player_death_two_player_policy_bleeds_out_on_tick_900_then_respawns_next_tick`: down → assist → alive, down → self-revive.
- Row O3 end to end through `Consumer::tick_then` in `player_death_surface_tests.rs`, dispatching the self-revive kill impact in `between`; drive the helper's hold through the host pawn's `SimCommand.assist_held`, or a `RemotePawnCommand` with `real_command: true`.
- Row O27 with the dev mod's input rows driving the activator.
- **Manual proof**, recorded in the PR: a two-client playtest of the dev mod at 150 ms RTT (`POSTRETRO_NET_LAG`) on `coop-presentation-test`, with `assist` on the dev mod's hold-E. Check AC9 from both the helper's and the target's seat, and AC8's tap-for-Use beside a downed teammate. Record each run's lag settings and what was seen.
  - reviving and being revived
  - releasing `assist` or stepping out of range mid-hold
  - a self-revive kill

**Docs**
- Update `docs/scripting-reference.md` per AC12. Task 1 owns the `SourceHandle` doc comments.

**Orderings**

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O3 | Assist completion recorded in tick, then a post-tick remote-hit flush self-revives the target | Completion, `host_flush_pending_hit_declarations`, player-event pass | The completion is dropped at application because the target left the life state recorded with it. No assist sources fire |
| O27 | Hold threshold plus `holdMs` on the dev mod's hold-E | Key-down at t = 0 | `assist` resolves after the default hold threshold; completion follows `holdMs` after that point, not after key-down |

Delivers AC11, AC12 (`docs/scripting-reference.md`), O3, O27, and the manual proof for AC8 and AC9.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2a. Task 1 owns impact policy; Task 2a owns the accumulator, input, wire, tuning payload, remote retention and slots. Both edit `ir/scopes.rs`, `sdk/lib/data_script.ts` / `.luau` and the `sdk_lib` templates and regenerate SDK types and fixtures; each section carries the merge rule.
**Phase 2 (sequential):** Task 2b. It consumes Task 2a's step, candidate query, retention accessor, row-6 flag, slots and stamp.
**Phase 3 (sequential):** Task 3. Its content and tests consume Task 1 and Task 2a; its manual proof checks Task 2b's bars.

`input-contextual-priority` builds on Task 2a (`Command::Assist`, `assist_held`, `assist_candidate`, `assist::step`) and Task 2b (the client's predicted step in `client_assist`), and replaces the dev mod's interim `assist` binding (Task 3).

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| Assist profile key | `LifeStateProfile` field | `assistable.radius`, `assistable.holdMs`; rides `TuningPayload.life_state_profiles` | same | same |
| Assist stamp | `PlayerMovementComponent` field | `WirePlayerMovementState.assistable` | n/a | n/a |
| Assist command | `Command::Assist`, `Action::Assist` | `"assist"` in bindings and the manifest `input` block | n/a | n/a |
| Dev-mod assist binding | n/a | `input` block `assist`: `hold` on `KeyE` / `west` | `input.ts` | n/a |
| Held-assist input | `WireMovementInput.assist_held`, `SimCommand` | `bool` beside `use_pressed` | n/a | n/a |
| Impact source read | `resolve_owned_input` life-state branch | `{op:"input", owner:"@impact.source", name:"player.inLifeState:<name>"}` | `source.inLifeState(name)` | `source:inLifeState(name)` |
| Impact source effect | planned effect intercepted in `apply_planned` | `{ primitive:"setLifeState", target:"@impact.source", args:{ state } }` | `source.setLifeState(name)` | `source:setLifeState(name)` |
| Helper progress slot | `PlayerSlot::AssistProgress`, read from `PlayerMovementComponent` | `"player.assistProgress"` | `player.assistProgress` | same |
| Received progress slot | `PlayerSlot::AssistReceivedProgress`, read from `PlayerMovementComponent` | `"player.assistReceivedProgress"` | `player.assistReceivedProgress` | same |
| Assist sources | `BoundPlayerEvent` trigger kind `AssistReceived` / `AssistGiven` | `edge: "assistReceived"` / `"assistGiven"`, no `condition` | `assistReceived()` / `assistGiven()` | same |
| Candidate query | `assist::assist_candidate` | n/a | n/a | n/a |

## Wire format

- **`WireMovementInput`** gains `assist_held: bool` beside `use_pressed`.
- **`WirePlayerMovementState`** gains `assistable: Option<WireAssistable { radius: f32, hold_ms: f32 }>` beside `life_state`, `None` when the current state declares none. `all_finite` requires both floats finite when present.
- `WIRE_VERSION` goes to the next value (39 at `b6fc0c4`).
- `SNAPSHOT_VERSION` goes to the next value (25 at `b6fc0c4`), with a history line, because `WirePlayerMovementState` rides the snapshot record.
- `TUNING_PAYLOAD_EPOCH` goes to the next value (17 at `b6fc0c4`), with a history line.
- `TuningPayload.life_state_profiles` serializes `LifeStateProfile` itself, so the new `assistable` key rides it with no payload code. `LifeStateProfile` denies unknown fields, so a reader on the old epoch rejects the payload: `TUNING_PAYLOAD_EPOCH` bumps, and `validate_life_states` refuses a bad `radius` or `holdMs`. The payload carries only the pawn's own profiles; remote pawns' stamps come from the movement state.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1. At most one assist completion per target per tick, applied before that tick's player-event condition snapshot in `players()` order of the target, and only while the target is still in the life state recorded with the completion and, unless the completion was recorded at zero elapsed, has not re-entered it | Task 2a | Remote-hit flush between tick and player-event pass; an earlier completion's fire list in the same pass | AC4, AC6, O1, O3, O23, O34 |
| I2. Assist presentation: the helper's progress is predicted locally, corrected toward the host at the acknowledged input tick, and written only to the UI display map; state crossings, conditions and store reads see only host samples, and no success cue plays without host confirmation | Task 2b (prediction, correction, display); Task 2a (the `ui_proxy` gate) | Any authored cue placed outside the assist fire lists; a predicted write into the slot store; a client `ui_proxy` write of the progress slots | AC9, O6, O15, O16 |
| I3. Spec A's I3 holds with `source.setLifeState` as an additional authored transition source; assist itself never changes life state | Task 1 | Task 2a completions (fire lists only) | AC3, AC4 |
| I4. A press on a key shared by `assist` (`hold`) and `use` (`press`) performs exactly one of them, resolved by the client's activator; the host never turns `assist_held` into Use or Use into an assist | Task 2a | Any host path that reads Use from `assist_held`; `held_gap_sim_command` synthesizing a Use edge | AC8 |

## Script syntax examples

**Borderlands (dev mod, TS).** Spec A's policy plus revive, second wind and bonuses.

```ts
// player descriptor: Spec A's `down` gains
assistable: { radius: 1.5, holdMs: 3000 },

// player-death.ts, beside Spec A's goDown and bleedOut rows
players().on(assistReceived(), [revived]),  // on.player.setLifeState("alive"), on.player.grantHealth(30)
players().on(assistGiven(), [assistBonus]), // on.player.addSlot(progression.xp, 25)

// HUD: "Reviving" bar on player.assistProgress, "Being revived" bar on player.assistReceivedProgress

export const secondWind = defineImpactEvent("second-wind", { tag: "enemy" }, (impact) => {
  const { target, source } = impact;
  const killed = target.healthBefore.gt(0).and(target.healthAfter.le(0));
  return [{
    when: killed.and(source.inLifeState("down")),
    do: [source.setLifeState("alive"), source.grantHealth(40),
         update(progression.xp.byPlayer(source), (xp) => xp.plus(50))],
  }];
});
```

**L4D-style (Luau).** Only a teammate revives; no second wind, no bonus.

```lua
-- down = { ..., assistable = { radius = 2, holdMs = 5000 } }
players():on(assistReceived(), { revived })  -- on.player:setLifeState("alive"), on.player:grantHealth(30)
```
