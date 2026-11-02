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
- **Host vs client.** Completion is host-authoritative. The helper's own progress bar is predicted on its client so host authority never shows up as latency. The prediction reads interpolated remote pawns, the remote pawns' life-state id and `assistable` stamp (replicated on their movement state, below), and the helper's own `use` capability. It is corrected toward the host. Success presentation plays only on host confirmation.
- **What the client knows about a teammate.** The host stamps each pawn's current `assistable` (or none) onto its movement component and replicates it on the pawn's movement state, beside the life-state id that already rides there. A connected client keeps both fields for remote pawns, which it drops today. The value is per pawn, so mixed player descriptors work and the client needs no other pawn's descriptor or profile table.
- **Input resolution is client-side.** On a key shared by `assist` (`hold`) and `use` (`press`), as the dev mod binds it, the client's activator resolution decides: holding past the hold threshold drives `assist`; releasing before it fires Use. The client sends only the resolved command. The host never turns `assist_held` into Use or Use into an assist. If host and client disagree about range, the assist does not progress on the host and no trigger fires.
- **Impact policy vs player events.** Second wind is a consequence of a kill, so it is authored where kills are: an impact policy that reads and sets the *source's* life state.

**Prior commitments.**

- **Life states, capability gates, `setLifeState`, `respawn`, `inLifeState` and the name table** (Spec A). Kept. `assistable` is a closed profile key added to Spec A's set. Unknown names in the new read and effect fail at bind the way Spec A's `inLifeState` does: `resolve_life_state_id` refuses the name, and the policy is skipped with a warning. Spec A's I3 (the engine never changes life state on HP) gains `source.setLifeState` as an authored transition source.
- **The client restamps only its own pawn's profile, from `TuningPayload`; movement payloads for pawns without a local `PlayerMovementComponent` are ignored** (`IgnoredPayload::MovementWithoutLocalComponent`, Spec A). Diverges, narrowly. The client keeps remote pawns' `life_state` and `assistable` fields from the payload it already receives. Gates still never run on remote pawns. The divergence is needed because the helper's prediction reads its target's state. `TuningPayload` is unchanged.
- **Player events are host-only, and `on.player` is legal only before `wait`** (`scripting.md` §12). Kept. `assistReceived` and `assistGiven` are discrete sources that bind the existing `on.player` at completion; no `wait` is involved.
- **Command relevance is an exhaustive match over descriptors** (`input.md` §Relevance, `derived_relevance`). Kept. `assist` gains an arm: relevant exactly when some loaded descriptor declares an `assistable` profile key.
- **Shared keys (Steam rule): one short binding plus one `hold` per key in a context** (`input.md` §Shared keys). Kept. The dev mod's `assist` (`hold`) beside `use` (`press`) on `KeyE` / `west` is a legal pair today, the same pattern as its hold-Shift sprint beside tap-Shift dash. In that mod, Use on those keys fires on release before the hold threshold rather than on press.
- **Engine default bindings are all `press`** (`default_bindings_for`). Kept. `assist` gets no engine default here.
- **"Engine features per mechanic (a revive system, a lives system) put policy in the engine"** (Spec A, Alternatives rejected). Diverges, narrowly. The accumulator carries a small fixed rule set: nearest target with ties to `players()` order, reset on release, target switch or state change, and one completion per target per tick. The divergence is needed because range and hold time over per-tick pawn positions and held input are invisible to the IR, so no authored policy can compute them. Everything a revive *does* stays authored.
- **"Death is modder policy derived from impact"** (Q3 archive, E16). Kept. Self-revive is an authored impact effect, never an engine rule.

**Alternatives rejected.**

