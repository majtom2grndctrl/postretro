# Player Revive and Assist

## Goal

Let mod authors build revive mechanics (Borderlands second wind, L4D teammate revive) with author-chosen bonuses. The engine adds a teammate **assist** interaction, driven by its own `assist` input command, and life-state reads and effects on an impact's source. The dev mod upgrades its down/bleed-out policy to full Borderlands. Builds on `E18--player-life-states` (Spec A, landed in `done/`): its life states, capability gates, verbs, name table and slots are the base this spec extends. `assist` has no engine default. The dev mod binds it as a `hold` on Use's inputs, a shared key legal under today's rule; `input-contextual-priority` later adds an engine default with availability-ranked sharing and consumes this spec's candidate query.

## Scope

### In scope

- The `assistable: { radius, holdMs }` profile key on a Spec A life state, stamped per pawn and replicated on the pawn's movement state.
- A gameplay input command `assist`, with no engine default, and its held bit on the wire.
- A host assist accumulator, with an owning-client prediction of the helper's own progress. Target selection is a standalone pure query.
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
- **"Engine features per mechanic (a revive system, a lives system) put policy in the engine"** (Spec A, Alternatives rejected). Diverges, narrowly. The accumulator carries a small fixed rule set: nearest target with ties to `players()` order, reset on release, target switch or state change, one completion per target per tick, and a completion ending the hold. The divergence is needed because range and hold time over per-tick pawn positions and held input are invisible to the IR, so no authored policy can compute them. Everything a revive *does* stays authored.
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
- **A respawn hook in `sim::respawn::drain`.** `drain` takes only the `ScriptCtx` and registry and cannot reach the session-owned accumulator state. The accumulator's elapsed check already sees every respawn, since respawn enters its state at elapsed zero.

**Foreclosures and one-way doors.**

- `assist_held` on `WireMovementInput` and `assistable` on `WirePlayerMovementState` take the next `WIRE_VERSION` (39 at `b6fc0c4`), and `assistable` on `LifeStateProfile` takes the next `TUNING_PAYLOAD_EPOCH`. Pre-stable, each undone with another bump.
- SDK names (`assistable`, `assistReceived`, `assistGiven`, `player.assistProgress`, `player.assistReceivedProgress`, `source.inLifeState`, `source.setLifeState`) and the command ID `assist` become the modder contract; the command ID is also written into player bindings and manifests. Renaming any of them costs a pre-stable SDK break across both runtimes.
- `assist` has no engine default until `input-contextual-priority` adds one. The dev mod's hold-E binding is content, replaced when that spec lands; a player who rebound `assist` keeps their binding.
- Task 2 widens the per-player slot bitset (`EntityScope.warned_owned_engine`, capped at sixteen by a compile-time assert) to u32. The slot count is fifteen (`PlayerSlot::ALL` at `b6fc0c4`); the two progress slots take it to seventeen, so the widening is required for this spec to compile.
- An assist with no line-of-sight or facing requirement can be tightened later by adding optional profile keys. `holdMs` belongs to the target's state, so a helper-side speed modifier (a revive perk) needs a later helper-profile key; it is additive. Nothing material is foreclosed.

## Acceptance criteria

### Descriptor and validation
- [ ] **AC1.** A life-state profile accepts `assistable: { radius, holdMs }` in both runtimes into the same lowered shape. Loading is rejected, with the full key path, for an `assistable.radius` ≤ 0 or non-finite, an `assistable.holdMs` < 0 or non-finite, and an unknown key inside `assistable`. A connected client refuses a tuning payload whose profile carries the same faults. On every role, a pawn's movement component carries its current state's `assistable` (or none), restamped at every life-state write, and a connected client reads the same value for a remote pawn.
- [ ] **AC2.** An impact policy whose `source.inLifeState` or `source.setLifeState` names a state absent from the mod's name table is skipped at bind with a warning naming the policy and the state; the mod's other policies still bind. At runtime, `source.setLifeState` on a source whose descriptor lacks that state, or declares no states, warn-skips.

