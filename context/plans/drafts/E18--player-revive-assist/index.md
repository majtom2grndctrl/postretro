# Player Revive and Assist

## Goal

Let mod authors build revive mechanics (Borderlands second wind, L4D teammate revive) with author-chosen bonuses. The engine adds a teammate **assist** interaction and life-state reads and effects on an impact's source. The dev mod upgrades its down/bleed-out policy to full Borderlands. Depends on `E18--player-life-states` (Spec A) having landed: its life states, capability gates, verbs, name table and slots are the base this spec extends.

## Scope

### In scope

- The `assistable: { radius, holdMs }` profile key on a Spec A life state, riding `TuningPayload` with the other profile keys.
- A held-Use input on the wire.
- A host assist accumulator, with an owning-client prediction of the helper's own progress.
- Discrete player-event sources `assistReceived()` and `assistGiven()`.
- `player.assistProgress` (per-player Number slot).
- Impact-policy source read and effect: `source.inLifeState(name)`, `source.setLifeState(name)`.
- Dev-mod upgrade: teammate revive, second-wind self-revive on kill, XP bonuses, assist HUD bar.
- Updates to `docs/scripting-reference.md`.

### Out of scope

- Phases 2–4 as listed in Spec A's Out of scope.
- **Phase 3 presentation for assist:**
  - A revive prompt showing the nearest assistable teammate.
  - A target-side "being revived" slot separate from `player.assistProgress`.
- **Not planned:**
  - Line-of-sight or facing requirements on assist. Optional profile keys can add them later.
  - Assist on non-pawn targets.

## Direction

**Problem.** Spec A lets authors down and respawn a player, but nothing lets a teammate bring a downed player back, and impact policy cannot read or change its source's life state. Revive mechanics therefore cannot be authored.

**Placement.**
- **Engine mechanism vs mod policy.** The engine owns the assist accumulator: who is in range, how long Use is held, and one completion per target. These run per tick over pawn positions and held input, which the IR cannot see. The mod owns every consequence: the state a revive enters, health, bonuses, cues.
- **Host vs client.** Completion is host-authoritative. The helper's own progress bar is predicted on its client from data it already holds (interpolated pawns, replicated life states, `TuningPayload` profiles) and corrected toward the host, so host authority never shows up as latency. Success presentation plays only on host confirmation.
- **Impact policy vs player events.** Second wind is a consequence of a kill, so it is authored where kills are: an impact policy that reads and sets the *source's* life state.

**Prior commitments.**

- **Life states, capability gates, `setLifeState`, `respawn`, `inLifeState` and the name table** (Spec A). Kept. `assistable` is a closed profile key added to Spec A's set. Unknown names in the new reads and effect fail through Spec A's name-validation walk. Spec A's I3 (the engine never changes life state on HP) gains `source.setLifeState` as an authored transition source.
- **Player events are host-only, and `on.player` is legal only before `wait`** (`scripting.md` §12). Kept. `assistReceived` and `assistGiven` are discrete sources that bind the existing `on.player` at completion; no `wait` is involved.
- **Timed-slot projection lists only enumerated slots** (`done/coop--timed-slot-projection`). Kept. `player.assistProgress` is added to its enumeration explicitly.
- **"Death is modder policy derived from impact"** (Q3 archive, E16). Kept. Self-revive is an authored impact effect, never an engine rule.

**Alternatives rejected.**

- **A new `on.assister` token** for one `assisted()` event. A second dispatch token touches about a dozen hand-kept sites: SubjectToken, SequenceTarget, BoundTarget, DispatchScope's single event-player snapshot, `byPlayer` recognition in three installers, and five per-source rejection guards. Two discrete sources, each binding the existing `on.player` to a different pawn, deliver both bonuses with none of that.
- **Assist as a trigger volume attached to the pawn.** Trigger volumes are level-authored AABBs keyed by map identity. A pawn-carried one has no placement, tag, or install story.
- **A generic hold-to-interact mechanism with assist as its first target kind.** Pawn targets need the same accumulator either way. Assist stays player-on-player because no other hold-to-interact consumer exists; generalising later adds a target kind beside pawns without changing the profile key or the sources.
- **Revive as an engine verb.** A revive is `setLifeState` plus `grantHealth` in an authored fire list; an engine verb would fix its health and state.

**Foreclosures and one-way doors.**

- `use_held` on `WireMovementInput` takes the next `WIRE_VERSION` after Spec A's. `assistable` in the profile takes the next `TUNING_PAYLOAD_EPOCH` after Spec A's. Pre-stable, undone with another bump.
- SDK names (`assistable`, `assistReceived`, `assistGiven`, `player.assistProgress`, `source.inLifeState`, `source.setLifeState`) become the modder contract. Renaming them costs a pre-stable SDK break across both runtimes.
- Task 2 widens the per-player slot bitset (`EntityScope.warned_owned_engine`, capped at 16 by a compile-time assert) to u32. `player.assistProgress` is the 16th slot; without widening the next one would fail to compile.
- An assist with no line-of-sight or facing requirement can be tightened later by adding optional profile keys. Nothing material is foreclosed.