- **Assist driven by held Use.** A Use press beside a downed teammate would also fire any Use-activated trigger or switch in reach, and the host cannot tell which the player meant. A separate command resolved on the client keeps the two apart.
- **`assistable` for every descriptor in `TuningPayload`.** The client would also need each remote pawn's descriptor identity and life-state id, and would carry profile tables it mostly never reads. A per-pawn stamp delivers the one answer the prediction needs.
- **No client prediction; the helper's bar shows the host's value.** The bar would start about one round trip late, against the smooth co-op presentation Spec A set as the governing preference.
- **An interim engine default (`hold` on Use's inputs).** Every mod declaring `assistable` would get Use on key-up, single player included, and the per-command default activators it needs would serve nothing once `input-contextual-priority` groups the two commands. The dev mod binds the hold itself, as it binds hold-Shift sprint.
- **A dedicated key in the dev mod.** Every gamepad face button already carries a gameplay command, and the free keyboard keys sit far from Use. L4D2, Back 4 Blood and Darktide all put revive on the use key.
- **One progress slot for helper and target.** Per-player slots are owner-private, so a target's client cannot read its helper's slot. A merged slot would also be ambiguous for a pawn that is helping while being helped. Two slots give each client its own reading.
- **A new `on.assister` token** for one `assisted()` event. A second dispatch token touches about a dozen hand-kept sites: SubjectToken, SequenceTarget, BoundTarget, DispatchScope's single event-player snapshot, `byPlayer` recognition in three installers, and five per-source rejection guards. Two discrete sources, each binding the existing `on.player` to a different pawn, deliver both bonuses with none of that.
- **Assist as a trigger volume attached to the pawn.** Trigger volumes are level-authored AABBs keyed by map identity. A pawn-carried one has no placement, tag, or install story.
- **A generic hold-to-interact mechanism with assist as its first target kind.** Pawn targets need the same accumulator either way. Assist stays player-on-player because no other hold-to-interact consumer exists; generalising later could reuse the accumulator and `assistGiven` (which binds `on.player` to the helper), but `assistReceived` binds `on.player` to a target pawn, so a non-pawn target would need its own source. That is why non-pawn assist stays Not planned rather than deferred.
- **Second wind without impact-source effects.** The kill policy could write a per-player flag through `update(store.byPlayer(source), …)`, and a player event on `inLifeState("down")` plus the flag could revive. Without `source.inLifeState` the policy cannot restrict the flag to kills made while down, so the author must clear it on every down transition. The revive also lands a tick later than an in-tick `source.setLifeState`. Task 1 is cheaper for authors and faster.
- **Revive as an engine verb.** A revive is `setLifeState` plus `grantHealth` in an authored fire list; an engine verb would fix its health and state.

**Foreclosures and one-way doors.**

- `assist_held` on `WireMovementInput` and `assistable` on `WirePlayerMovementState` take the next `WIRE_VERSION` (39 at `b6fc0c4`). Pre-stable, undone with another bump.
- SDK names (`assistable`, `assistReceived`, `assistGiven`, `player.assistProgress`, `player.assistReceivedProgress`, `source.inLifeState`, `source.setLifeState`) and the command ID `assist` become the modder contract; the command ID is also written into player bindings and manifests. Renaming any of them costs a pre-stable SDK break across both runtimes.
- `assist` has no engine default until `input-contextual-priority` adds one. The dev mod's hold-E binding is content, replaced when that spec lands; a player who rebound `assist` keeps their binding.
- Task 2 widens the per-player slot bitset (`EntityScope.warned_owned_engine`, capped at 16 by a compile-time assert) to u32. The slot count is 15 (`PlayerSlot::ALL` at `b6fc0c4`); the two progress slots take it to 17, so the widening is required for this spec to compile.
- An assist with no line-of-sight or facing requirement can be tightened later by adding optional profile keys. `holdMs` belongs to the target's state, so a helper-side speed modifier (a revive perk) needs a later helper-profile key; it is additive. Nothing material is foreclosed.

## Acceptance criteria

### Descriptor and validation
- [ ] **AC1.** A life-state profile accepts `assistable: { radius, holdMs }` in both runtimes into the same lowered shape. Loading is rejected, with the full key path, for an `assistable.radius` ≤ 0 or non-finite, an `assistable.holdMs` < 0 or non-finite, and an unknown key inside `assistable`. On every role, a pawn's movement component carries its current state's `assistable` (or none), restamped at every life-state write, and a connected client reads the same value for a remote pawn.
- [ ] **AC2.** An impact policy whose `source.inLifeState` or `source.setLifeState` names a state absent from the mod's name table is skipped at bind with a warning naming the policy and the state; the mod's other policies still bind. At runtime, `source.setLifeState` on a source whose descriptor lacks that state, or declares no states, warn-skips.

### Impact source read and effect
- [ ] **AC3.** In an impact policy, `source.inLifeState(name)` holds exactly while the source pawn is in `name`. It is false for a non-player source, which reads no warning. `source.setLifeState(name)` applies to the source pawn in-tick with Spec A's `setLifeState` semantics: the next tick's gates read the new profile, elapsed time restarts, and the current state is a no-op. A non-player source warn-skips.

### Assist
- [ ] **AC4.** Holding `assist` within `radius` of a pawn whose current state declares `assistable` completes after `holdMs`. The completion fires every `assistReceived()` subscription with `on.player` = target, then every `assistGiven()` subscription with `on.player` = helper. Both fire in the player-event pass of the tick the hold completes, before that pass's condition snapshot. Completion itself changes no life state or health; only the authored fire lists do.
- [ ] **AC5.** Paired refuse/permit edges for assist:
  - **Refused:** out of range; target in a non-assistable state; helper's state has `use: false`; release one tick before `holdMs`.
  - **Permitted:** in range on an assistable state; `holdMs: 0` completes on the first held tick.

  Progress resets when the helper releases or switches targets, when either pawn changes state, and when the target respawns. `player.assistProgress` reports the pawn's progress as helper, 0..1. `player.assistReceivedProgress` reports the progress of the assist the pawn is receiving, 0..1: the highest among its helpers, and 0 with none. A pawn helping while being helped reports both.
- [ ] **AC6.** Two helpers completing on one target in one tick produce one completion, for the earlier helper in `players()` order. The other helper's progress resets. Held `assist` survives a short input gap under the host's hold-then-neutral gap policy.

### Input command
- [ ] **AC7.** `assist` is listed and bindable exactly when some loaded descriptor declares `assistable`; otherwise it is unbound, unlisted and never part of a conflict. It has no engine default. Bound to a key of its own, it drives every assist AC above.
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
  - It drops its stale claims that per-owner + persist is rejected and that there is "no source-scoped fact vocabulary" (also in the `SourceHandle` doc comments in `sdk/lib/data_script.ts`).
  - Every command shown runs from an SDK bundle.

## Orderings

Task agents cite rows by number. Expected outcomes are binding.

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Assist completes the same tick bleed-out would expire | Assist in tick, completions applied before the player-event snapshot | Revive applies first; the bleed-out condition reads `inLifeState("down")` false and does not fire |
| O2 | Self-revive kill from a remote hit still waiting on fire authorization, landing after bleed-out expired | Bleed-out fires in tick N's player-event pass, respawn drains at tick N+1 start, hit flushed at frame N+k (k ≥ 1) | Respawn wins. The later `source.inLifeState("down")` is false and the self-revive row does not fire |
| O3 | Assist completion recorded in tick, then a post-tick remote-hit flush self-revives the target | Completion, `host_flush_pending_hit_declarations`, player-event pass | The completion is dropped at application because the target no longer declares `assistable`. No assist sources fire |
| O4 | `holdMs: 0` | First held tick | Completes that tick |
| O5 | Helper disconnects mid-hold | Seat harvest between frames | Progress dropped. The target is unchanged |
| O6 | Helper client predicts completion, but the host's accumulator reset one tick earlier (range edge under interpolation) | Client ahead of host | The host's reset wins. The client bar drains to 0 at the next snapshot and no success presentation plays |

## Tasks

New logic lands in new modules. Files already past ~800 lines (`sim/src/sim/mod.rs`, `sim/src/impact_policy.rs`, `netcode/src/lib.rs`, `netcode/src/client.rs`, `netcode/src/command_queue.rs`, `ir/scopes.rs`, `netcode/src/state_slots.rs`, `startup/reaction_validation.rs`, `main.rs`, `frame_loop/mod.rs`) receive only call-site edits and match arms. Symbol grounding: `research.md` in this folder.

### Task 1: Impact-policy source reads and effects

- **`source.inLifeState(name)`.** `EntityScope::resolve_owned_input` gains a life-state branch for `owner:"@impact.source"` names with `LIFE_STATE_INPUT_PREFIX`, resolving the id through `resolve_life_state_id`. `seed_owner_seat_from_registry` also captures the source pawn's `PlayerMovementComponent.life_state`, which the owner snapshot does not hold today. The read is false, without a warning, for a non-player source.
- **`source.setLifeState(name)`.** `bind_effect` accepts it for `IMPACT_SOURCE_TOKEN` only. `apply_planned` intercepts it the way it intercepts `SetOwnerSlot` (resolving the recipient through `resolve_command_recipient`) and calls `life_state_commands::set_life_state` with its `ctx` and registry, so elapsed reset, the same-state no-op and the weapon-activation cancel hold. `apply_effect` marks it unreachable like the other intercepted effects.
- **SDK.** Add `inLifeState` and `setLifeState` to `SourceHandle` in `sdk/lib/data_script.ts` / `.luau` and the sdk_lib templates, then regenerate types and fixtures.
- **Unknown names.** An unknown state fails `bind_policy`, which skips the policy with its existing warning (AC2). The warning names the policy and the state.
- **Test** AC2, AC3 and O2 with the `impact_policy` test helpers and Spec A's bleed-out `respawn`.
- Delivers AC2, AC3 and O2.

### Task 2: Assist

**Profile key and stamp**
- Add `assistable: { radius, holdMs }` to `LifeStateProfile` and Spec A's shared validator (AC1).
- Stamp the current state's `assistable` onto `PlayerMovementComponent` wherever Spec A stamps the other profile fields, and carry it on `WirePlayerMovementState` beside `life_state` (`movement_state_to_wire` and the merge). Bump `WIRE_VERSION`.
- On a connected client, keep `life_state` and `assistable` for remote pawns from the movement payload `client.rs` drops today as `MovementWithoutLocalComponent`. Keep them beside the remote pawn's entity; no gate runs on them.

**Input command**
- Add `Command::Assist` and `Action::Assist`: `Command::ALL`, `id`, `context`, `kind`, an `accepts` arm for `Press | Hold` (the accumulator reads a level), `gameplay_target`, `legacy_actions`, and the `CommandId` list in `sim/src/scripting/primitives/manifest.rs` (then regenerate types).
- Add a relevance fact and its `derived_relevance` arm: relevant exactly when some loaded descriptor declares `assistable` (AC7), derived in `registry_facts` and, if a client derives facts there, `tuning_facts`.
- Add `assist_held: bool` to `WireMovementInput` and `SimCommand`, threaded through `build_sim_command`, `sim_command_to_input` / `input_command_to_sim` and `neutral_sim_command` (false). `held_gap_sim_command` carries it through a gap like other levels, not clearing it as it clears edges.

- `engine_default_inputs(Command::Assist)` is empty. Test AC8 with an author binding of `assist` as `hold` on `KeyE` beside the default Use.

**Accumulator**
- A host and single-player accumulator in new module `sim/src/sim/assist.rs`. It runs in the fixed tick after the Weapons stage and `run_death_sweep`.
- Each helper whose state allows `use` (`player_life::permits` with `Capability::Use`) and who holds `assist` accumulates on the nearest pawn within its target's `assistable.radius`. Distance is between pawn origins; ties go to the earlier pawn in `players()` order.
- Target selection is its own pure function (`assist_candidate`): helper position and capability, candidate positions, life-state ids and `assistable` stamps in, chosen target out. `input-contextual-priority` calls it for availability.
- Progress resets per AC5. The accumulator stores each pawn's life-state id and resets on change. `sim::respawn::drain` calls the accumulator's reset for each respawned pawn. A helper whose pawn no longer exists (seat harvest, O5) drops its progress. One completion per target per tick (AC6).
- Completions go into a `TickEvents` `assist_completions` list (precedent `trigger_exit_fires`).
- **One rule, two runners.** The accumulator is a pure step over (helper input, helper capability, candidates, previous progress). The host runs it authoritatively. The owning client runs the same step each predicted tick for its own pawn as helper only, over interpolated remote pawns and their retained life-state ids and `assistable` stamps. It writes only its local `player.assistProgress` for presentation and never emits completions.

**Client presentation**
- When host samples of the helper's own progress arrive, the client keeps its predicted value unless the host reports a reset or a different target. In that case the predicted progress drops to the host's value (O6).
- The target client's received progress ramps instead of stepping, as an upward ramp bounded at 1. The timed-slot projection (`timed_slot_targets.rs`) covers only four weapon-fed slots at fixed indices, so this is its own display projection beside `LifeStateDisplay`, reusing `Ramp`, applied in `App::build_ui_display_slot_snapshot`.
- Success presentation (sounds, flashes) belongs to the authored `assistReceived` / `assistGiven` fire lists. Those run on host confirmation and route to each player's machine, so a prediction never plays one (AC9).

**Sources**
- `PlayerEventTable::run_tick` takes the tick's `assist_completions` and applies them before `evaluate()`. Each completion is re-checked first and dropped if the target no longer declares `assistable` (I1). Test the re-check with a direct state change between record and apply; Task 3 runs O3 end to end.
- `BoundPlayerEvent` gains a source kind: `Edge { condition, edge, program, memory, … }` or `AssistReceived` or `AssistGiven`. Discrete kinds carry no condition, program or memory.
- `player_event_from_json` accepts `edge: "assistReceived" | "assistGiven"` with no condition; it requires a condition today.
- SDK `assistReceived()` / `assistGiven()` in both runtimes and the type declarations, then regenerate.
- Fire-list validation is the player-event rule set unchanged. Each fire binds `on.player` to the target or the helper respectively.

**Progress slots**
- Add `player.assistProgress` and `player.assistReceivedProgress` as `PlayerSlot`s (17 slots), at the sites `PlayerSlot::LifeStateMs` touched: `player_slots.rs` arms, an `engine_state_catalog.rs` entry with `OwnerPrivatePlayer`, the `ui_proxy.rs` publisher, `MIRRORED_GAME_STATE_LEAVES`, regenerated types and `expected.d.*` fixtures. The host writes both from the accumulator per AC5.
- Widen the `warned_owned_engine` bitset in `ir/scopes.rs` from u16 to u32 and update its compile-time assert.

Delivers AC1, AC4, AC5, AC6, AC7, AC8, AC9, O1, O4, O5 and O6.

### Task 3: Dev-mod Borderlands upgrade, tests and docs

**Content** (`content/dev/scripts/player-death.ts` + `.luau` twin, `content/dev/scripts/player.ts`, `content/dev/scripts/hud.ts`)
- **Player descriptor:** `down` gains `assistable: { radius: 1.5, holdMs: 3000 }`.
- **Player events:**
  - `assistReceived` → `setLifeState("alive")` + `grantHealth(30)`
  - `assistGiven` → XP via `addSlot` on `progression.xp`
- **Impact event:** a separate `defineImpactEvent("second-wind", { tag: "enemy" })`, not a row inside `enemyDeath`, which the zombie override would evict. On a kill with `source.inLifeState("down")`: `source.setLifeState("alive")`, `source.grantHealth(40)`, and an XP bump via `update(progression.xp.byPlayer(impact.source), …)`.
- **HUD:** a "Reviving" bar on `player.assistProgress` and a "Being revived" bar on `player.assistReceivedProgress`, built like `bleedOutMeterTree`.
- **Input:** the dev mod's `input.ts` adds an `assist` row with label and category, bound `hold` on `KeyE` and gamepad `west` (the `ShiftLeft` sprint row is the pattern). Use keeps its default.

**Tests**
- Extend the parity surface test.
- Extend `player_death_surface_tests.rs` beside `player_death_two_player_policy_bleeds_out_on_tick_900_then_respawns_next_tick`: down → assist → alive, down → self-revive.
- Orderings row O3 end to end.
- **Manual proof**, recorded in the PR: a two-client playtest of the dev mod at 150 ms RTT (`POSTRETRO_NET_LAG`) on `coop-presentation-test`, with `assist` on the dev mod's hold-E. Check AC9 from both the helper's and the target's seat, and AC8's tap-for-Use beside a downed teammate. Record each run's lag settings and what was seen.
  - reviving and being revived
  - releasing `assist` or stepping out of range mid-hold
  - a self-revive kill

**Docs**
- Update the docs per AC12.

Delivers AC10 (residual rows), AC11, AC12, O3, and the manual proof for AC8 and AC9.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2. Task 1 owns impact policy, Task 2 owns the accumulator, input, wire and slots. Both regenerate SDK types; merge-coordinate.
**Phase 2 (sequential):** Task 3. It consumes both surfaces.

`input-contextual-priority` builds on Task 2 (`Command::Assist`, `assist_held`, `assist_candidate`, the client's predicted step) and replaces its default binding.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| Assist profile key | `LifeStateProfile` field | `assistable.radius`, `assistable.holdMs` | same | same |
| Assist stamp | `PlayerMovementComponent` field | `WirePlayerMovementState.assistable` | n/a | n/a |
| Assist command | `Command::Assist`, `Action::Assist` | `"assist"` in bindings and the manifest `input` block | n/a | n/a |
| Dev-mod assist binding | n/a | `input` block `assist`: `hold` on `KeyE` / `west` | `input.ts` | n/a |
| Held-assist input | `WireMovementInput.assist_held`, `SimCommand` | `bool` beside `use_pressed` | n/a | n/a |
| Impact source read | `resolve_owned_input` life-state branch | `{op:"input", owner:"@impact.source", name:"player.inLifeState:<name>"}` | `source.inLifeState(name)` | `source:inLifeState(name)` |
| Impact source effect | planned effect intercepted in `apply_planned` | `{ effect:"setLifeState", owner:"@impact.source", state }` | `source.setLifeState(name)` | `source:setLifeState(name)` |
| Helper progress slot | `PlayerSlot::AssistProgress` | `"player.assistProgress"` | `player.assistProgress` | same |
| Received progress slot | `PlayerSlot::AssistReceivedProgress` | `"player.assistReceivedProgress"` | `player.assistReceivedProgress` | same |
| Assist sources | `BoundPlayerEvent` source kind `AssistReceived` / `AssistGiven` | `edge: "assistReceived"` / `"assistGiven"`, no `condition` | `assistReceived()` / `assistGiven()` | same |
| Candidate query | `assist::assist_candidate` | n/a | n/a | n/a |

## Wire format

- **`WireMovementInput`** gains `assist_held: bool` beside `use_pressed`.
- **`WirePlayerMovementState`** gains `assistable: Option<{ radius: f32, hold_ms: f32 }>` beside `life_state`, `None` when the current state declares none.
- `WIRE_VERSION` goes to the next value (39 at `b6fc0c4`). `TuningPayload` and `TUNING_PAYLOAD_EPOCH` are unchanged.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1. At most one assist completion per target per tick, applied before that tick's player-event condition snapshot, and only while the target still declares `assistable` | Task 2 | Remote-hit flush between tick and player-event pass | AC4, AC6, O1, O3 |
| I2. Assist presentation: the helper's progress is predicted locally and corrected toward the host; no success cue plays without host confirmation | Task 2 | Any authored cue placed outside the assist fire lists | AC9, O6 |
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
         update(progression.xp.byPlayer(source), (xp) => xp.add(50))],
  }];
});
```

**L4D-style (Luau).** Only a teammate revives; no second wind, no bonus.

```lua
-- down = { ..., assistable = { radius = 2, holdMs = 5000 } }
players():on(assistReceived(), { revived })  -- on.player:setLifeState("alive"), on.player:grantHealth(30)
```