### Impact source read and effect
- [ ] **AC3.** In an impact policy, `source.inLifeState(name)` holds exactly while the source pawn is in `name`. It is false for a non-player source, which reads no warning. `source.setLifeState(name)` applies to the source pawn in-tick with Spec A's `setLifeState` semantics: the next tick's gates read the new profile, elapsed time restarts, and the current state is a no-op. A live source that is not a player pawn warn-skips through `set_life_state`'s missing-state warning; an impact with no live source skips silently, as every source effect does.

### Assist
- [ ] **AC4.** Holding `assist` within `radius` of a pawn whose current state declares `assistable` completes after `holdMs`. The completion fires every `assistReceived()` subscription with `on.player` = target, then every `assistGiven()` subscription with `on.player` = helper. Both fire in the player-event pass of the tick the hold completes, before that pass's condition snapshot. Completion itself changes no life state or health; only the authored fire lists do.
- [ ] **AC5.** Paired refuse/permit edges for assist:
  - **Refused:** out of range; target in a non-assistable state; helper's state has `use: false`; release one tick before `holdMs`.
  - **Permitted:** in range on an assistable state; `holdMs: 0` completes on the first held tick.

  Progress resets when the helper releases or switches targets, when either pawn changes or re-enters a life state (respawn included), and when the hold completes. After a completion the helper accumulates again only after releasing and holding `assist` anew. `player.assistProgress` reports the pawn's progress as helper, 0..1. `player.assistReceivedProgress` reports the progress of the assist the pawn is receiving, 0..1: the highest among its helpers, and 0 with none. A pawn helping while being helped reports both.
- [ ] **AC6.** Two helpers completing on one target in one tick produce one completion, for the earlier helper in `players()` order. The other helper's progress resets. Held `assist` survives a short input gap under the host's hold-then-neutral gap policy: gap-held ticks keep progress without advancing it, and only ticks driven by a real command advance it.

### Input command
- [ ] **AC7.** Absent a manifest `show` override, `assist` is listed and bindable exactly when some loaded descriptor declares `assistable` or, while participating in co-op, a life-state profile in the installed host tuning does; otherwise it is unbound, unlisted and never part of a conflict. It has no engine default. Bound to a key of its own, it drives every assist AC above.
- [ ] **AC8.** With `assist` bound as a `hold` on a key that also carries Use's `press` (the dev mod's binding on `KeyE` and gamepad `west`), holding the key past the hold threshold drives assist and fires no Use-activated trigger or switch, with or without a teammate in range. Releasing before the threshold fires Use once. The pair raises no binding conflict. Engine default bindings are unchanged: with the engine defaults alone, Use fires on press exactly as today.

### Client experience
- [ ] **AC9.** The helper client's "Reviving" bar shows its own `player.assistProgress` on the first held tick, with no round-trip wait, and advances every rendered frame. If the host resets or never confirms (target moved, revived elsewhere, out of range on the host), the bar returns to 0 with no success cue. Success consequences (state, health, bonuses, their presentation) appear only after host confirmation. The target client's `player.assistReceivedProgress` bar has no stair-steps at snapshot arrival.

### Orderings and consumer
- [ ] **AC10.** Every row of the Orderings table has a test asserting its expected outcome.
- [ ] **AC11.** The dev mod upgrades Spec A's down/bleed-out policy in TS and Luau twins, with the parity surface test extended:
  - `down` declares `assistable: { radius: 1.5, holdMs: 3000 }`
  - teammate hold-`assist` → `alive` with 30 health, and an XP bonus to the helper
  - an enemy kill while down → `alive` with a health and XP bonus
  - a "Reviving" HUD bar on `player.assistProgress` and a "Being revived" HUD bar on `player.assistReceivedProgress`

  The two-player test adds down → assist → alive and down → self-revive beside down → bleed-out → respawn.
- [ ] **AC12.** The docs need these changes:
  - `docs/scripting-reference.md` documents `assistable`, the `assist` command, its default and when it is relevant, the assist sources, `player.assistProgress`, `player.assistReceivedProgress`, `source.inLifeState` / `source.setLifeState`, and the full Borderlands and L4D-style recipes.
  - It drops its stale claims that per-owner + persist is rejected and that there is "no source-scoped fact vocabulary" (also in the `SourceHandle` doc comments in `sdk/lib/data_script.ts`, its `.luau` twin and the `sdk_lib` typedef templates, then regenerated types).
  - Every command shown runs from an SDK bundle.