## Acceptance criteria

### Descriptor and validation
- [ ] **AC1.** A life-state profile accepts `assistable: { radius, holdMs }` in both runtimes into the same lowered shape. Loading is rejected, with the full key path, for an `assistable.radius` ≤ 0 or non-finite, an `assistable.holdMs` < 0 or non-finite, and an unknown key inside `assistable`. The key rides `TuningPayload` with the other profile keys.
- [ ] **AC2.** Install rejects any impact policy whose `source.inLifeState` or `source.setLifeState` names a state absent from the mod's name table. The error names the policy and the state. A runtime source whose descriptor lacks that state, or declares no states, warn-skips.

### Impact source read and effect
- [ ] **AC3.** In an impact policy, `source.inLifeState(name)` holds exactly while the source pawn is in `name`. It is false for a non-player source, which reads no warning. `source.setLifeState(name)` applies to the source pawn in-tick with Spec A's `setLifeState` semantics: the next tick's gates read the new profile, elapsed time restarts, and the current state is a no-op. A non-player source warn-skips.

### Assist
- [ ] **AC4.** Holding Use within `radius` of a pawn whose current state declares `assistable` completes after `holdMs`. The completion fires every `assistReceived()` subscription with `on.player` = target, then every `assistGiven()` subscription with `on.player` = helper. Both fire in the tick the hold completes, before that tick's condition snapshot. Completion itself changes no life state or health; only the authored fire lists do.
- [ ] **AC5.** Paired refuse/permit edges for assist:
  - **Refused:** out of range; target in a non-assistable state; helper's state has `use: false`; release one tick before `holdMs`.
  - **Permitted:** in range on an assistable state; `holdMs: 0` completes on the first held tick.

  Progress resets when the helper releases or switches targets, when either pawn changes state, and when the target respawns. `player.assistProgress` reports the helper's progress 0..1, and the target's when it is not itself helping.
- [ ] **AC6.** Two helpers completing on one target in one tick produce one completion, for the earlier helper in `players()` order. The other helper's progress resets. Held Use survives a short input gap under the host's hold-then-neutral gap policy.

### Client experience
- [ ] **AC7.** The helper client's assist bar shows its own progress on the first held tick, with no round-trip wait, and advances every rendered frame. If the host resets or never confirms (target moved, revived elsewhere, out of range on the host), the bar returns to 0 with no success cue. Success consequences (state, health, bonuses, their presentation) appear only after host confirmation. The target client's received-progress bar has no stair-steps at snapshot arrival.

### Orderings and consumer
- [ ] **AC8.** Every row of the Orderings table has a test asserting its expected outcome.
- [ ] **AC9.** The dev mod upgrades Spec A's down/bleed-out policy in TS and Luau twins, with the parity surface test extended:
  - `down` declares `assistable: { radius: 1.5, holdMs: 3000 }`
  - teammate hold-Use → `alive` with 30 health, and an XP bonus to the helper
  - an enemy kill while down → `alive` with a health and XP bonus
  - an assist-progress bar on the HUD

  The two-player headless test adds down → assist → alive and down → self-revive beside down → bleed-out → respawn.
- [ ] **AC10.** The docs need these changes:
  - `docs/scripting-reference.md` documents `assistable`, the assist sources, `player.assistProgress`, `source.inLifeState` / `source.setLifeState`, and the full Borderlands and L4D-style recipes.
  - It drops its stale claims that per-owner + persist is rejected and that there is "no source-scoped fact vocabulary".
  - Every command shown runs from an SDK bundle.

## Orderings

Task agents cite rows by number. Expected outcomes are binding.

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Assist completes the same tick bleed-out would expire | Assist in tick, condition snapshot after | Revive applies first; the bleed-out condition reads `inLifeState("down")` false and does not fire |
| O2 | Self-revive kill from a remote hit still waiting on fire authorization, landing after bleed-out expired | Bleed-out tick N, hit tick N+k | Respawn wins. The later `source.inLifeState("down")` is false and the self-revive row does not fire |
| O3 | Assist completion recorded in tick, then a post-tick remote-hit flush self-revives the target | Completion, flush, player-event pass | The completion is dropped at application because the target no longer declares `assistable`. No assist sources fire |
| O4 | `holdMs: 0` | First held tick | Completes that tick |
| O5 | Helper disconnects mid-hold | Seat hold | Progress dropped. The target is unchanged |
| O6 | Helper client predicts completion, but the host's accumulator reset one tick earlier (range edge under interpolation) | Client ahead of host | The host's reset wins. The client bar drains to 0 at the next snapshot and no success presentation plays |