## Orderings

Task agents cite rows by number. Expected outcomes are binding.

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Assist completes the same tick bleed-out would expire | Assist in tick, completions applied before the player-event snapshot; the tick may be the first of several in one frame | Revive applies first, in that tick's pass rather than the frame's last tick; the bleed-out condition reads `inLifeState("down")` false and does not fire |
| O2 | Self-revive kill from a remote hit still waiting on fire authorization, landing after bleed-out expired | Bleed-out fires in tick N's player-event pass, respawn drains at tick N+1 start, hit flushed by tick N+k's post-tick flush (k ≥ 1, in the same frame or a later one) | Respawn wins. The later `source.inLifeState("down")` is false and the self-revive row does not fire |
| O3 | Assist completion recorded in tick, then a post-tick remote-hit flush self-revives the target | Completion, `host_flush_pending_hit_declarations`, player-event pass | The completion is dropped at application because the target left the life state recorded with it. No assist sources fire |
| O4 | `holdMs: 0` | First held tick, helper keeps holding | Completes that tick, once. Holding on completes nothing more until `assist` is released and held anew, even when the fire list leaves the target assistable |
| O5 | Helper disconnects mid-hold | Seat harvest between frames | Progress dropped. The target is unchanged |
| O6 | Helper client predicts completion, but the host's accumulator reset one tick earlier (range edge under interpolation) | Client ahead of host; the host keeps refusing on later ticks | The host's reset wins. At the first snapshot acknowledging an input tick after the reset, the bar drops to 0 and stays there while the host keeps refusing, with no sawtooth between samples, until the helper releases or a host sample shows progress. No success presentation plays |
| O7 | Self-revive remote hit flushed in the same tick bleed-out expires | Tick N: sim, then `host_flush_pending_hit_declarations` runs `source.setLifeState("alive")`, then the player-event pass whose bleed-out condition would turn true at N | Self-revive wins. The bleed-out row reads `inLifeState("down")` false and does not fire, and no respawn is queued |
| O8 | Self-revive by an in-tick hit in the tick a helper would complete | Tick N: Projectiles or Weapons stage self-revive, then the accumulator after `run_death_sweep` | The target is `alive` at the accumulator pass. No completion is recorded, every helper's progress on it resets, and no assist source fires |
| O9 | Target leaves and re-enters its assistable state between accumulator passes | Tick N accumulator (helper near `holdMs`), then the flush self-revives the target, a lethal late hit and the death sweep follow, then the pass's down row re-enters `down` | Tick N+1: the target's elapsed has not advanced, so every helper's progress on it restarts from 0. No completion comes before a further full `holdMs` |
| O10 | Completion whose fire list leaves the target assistable | Tick N completion, heal-only fire list, helper keeps holding | One completion at N. None while the helper keeps holding; after a release and re-press, the next comes after another full `holdMs` |
| O11 | Late release command across an input gap | Helper releases one tick before `holdMs`. The host frontier-freezes on the previous held command, then consumes the release | No completion. Gap-held ticks keep progress without advancing it, so host and client tick counts agree |
| O12 | Gap longer than the hold grace | `INPUT_HOLD_TICKS` held ticks, then neutral, then the real stream resumes held | The neutral tick reads as a release and resets progress. The resumed hold starts from 0. The helper's bar drops through the reset row of the Client presentation table at the acknowledged tick |
| O13 | Hold onset on a lagged client | Client predicts from tick C0. Snapshots acknowledging ticks before C0 arrive for about one round trip, reading 0 | No prediction is recorded at those ticks, so the bar keeps the prediction and never drops to 0 on those samples |
| O14 | Release then re-press within one round trip | Release at client tick C1, re-press at C2. Samples acknowledging C1 and the idle ticks after it read 0 and arrive after C2 | The step recorded nothing at those ticks, so the new hold's prediction is untouched |
| O15 | Zero-tick frame with a snapshot during a held assist | Frame runs no tick; a snapshot applies before the tick loop, then the UI snapshot is built | The displayed helper bar advances by render alpha and never shows the host's older store value |
| O16 | Client-side state crossing on `player.assistProgress` | Prediction reaches 1 before the host completes; the host then resets (O6) | The crossing fires only from host samples, never from the prediction, and no cue plays |
| O17 | Target bar onset | First, second and third received samples of an assist | The displayed value rises continuously from the first nonzero sample at the rate the target's own `holdMs` gives, with no step at any sample's arrival |
| O18 | Highest helper releases while a lower helper continues | Received samples step down from the higher progress to the lower | The bar drops at once to the lower value, then rises at the known rate |
| O19 | Target disconnects mid-hold | `net_poll_and_apply` harvests and despawns the target before tick N | The helper has no target at tick N; its progress resets and nothing is recorded. The helper's bar drops through the host sample |
| O20 | Target rejoins mid-hold, restored into `down` | Despawn, then rejoin spawns a new pawn carried into `down` while the helper holds | Progress on the new pawn starts from 0, as a target switch |
| O21 | Level change mid-hold | Unload, then install with the target carried into `down` and the helper holding | First tick of the new level: progress 0, and no completion before a full `holdMs` |
| O22 | Hot reload removes `assistable` from the target's state mid-hold | Reload restamps between frames, then the next tick runs | The next accumulator pass finds no target and resets progress. The client's retained stamp updates from the next movement payload and its prediction stops |
| O23 | Completions on different targets in one tick | The accumulator records C1 and C2, then the pass applies them before `evaluate()` | Applied in `players()` order of the target. C1's `assistReceived` and then `assistGiven` subscriptions fire before C2 is re-checked |
| O24 | No completions in a tick | Empty `assist_completions` | `run_tick` behaves exactly as before this spec, and existing player-event tests pass unchanged |
| O25 | Helper respawns within radius of its target mid-hold | Tick-start drain relocates the helper, still within `radius` and holding | The helper's elapsed has not advanced, so its progress restarts from 0 |
| O26 | Focus loss mid-hold | `cancel_all` lifts `assist` without a pulse | Progress resets as on a release, and Use does not fire |
| O27 | Hold threshold plus `holdMs` on the dev mod's hold-E | Key-down at t = 0 | `assist` resolves after the default hold threshold; completion follows `holdMs` after that point, not after key-down |

## Tasks

New logic lands in new modules. Files already past ~800 lines (`sim/src/sim/mod.rs`, `sim/src/impact_policy.rs`, `netcode/src/lib.rs`, `netcode/src/client.rs`, `netcode/src/command_queue.rs`, `ir/scopes.rs`, `netcode/src/state_slots.rs`, `startup/reaction_validation.rs`, `main.rs`, `frame_loop/mod.rs`) receive only call-site edits and match arms. Symbol grounding: `research.md` in this folder.

### Task 1: Impact-policy source reads and effects

- **`source.inLifeState(name)`.** `EntityScope::resolve_owned_input` gains a life-state branch for `owner:"@impact.source"` names with `LIFE_STATE_INPUT_PREFIX`, resolving the id through `resolve_life_state_id`. `seed_owner_seat_from_registry` also captures the source pawn's `PlayerMovementComponent.life_state`, which the owner snapshot does not hold today. The read is false, without a warning, for a non-player source.
- **`source.setLifeState(name)`.** `bind_effect` accepts it for `IMPACT_SOURCE_TOKEN` only, reading the name from `args.state`. `resolve_life_state_id` is `pub(crate)` in `scripting-core`, so `EntityScope` gains a public method reporting whether the mod name table holds a name, and `bind_effect` returns an error naming the state when it does not. `apply_planned` intercepts it the way it intercepts `SetOwnerSlot` (resolving the recipient through `resolve_command_recipient`) and calls `life_state_commands::set_life_state` with its `ctx` and registry, so elapsed reset, the same-state no-op and the weapon-activation cancel hold. `apply_effect` marks it unreachable like the other intercepted effects.
- **SDK.** Add `inLifeState` and `setLifeState` to `SourceHandle` in `sdk/lib/data_script.ts` / `.luau` and the sdk_lib templates, and a `setLifeState` arm to `ImpactEffectWire`, then regenerate types and fixtures.
- **Unknown names.** An unknown state fails `bind_policy`, which skips the policy with its existing warning (AC2). The warning names the policy and the state.
- **Test** AC2, AC3, O2 and O7 with the `impact_policy` test helpers and Spec A's bleed-out `respawn`.
- Delivers AC2, AC3, O2 and O7.