## Tasks

New logic lands in new modules. Files already past ~800 lines (`sim/src/sim/mod.rs`, `sim/src/impact_policy.rs`, `netcode/src/lib.rs`, `netcode/src/client.rs`, `netcode/src/command_queue.rs`, `ir/scopes.rs`, `netcode/src/state_slots.rs`, `startup/reaction_validation.rs`) receive only call-site edits and match arms. Symbol grounding: Spec A's `research.md` §Task 6 (impact policy) and §Task 7 (assist).

### Task 1: Impact-policy source reads and effects

- **`source.inLifeState(name)`.** Add `EntityInputHandle::SourceLifeState`, resolved by `EntityScope::resolve_owned_input` for `owner:"@impact.source"`. It returns false, without a warning, for a non-player source.
- **`source.setLifeState(name)`.** Intercept it in `apply_planned` the way `SetOwnerSlot` is, not in `apply_effect`, which lacks the tick and pawn checks. It applies through Spec A's in-tick life-state setter, so elapsed reset and the same-state no-op hold.
- **SDK.** Add `source` handle methods to `sdk/lib/data_script.ts` / `.luau` and the sdk_lib templates, then regenerate types.
- **Name validation.** Extend Spec A's name walk in `validate_reaction_bodies_pass_a` to impact policies. Without it, `bind_policy` would silently drop the whole policy.
- **Test** O2 with the `impact_policy` test helpers and Spec A's bleed-out `respawn`.
- Delivers AC2, AC3 and O2.

### Task 2: Assist

**Profile key**
- Add `assistable: { radius, holdMs }` to `LifeStateProfile` and Spec A's shared validator (AC1). Bump `TUNING_PAYLOAD_EPOCH` to the next value after Spec A's and update its fixture.

**Input**
- Add a held-Use bit `use_held` to `WireMovementInput`, after `use_pressed`, bumping `WIRE_VERSION` to the next value after Spec A's. It follows `command_queue.rs`'s hold-then-neutral gap policy like other held inputs.
- Thread it to the host and the single-player input.