### Task 2: Assist

**Profile key and stamp**
- Add `assistable: { radius, holdMs }` to `LifeStateProfile` and Spec A's shared validator (AC1).
- `TuningPayload.life_state_profiles` serializes `LifeStateProfile` itself (`tuning_payload_for_pawn`), so `assistable` rides it with no payload code. `LifeStateProfile` denies unknown fields, so a reader on the old epoch rejects the payload: bump `TUNING_PAYLOAD_EPOCH` (`combat-model/src/carried_loadout.rs`) with a history line, and extend `validate_life_states` (`netcode/src/tuning_payload.rs`) to refuse a non-finite or non-positive `radius` and a non-finite or negative `holdMs` (AC1). The client restamps its own pawn's `assistable` from these profiles like the other profile fields.
- Stamp the current state's `assistable` onto `PlayerMovementComponent` wherever Spec A stamps the other profile fields, and carry it on `WirePlayerMovementState` beside `life_state` (`movement_state_to_wire` and the merge). Bump `WIRE_VERSION`.
- On a connected client, keep `life_state` and `assistable` for remote pawns from the movement payload `client.rs` drops today as `MovementWithoutLocalComponent`. Retain them in a map keyed by `NetworkId` on `ClientReplication`, beside `remote_player_locomotion`. Each accepted payload overwrites its entry. Every site that removes or clears `remote_player_locomotion` (`clear_identity_state`, local-pawn arming, both full clears) does the same for this map. No `PlayerMovementComponent` is constructed and no gate runs on them. The payload still reports `MovementWithoutLocalComponent`, since no component merge happens. Retained ids are compared only for equality, never resolved through a name table.

**Input command**
- Add `Command::Assist` and `Action::Assist`: `Command::ALL`, `id`, `context`, `kind`, an `accepts` arm for `Press | Hold` (the accumulator reads a level), `gameplay_target`, `legacy_actions`, and the `CommandId` list in `sim/src/scripting/primitives/manifest.rs` (then regenerate types).
- Add a relevance fact and its `derived_relevance` arm: relevant exactly when some loaded descriptor declares `assistable` (AC7). `registry_facts` derives it from local descriptors, and `tuning_facts` from the installed host tuning's `life_state_profiles`, which carry `assistable`, per `input.md`'s co-op relevance rule.
- Add `assist_held: bool` to `WireMovementInput` and to `SimCommand` beside its top-level `use_pressed` (not `MovementInput`; both runners read it from `SimCommand`), threaded through `build_sim_command`, `sim_command_to_input` / `input_command_to_sim` and `neutral_sim_command` (false). `held_gap_sim_command` carries it through a gap like other levels, not clearing it as it clears edges. The accumulator advances progress only on a tick whose command is real (`RemotePawnCommand::real_command`). A gap-held tick keeps progress without advancing it, so a release whose command arrives late never completes on held ticks, and a frontier freeze that consumes one client tick twice counts it once (O11). A neutral command after the hold grace reads as a release (O12). A remote pawn with no resolved command holds nothing.
- `engine_default_inputs(Command::Assist, class)` returns no inputs for either device class. Test AC8 with an author binding of `assist` as `hold` on `KeyE` beside the default Use.

**Accumulator**
- A host and single-player accumulator in new module `sim/src/sim/assist.rs`. It runs in the fixed tick after the Weapons stage and `run_death_sweep`. Its cross-tick state is session-owned, beside `touch_system`, and passed to the sim tick as `&mut`. The state is level-scoped: actual level unload clears it beside the respawn queue (`clear_for_level_unload`), so no progress carries into the next level when an entity id recurs or a target arrives carried into its assistable state (O21). Suspend keeps it, since the suspended world stays live.
- Each helper whose state allows `use` (`player_life::permits` with `Capability::Use`) and who holds `assist` accumulates on the nearest pawn within its target's `assistable.radius`. Distance is between pawn origins; ties go to the earlier pawn in `players()` order.
- Target selection is its own pure function (`assist_candidate`): helper position and capability, candidate positions, life-state ids and `assistable` stamps in, chosen target out. `input-contextual-priority` calls it for availability.
- Progress resets per AC5. The state stores, per pawn, its life-state id and `life_state_elapsed_ticks` at the last pass. Elapsed advances once per host tick for every pawn, and entering a state, respawn included, sets it to zero. When a pawn's id differs from the stored one, or its elapsed is not greater than the stored value, the accumulator resets that pawn's progress in both roles: its own progress as helper, and every helper's progress on it as target. This covers a respawn into the same state (O25) and a leave-and-re-enter between passes (O9), with no hook in `sim::respawn::drain`. A pawn removed from the registry (seat harvest, O5, O19) drops its progress in both roles. A pawn that appears as a new entity starts from 0 (O20).
- Completions go into a `TickEvents` `assist_completions` list (precedent `trigger_exit_fires`), each carrying the target's life-state id at record. One completion per target per tick (AC6): the earlier helper in `players()` order wins and the others reset. A completion ends the winning helper's hold: its progress resets to 0 and accumulates again only after `assist` is released and held anew (O4, O10).
- **One rule, two runners.** The accumulator is a pure step over (helper input, helper capability, candidates, previous progress). The host runs it authoritatively. The owning client runs the same step once per forward-predicted tick, never during reconcile replay, for its own pawn as helper only, over interpolated remote pawns and their retained life-state ids and `assistable` stamps. On reaching the target's `holdMs` the client's step holds at 1 instead of completing, and it never emits completions.

**Client presentation**
- The client records its predicted progress at each input tick on which its step holds a target; an idle tick records nothing. The host's sample is one owner-private Number on the state-slot channel and names no target, so a host target switch reaches the client only as a reset. Each accepted snapshot that carries the local pawn's movement record names the input tick the host has processed (`last_processed_client_tick`) and carries the host's progress in its state batch. The client compares that sample with the prediction recorded at that acknowledged tick, never with the current prediction. One tick's increment is one tick over the target's `holdMs`; with `holdMs: 0` it is 1.

  | Host sample vs the prediction recorded at the acknowledged tick | Client action |
  |---|---|
  | No sample yet, or no prediction recorded at that tick (step idle, or record pruned) | Keep the prediction (O13, O14) |
  | Equal within one tick's increment, or higher | Keep the prediction |
  | Lower by more than one tick's increment, and above 0 | Rebase: the prediction becomes the host value plus one increment for each later tick on which the step accumulated, capped at 1 |
  | 0 while the recorded prediction exceeds one tick's increment (a host reset, a confirmed completion included) | The prediction drops to 0 and follows host samples until the client's step resets (release, target switch, state change) or a later host sample shows progress, then rebases from that sample (O6, O12) |

- The prediction holds at 1 from the tick it reaches `holdMs` until a host sample confirms or resets it. A confirmation is a completion, which resets the host's progress, so both arrive through the table's last row and the bar returns to 0 until the next press.
- The prediction never writes the slot store. Host samples land in the store as they arrive, so state crossings, conditions and store reads see only host samples (I2, O16). The prediction replaces `player.assistProgress` only in the UI display map that `App::build_ui_display_slot_snapshot` builds, the way `LifeStateDisplay` projects the life-state clock; a store write would be pulled back by a snapshot applied on a frame that runs no tick (O15). While the step accumulates, the displayed value adds render alpha times one tick's increment, capped at 1, so the bar advances every rendered frame (AC9). The listen host's own bar adds the same alpha term over its authoritative slot.
- The target client's `player.assistReceivedProgress` rises without stair-steps from its first nonzero sample. `Ramp` is `pub(super)` in netcode's `state_slots/timed_slot_targets` module and runs on that module's sample timeline, so the projection lives there as its own stream beside the four weapon-fed ones, outside `NAMES`. `accept_timed_slot_sample` feeds it the committed slot value from each accepted state batch with the snapshot's server tick, `advance_timed_slot_targets` advances it, and `apply_timed_slot_targets`, which `App::build_ui_display_slot_snapshot` already calls, substitutes it in the display map. `reset_timed_slot_targets` clears it with the other streams.
- The stream's rate is known in advance: each held tick adds one tick over the target's own `assistable.holdMs`, which the target's client holds on its own pawn, restamped from `TuningPayload`. The stream uses that rate as its slope from the first nonzero sample, since `Ramp` alone ramps only after successive samples agree on a slope and would step on the first samples of every assist (O17). It stays at or below 1 and holds at the newest sample once two snapshot intervals pass with no new sample. A sample below the previous one shows at once: a helper released or reset, or a lower helper is now highest (O18).
- Success presentation (sounds, flashes) belongs to the authored `assistReceived` / `assistGiven` fire lists. Those run on host confirmation and route to each player's machine, so a prediction never plays one (AC9).