**Accumulator**
- A host and single-player accumulator in new module `sim/src/sim/assist.rs`. It runs in the fixed tick after the weapon stage and death sweep.
- Each helper whose state allows `use` (Spec A's capability helper) and who holds Use accumulates on the nearest pawn within its target's `assistable.radius`. Distance is between pawn origins; ties go to the earlier pawn in `players()` order.
- Progress resets per AC5. The accumulator stores each pawn's life-state id and resets on change. Spec A's respawn drain calls the accumulator's reset for the respawned pawn. A helper whose seat is harvested drops its progress (O5). One completion per target per tick (AC6).
- Completions go into a tick-events `assist_completions` list (precedent `tick_events.trigger_exit_fires`).
- **One rule, two runners.** Write the accumulator as a pure step over (helper input, helper state, candidate pawn positions and states, profiles, previous progress). The host runs it authoritatively. The owning client runs the same step each predicted tick for its own pawn as helper only, using interpolated remote pawns, replicated life states and `TuningPayload` profiles. It writes only its local `player.assistProgress` for presentation and never emits completions.

**Client presentation**
- When host samples of the helper's own progress arrive, the client keeps its predicted value unless the host reports a reset or a different target. In that case the predicted progress drops to the host's value (O6).
- The target client's received progress ramps instead of stepping. Add `player.assistProgress` to the timed-slot display projection's enumerated slots (`done/coop--timed-slot-projection`; it projects only listed slots) as an upward ramp bounded at 1. The helper's own predicted value overrides the projection on the helper's machine.
- Success presentation (sounds, flashes) belongs to the authored `assistReceived` / `assistGiven` fire lists. Those run on host confirmation and route to each player's machine, so a prediction never plays one (AC7).

**Sources**
- `PlayerEventTable::run_tick` applies completions before `evaluate()`. Each completion is re-checked first and dropped if the target no longer declares `assistable` (I1). Test the re-check with a direct state change between record and apply; Task 3 runs O3 end to end.
- `BoundPlayerEvent` gains a source kind: `Edge { condition, edge, … }` or `AssistReceived` or `AssistGiven`.
- `player_event_from_json` accepts `edge: "assistReceived" | "assistGiven"` with no condition.
- SDK `assistReceived()` / `assistGiven()` in both runtimes and the type declarations, then regenerate.
- Fire-list validation is the player-event rule set unchanged. Each fire binds `on.player` to the target or the helper respectively.

**Progress slot**
- Add `player.assistProgress` as a `PlayerSlot` (16 slots): catalog entry with `OwnerPrivatePlayer`, `player_slots.rs` arm, HUD publisher, `MIRRORED_GAME_STATE_LEAVES`, regenerated types and fixtures. Widen the `warned_owned_engine` bitset in `ir/scopes.rs` from u16 to u32 and update its compile-time assert.

Delivers AC1, AC4, AC5, AC6, AC7, O1, O4, O5 and O6.

### Task 3: Dev-mod Borderlands upgrade, tests and docs

**Content** (Spec A's `content/dev/scripts/player-death.ts` + `.luau` twin and `content/dev/scripts/player.ts`)
- **Player descriptor:** `down` gains `assistable: { radius: 1.5, holdMs: 3000 }`.
- **Player events:**
  - `assistReceived` → `setLifeState("alive")` + `grantHealth(30)`
  - `assistGiven` → XP via `addSlot` on `progression.xp`
- **Impact event:** a separate `defineImpactEvent("second-wind", { tag: "enemy" })`, not a row inside `enemyDeath`, which the zombie override would evict. On a kill with `source.inLifeState("down")`: `source.setLifeState("alive")`, `source.grantHealth(40)`, and an XP bump via `update(progression.xp.byPlayer(impact.source), …)`.
- **HUD:** an assist bar on `player.assistProgress`.

**Tests**
- Extend the parity surface test.
- Extend the two-player test on `player_events::tests::World` with the `impact_policy` test helpers: down → assist → alive, down → self-revive.
- Orderings row O3 end to end.
- **Manual proof**, recorded in the PR: a two-client playtest of the dev mod at 150 ms RTT (`POSTRETRO_NET_LAG`) on `coop-presentation-test`. Check AC7 from both the helper's and the target's seat, covering the cases below. Record each run's lag settings and what was seen.
  - reviving and being revived
  - releasing Use or stepping out of range mid-hold
  - a self-revive kill

**Docs**
- Update the docs per AC10.

Delivers AC8 (residual rows), AC9, AC10, O3, and the manual proof for AC7.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2. Task 1 owns impact policy, Task 2 owns the accumulator, input and slot. Both regenerate SDK types; merge-coordinate.
**Phase 2 (sequential):** Task 3. It consumes both surfaces.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| Assist profile key | `LifeStateProfile` field | `assistable.radius`, `assistable.holdMs` | same | same |
| Held-Use input | `WireMovementInput.use_held` | `bool` after `use_pressed` | n/a | n/a |
| Impact source read | `EntityInputHandle::SourceLifeState` | `{op:"input", owner:"@impact.source", name:"player.inLifeState:<name>"}` | `source.inLifeState(name)` | `source:inLifeState(name)` |
| Impact source effect | planned effect intercepted in `apply_planned` | `{ effect:"setLifeState", owner:"@impact.source", state }` | `source.setLifeState(name)` | `source:setLifeState(name)` |
| Slot | `PlayerSlot::AssistProgress` | `"player.assistProgress"` | `player.assistProgress` | same |
| Assist sources | `PlayerEventSourceKind::{AssistReceived, AssistGiven}` | `edge: "assistReceived"` / `"assistGiven"`, no `condition` | `assistReceived()` / `assistGiven()` | same |

## Wire format

- **`WireMovementInput`** appends `use_held: bool` after `use_pressed`. `WIRE_VERSION` goes to the next value after Spec A's.
- **`TuningPayload`** profiles gain `assistable: Option<{ radius: f32, holdMs: f32 }>`, absent when undeclared. `TUNING_PAYLOAD_EPOCH` goes to the next value after Spec A's.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1. At most one assist completion per target per tick, applied before that tick's condition snapshot, and only while the target still declares `assistable` | Task 2 | Remote-hit flush between tick and player-event pass | AC4, AC6, O1, O3 |
| I2. Assist presentation: the helper's progress is predicted locally and corrected toward the host; no success cue plays without host confirmation | Task 2 | Any authored cue placed outside the assist fire lists | AC7, O6 |
| I3. Spec A's I3 holds with `source.setLifeState` as an additional authored transition source; assist itself never changes life state | Task 1 | Task 2 completions (fire lists only) | AC3, AC4 |

## Script syntax examples

**Borderlands (dev mod, TS).** Spec A's policy plus revive, second wind and bonuses.

```ts
// player descriptor: Spec A's `down` gains
assistable: { radius: 1.5, holdMs: 3000 },

// player-death.ts, beside Spec A's goDown and bleedOut rows
players().on(assistReceived(), [revived]),  // on.player.setLifeState("alive"), on.player.grantHealth(30)
players().on(assistGiven(), [assistBonus]), // on.player.addSlot(progression.xp, 25)

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