**Sources**
- `PlayerEventTable::run_tick` takes the tick's `assist_completions` and applies them before `evaluate()`, in `players()` order of the target. Each completion is re-checked immediately before its own application, not all before the first, and dropped unless the target still holds the life-state id recorded with it and that state declares `assistable` (I1, O23). A move to another assistable state between record and apply drops it, matching AC5's reset on a state change. Test the re-check with a direct state change between record and apply; Task 3 runs O3 end to end. With no completions, `run_tick` behaves as before (O24).
- `BoundPlayerEvent` gains a trigger kind: `Edge { condition, edge, program, memory, … }` or `AssistReceived` or `AssistGiven`. Discrete kinds carry no condition, program or memory. Its existing `source` field is the entry's provenance (`PlayerEventSource`) and keeps that name and meaning.
- `PlayerEventDescriptor` (`entities/src/data_descriptors/types/player_events.rs`) carries the same trigger kind in place of its required `edge` and `condition`. `player_event_from_json` accepts `edge: "assistReceived" | "assistGiven"` with no condition; today it accepts only `becomes` or `ceases`, with a condition. Every descriptor consumer follows the kind: player-event install, staged-manifest transfer, the manifest typedef and `data_registry.rs`.
- SDK `assistReceived()` / `assistGiven()` in both runtimes and the type declarations, then regenerate.
- Fire-list validation is the player-event rule set unchanged. Each fire binds `on.player` to the target or the helper respectively.

**Progress slots**
- Add `player.assistProgress` and `player.assistReceivedProgress` as `PlayerSlot`s (seventeen slots), at the sites `PlayerSlot::LifeStateMs` touched: `player_slots.rs` arms, an `engine_state_catalog.rs` entry with `OwnerPrivatePlayer`, the `ui_proxy.rs` publisher, `MIRRORED_GAME_STATE_LEAVES`, regenerated types and `expected.d.*` fixtures.
- Both values live on the pawn. `player_slot_value` (`player_slots.rs`) reads every non-weapon slot straight from the pawn's components, and owner-private replication (`owner_private_source_value`), the host HUD publisher, player-event reads and `byPlayer(source)` reads all go through it; no slot-table write reaches them. Add `assist_progress` and `assist_received_progress` to `PlayerMovementComponent`. The accumulator writes both every tick per AC5, and the two new `player_slot_value` arms read them the way the `LifeStateMs` arm reads `life_state_elapsed_ticks` (`reads_active_weapon` false). A pawn with no assist activity reads 0 for both. The fields stay off `WirePlayerMovementState`; clients receive the values as owner-private samples.
- Widen the `warned_owned_engine` bitset in `ir/scopes.rs` from u16 to u32 and update its compile-time assert.

**Tests**
- Accumulator: AC4, AC5, AC6 and rows O4, O5, O8, O9, O10, O11, O12, O19, O20, O21, O22 and O25.
- Player-event pass: rows O1 (including a frame that runs several ticks), O23 and O24.
- Input: AC7, AC8 and row O26.
- Client presentation: AC9 and rows O6, O13, O14, O15, O16, O17 and O18, driving the client step, snapshot acknowledgement and the display map directly.

Delivers AC1, AC4, AC5, AC6, AC7, AC8, AC9, O1, O4, O5, O6, O8, O9, O10, O11, O12, O13, O14, O15, O16, O17, O18, O19, O20, O21, O22, O23, O24, O25 and O26.

### Task 3: Dev-mod Borderlands upgrade, tests and docs

**Content** (`content/dev/scripts/player-death.ts` + `.luau` twin, `content/dev/scripts/player.ts`, `content/dev/scripts/hud.ts`, `content/dev/scripts/input.ts`)
- **Player descriptor:** `down` gains `assistable: { radius: 1.5, holdMs: 3000 }`.
- **Player events:**
  - `assistReceived` → `setLifeState("alive")` + `grantHealth(30)`
  - `assistGiven` → XP via `addSlot` on `progression.xp`
- **Impact event:** a separate `defineImpactEvent("second-wind", { tag: "enemy" })`, not a row inside `enemyDeath`, which the zombie override would evict. On a kill with `source.inLifeState("down")`: `source.setLifeState("alive")`, `source.grantHealth(40)`, and an XP bump via `update(progression.xp.byPlayer(impact.source), …)`.
- **HUD:** a "Reviving" bar on `player.assistProgress` and a "Being revived" bar on `player.assistReceivedProgress`, built like `bleedOutMeterTree`.
- **Input:** the dev mod's `input.ts` adds an `assist` row with label and category, bound `hold` on `KeyE` and gamepad `west` at the default hold threshold (the `ShiftLeft` sprint row is the pattern). Use keeps its default. Progress starts when `assist` resolves, not at key-down, so a dev-mod revive takes the hold threshold plus `holdMs` (O27).

**Tests**
- Extend the parity surface test.
- Extend `player_death_surface_tests.rs` beside `player_death_two_player_policy_bleeds_out_on_tick_900_then_respawns_next_tick`: down → assist → alive, down → self-revive.
- Orderings rows O3 end to end, and O27 with the dev mod's input rows driving the activator.
- **Manual proof**, recorded in the PR: a two-client playtest of the dev mod at 150 ms RTT (`POSTRETRO_NET_LAG`) on `coop-presentation-test`, with `assist` on the dev mod's hold-E. Check AC9 from both the helper's and the target's seat, and AC8's tap-for-Use beside a downed teammate. Record each run's lag settings and what was seen.
  - reviving and being revived
  - releasing `assist` or stepping out of range mid-hold
  - a self-revive kill

**Docs**
- Update the docs per AC12.

Delivers AC10 (residual rows), AC11, AC12, O3, O27, and the manual proof for AC8 and AC9.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2. Task 1 owns impact policy, Task 2 owns the accumulator, input, wire, tuning payload and slots. Both edit `ir/scopes.rs` (Task 1 the life-state branch and name check, Task 2 the bitset width) and regenerate SDK types; merge-coordinate.
**Phase 2 (sequential):** Task 3. It consumes both surfaces.

`input-contextual-priority` builds on Task 2 (`Command::Assist`, `assist_held`, `assist_candidate`, the client's predicted step) and replaces the dev mod's interim `assist` binding (Task 3).

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
- **`WirePlayerMovementState`** gains `assistable: Option<{ radius: f32, hold_ms: f32 }>` beside `life_state`, `None` when the current state declares none.
- `WIRE_VERSION` goes to the next value (39 at `b6fc0c4`).
- `TuningPayload.life_state_profiles` serializes `LifeStateProfile` itself, so the new `assistable` key rides it with no payload code. `LifeStateProfile` denies unknown fields, so a reader on the old epoch rejects the payload: `TUNING_PAYLOAD_EPOCH` bumps, and `validate_life_states` refuses a bad `radius` or `holdMs`. The payload carries only the pawn's own profiles; remote pawns' stamps come from the movement state.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1. At most one assist completion per target per tick, applied before that tick's player-event condition snapshot in `players()` order of the target, and only while the target is still in the life state recorded with the completion | Task 2 | Remote-hit flush between tick and player-event pass; an earlier completion's fire list in the same pass | AC4, AC6, O1, O3, O23 |
| I2. Assist presentation: the helper's progress is predicted locally, corrected toward the host at the acknowledged input tick, and written only to the UI display map; state crossings, conditions and store reads see only host samples, and no success cue plays without host confirmation | Task 2 | Any authored cue placed outside the assist fire lists; a predicted write into the slot store | AC9, O6, O15, O16 |
| I3. Spec A's I3 holds with `source.setLifeState` as an additional authored transition source; assist itself never changes life state | Task 1 | Task 2 completions (fire lists only) | AC3, AC4 |
| I4. A press on a key shared by `assist` (`hold`) and `use` (`press`) performs exactly one of them, resolved by the client's activator; the host never turns `assist_held` into Use or Use into an assist | Task 2 | Any host path that reads Use from `assist_held`; `held_gap_sim_command` synthesizing a Use edge | AC8 |

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
