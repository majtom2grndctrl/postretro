# Input Contextual Priority

## Goal

Let several interaction commands share one key in a context. Each press drives the highest-ranked member that is available at the moment the press fires, and stays committed to it until release. Combat commands never resolve by availability, so a contextual action cannot steal a combat key mid-fight. The client resolves locally from what it already predicts, a HUD slot shows what a press will do, and the first consumer is `assist` over `use` on Use's key (`E18--player-revive-assist` AC8).

## Scope

### In scope

- A **contextual group**: two or more interaction-class gameplay commands bound to one input with the same activator (kind and threshold). The group is not a conflict.
- An engine-closed **rank** per command: `assist` (0), `use` (1). Every other command is unranked and never groups. The smallest rank among available members wins. The member with the largest rank present is the fallback and counts as available whatever the facts say, so a group always resolves to exactly one member.
- **Availability facts**: a small fact struct the App computes once per frame from the local pawn and pushes into the input system. `assist` is available when it is relevant, the local pawn's life state allows `use`, and the assist candidate query finds a target. `use` is always available.
- **Commit at fire.** The resolver picks the member when the group's slot fires — the press edge for `press`, key-up for `release` and `tap`, the min for `hold` — and drives only that member until release.
- The Steam rule generalised: a key carries at most one short slot and one hold slot, and either slot may hold a group.
- Engine default: `assist` on Use's inputs (`KeyE`, gamepad `west`).
- A readonly, client-local HUD slot `input.interaction`: the command a fresh press of a key carrying the full group performs now (`"assist"` or `"use"`).
- Controls panel: a shared mark on grouped bindings and a note stating the order. A capture onto a group partner's input applies without a conflict dialog.
- Author validation reports a grouped pair as no conflict. A mismatched pair (different activators, or a combat command) conflicts exactly as today.
- Client resolution: a connected client sends only the resolved command. No wire change.
- Dev-mod HUD prompt, docs, and the manual co-op playtest.

### Out of scope

- **Reload, or any combat command, as a group member.** Reload with an interaction on one key stays on the duration rule that exists today (reload `tap` + interaction `hold`, the Halo 3 pattern). Engine defaults keep reload on `KeyR` / `north`. The docs gain this recipe.
- **Author-declared ranks or groups** in the `input` block. Open question 1.
- **A player option** to flip the order or opt a key out of grouping. Open question 4.
- **Mod-defined commands** joining groups. They do not exist yet (`input.md` §Commands reserves `<mod_id>.<name>`).
- **Which teammate assist targets**, facing or line-of-sight on assist. Owned by `E18--player-revive-assist` (its Out of scope and Not planned).
- **UI-context groups.** UI commands keep today's conflict rule.

## Direction

**Problem.** The conflict checker refuses two short gameplay bindings on one input, and nothing in input can choose between commands by what is currently possible. So `assist` cannot default to Use's key, and no key can carry "revive if I can, otherwise use".

**Placement.**

- **Mechanism vs policy; engine vs mod.** The engine owns which commands may group, their rank, and each member's availability. The command set is engine-closed (`player_options.md` §6), and availability reads per-frame pawn state that only engine code holds. The mod owns whether keys are shared, through its `input` block defaults, and how the prompt looks, through its HUD. The player owns the final keys.
- **Input subsystem vs App composition.** The resolver lives in input, because the choice is per physical input. The snapshot is per action, so a resolver downstream of it cannot tell an `E` press from a second `use` binding on another key. Availability is computed by the App and passed in as a fact struct, as `RelevanceFacts` is. Input stays free of registry types.
- **Host vs client.** The client resolves and sends the resolved command (`use_pressed` edge or `assist_held` level). Activator resolution already sits on the client for the same reason (`input.md` §Activators): the press does what the player saw, with no round trip. If the host disagrees about availability, its assist step finds no candidate and nothing happens. The host never turns one command into the other.
- **Load time vs runtime.** Group membership is fixed when the binding table builds. The member a press drives is chosen at runtime.

**Prior commitments.**

- **Shared keys (Steam rule):** at most one short binding plus one `hold` per key in a context (`input.md` §Shared keys). Diverges, narrowly. A slot may now hold a group of interaction commands sharing one activator. Every pairing legal today stays legal with the same outcome, and press-only commands still never share. The divergence is needed because the Steam rule resolves by duration, and revive AC8 needs resolution by availability.
- **Two gameplay commands on one key conflict; the later is unbound** (`input.md` §Contexts, §Author validation; `player_options.md` §6 Conflicts). Kept for every pair except a group. A combat command beside an interaction still conflicts.
- **Activators resolve client-side; the wire carries resolved intent** (`input.md` §Activators). Kept and extended. Contextual resolution is part of the same client-side step, so `WIRE_VERSION` is unchanged by this spec.
- **Game logic never pushes state into input mid-frame** (`input.md` §7). Kept. Availability is written once per frame after the tick loop. The next frame's `snapshot()` resolves buffered edges against it. It is a previous-frame fact, like the relevance facts a table rebuild takes.
- **Client predicts only with host numbers** (`networking.md` §What gates). Kept. Client availability uses the revive spec's predicted candidate query over replicated life states and `TuningPayload` profiles.
- **Use edges ride a retained lane; held levels ride hold-then-neutral** (`networking.md` §Host input command queue). Kept. A resolved press emits one of them and never both.
- **Glyphs draw the command's first binding** (`ui.md` §4 Glyphs). Kept. Grouped members keep their table entries, so both glyphs draw the shared key.
- **The panel is the only remapping surface; activators read-only** (`player_options.md` §6 Panel). Kept. Rows stay one per command and gain a shared mark.
- **`E18--player-revive-assist` AC8, I4 and Task 2's "Shared-key default".** Delivered here (Task 1, Task 4). Open question 2 covers moving that spec's Phase 3 item here.

**Alternatives rejected.**

- **Duration split for assist vs use** (assist `hold`, use `tap` on E). This is legal today. But every Use in the game would fire on key-up instead of key-down. A tap beside a downed teammate would still open the door behind them, which breaks revive AC8.
- **Any commands may group, reload included.** This is Apex Legends' controller layout. When an interactable is in reach, reload stops working: players were forced into revives, finishers and door opens when they meant to reload. It is the Enshrouded failure class (research.md §Precedent survey, pattern 3). Excluding combat commands removes the case without a lockout timer.
- **Re-resolve every tick.** A teammate going down mid-hold would turn a held Use into an assist. A target drifting out of range would turn an assist into a Use. No working precedent switches a press mid-hold.
- **Host-side resolution** (send a raw interaction-key level and let the host pick). It adds a round trip before the player learns what the key did, the outcome can disagree with the prompt the player saw, and it adds a wire field.
- **A dedicated key for assist, with no feature.** All four gamepad face buttons already carry gameplay commands (`south` jump, `east` dash, `west` use, `north` reload). L4D2, Back 4 Blood and Darktide all put revive on the use key.

**Foreclosures and one-way doors.**

- `input.interaction` and its values (`"use"`, `"assist"`) become modder contract. Renaming is a pre-stable SDK break.
- The rule "combat commands never group" binds future mod-defined commands. Undoing it costs the anti-steal guarantee, not code.
- Saved player rows are keys only and carry no group data. Nothing persisted changes, so the change is reversible by reverting the conflict rule.
- No wire or `TUNING_PAYLOAD_EPOCH` change.

## Acceptance criteria

### Binding table
- [ ] **AC1.** On one input in the gameplay context, the binding table treats pairs as follows.
  - **Permitted:** `assist` and `use` with the same activator kind and threshold both stay bound. Neither is suppressed or flagged displaced, and validation logs nothing.
  - **Refused:** `assist` `press` with `use` `tap`, and `assist` `tap` 0.2 s with `use` `tap` 0.3 s, conflict as today, and the later in manifest order is unbound.
  - **Refused:** `use` `press` with `reload`, `drop`, `jump` or `dash` `press` conflicts as today.
  - **Permitted:** `reload` `tap` beside an `assist` + `use` `hold` group is legal, and so is a group in the short slot beside a single `hold` command.
- [ ] **AC2.** With no player rows and `assist` relevant, `KeyE` and gamepad `west` each carry both `assist` and `use`, and both glyphs draw that key. With `assist` irrelevant, or hidden by `show: false`, those inputs carry `use` alone and `assist` is unlisted. A player row binding another command to `KeyE` keeps it: `assist` and `use` defaults there are suppressed and both commands are flagged displaced.

### Resolution
- [ ] **AC3.** A `press` group resolves at the press edge, for paired cases:
  - **Assist available:** the key drives `assist` as a level for the whole hold. `use` never reads Pressed on that press, and no Use-activated trigger fires.
  - **Not available:** the key fires exactly one `use` press edge, and `assist` stays inactive.
  - **Helper's life state has `use: false`:** `assist` is unavailable, the press resolves to `use`, and Spec A's gate refuses it. Nothing acts.
- [ ] **AC4.** Commitment: while the key is held, a change in availability never re-routes it, in either direction (O3, O4). A second press re-resolves.
- [ ] **AC5.** Each activator kind fires its group at the right moment:
  - **`tap`, `release`:** resolves at key-up against availability at that snapshot.
  - **`hold`:** resolves when the min passes.
  - **Short group beside a `hold` partner:** resolves at a release before the min; past the min the hold partner fires and no member does.
  - **`reload` `tap` beside an interaction `hold` group:** a release before the min reloads, and past the min the group fires.
- [ ] **AC6.** Losing focus, a capturing tree opening, an active-pad switch or disconnect, and a table rebuild each leave a committed or pending group firing nothing afterwards: no member pulses and no fallback fires. A key held across a cancel acts again only on its next press. A key held across a rebuild keeps its committed member down until release (O9–O11).

### Prompt and co-op
- [ ] **AC7.** `input.interaction` reads `"assist"` exactly on frames whose availability facts mark `assist` available, else `"use"`. A press resolved in frame N+1 drives the command the slot showed in frame N. The case is tested with availability flipping on the frame before the press, and on the frame of the press.
- [ ] **AC8.** In a two-peer test, a connected client's press of the shared key resolves on the client and sends exactly one of `use_pressed` and `assist_held`. Cases:
  - **Client and host both see a candidate:** assist progresses and no Use-activated trigger fires.
  - **Neither sees one:** Use fires as today.
  - **Client sees one, host does not:** nothing happens on the host (O6).
  - **Client sees none, host does:** the host applies Use and no assist starts (O7).

  `WIRE_VERSION` and `TUNING_PAYLOAD_EPOCH` are unchanged by this spec. This discharges `E18--player-revive-assist` AC8.

### Authoring and panel
- [ ] **AC9.** An `input` block entry that binds `assist` and `use` to one input with one activator loads with no warning, in TS and Luau. The refused pairs of AC1, written in a block, each log the existing `conflicts with` warning and unbind the later entry.
- [ ] **AC10.** The controls panel behaves as follows for grouped bindings.
  - Each grouped binding's cell carries a shared mark.
  - A note naming the order ("Shared keys do the first that applies: Assist, then Use") shows exactly when some row has a mark.
  - Capturing `use` onto the input `assist` holds alone (or the reverse) applies with no dialog and forms a group.
  - Capturing `reload` onto a grouped input opens the conflict dialog naming both `assist` and `use`.
  - The panel, note included, fits the 1280×720 reference canvas.

### Orderings, consumer, docs
- [ ] **AC11.** Every row of the Orderings table has a test asserting its expected outcome.
- [ ] **AC12.** The dev mod's HUD shows a prompt (`Glyph({ command: "assist" })` and "Revive") only while `input.interaction` is `"assist"`. `docs/scripting-reference.md` documents shared interaction keys, the rank, commit-at-press, `input.interaction`, and the reload `tap` + interaction `hold` recipe. Every command shown runs from an SDK bundle. A two-client manual playtest at 150 ms RTT is recorded in the PR (Task 4).

## Orderings

Task agents cite rows by number. Expected outcomes are binding. "Available" means the availability facts the App published after frame N's tick loop.

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Press and release of a `press` group between two snapshots, assist available | Both edges buffered, resolved in one `snapshot()` | One assist level for that frame, then released. No Use edge |
| O2 | Same, assist unavailable | Same | One Use press edge. No assist |
| O3 | Key held committed to `use`; a teammate goes down in range | Facts flip at frame N+k | Key stays `use`. No assist until release and re-press |
| O4 | Key held committed to `assist`; target leaves range or is revived elsewhere | Facts flip at frame N+k | Key stays `assist` (the assist step stops progressing). No Use fires, on the hold or on release |
| O5 | Availability flips on the frame before the press, and on the frame of the press | Facts at N, press edge resolved at N+1's `snapshot()` | Resolves by frame N's facts in both cases; the prompt drawn at N matches |
| O6 | Client predicts a candidate, host's step finds none (range edge under interpolation) | Client sends `assist_held` | Host does nothing. No Use, no trigger. Revive O6 governs the bar |
| O7 | Client predicts none, host's step would find one | Client sends `use_pressed` | Host applies Use (fires a Use-activated trigger in reach, if any). No assist |
| O8 | Two inputs both carrying the group (E and a second bound key) pressed in one frame | One `snapshot()` | Each resolves on its own against the same facts. One command down, counted once per input |
| O9 | Focus loss or capturing tree while committed to `assist` | `cancel_all` | Assist lifts with no pulse. No Use. The held key is inert until seen up |
| O10 | Active pad switches or disconnects while `west` is committed | `park_gamepad` | Same as O9 for the pad input |
| O11 | Table rebuild mid-hold: host tuning makes `assist` irrelevant, or the player rebinds `assist` away | Rebuild while committed | The committed member stays down until release if its binding survives. Nothing new fires on this press. The next press resolves under the new table |
| O12 | `hold` group: key down at t, facts flip before the min elapses | Min elapses at a later `snapshot()` | Resolves by the facts at the snapshot where the min passes |
| O13 | `reload` `tap` + group `hold`: release exactly at the min | Strict min (`input.md` §Shared keys) | Reload fires; the group does not |
| O14 | Single player or listen host, own pawn | Same resolver, authoritative facts | Same outcomes as rows O1–O5 |

## Tasks

New logic lands in new modules: `crates/postretro/src/input/contextual.rs` (rank-free helpers: facts, panel mark text) and `crates/postretro/src/app/contextual_availability.rs`. Files past ~800 lines (`input/system.rs`, `app/controls_panel.rs`, `frame_loop/mod.rs`, `main.rs`) receive only call-site edits, no logic, so none needs a split first. Prerequisite: `E18--player-revive-assist` Task 2 has landed without its shared-key default. That gives `Command::Assist`, `Action::Assist`, `assist_held` threaded through `build_sim_command`, the accumulator in `sim/src/sim/assist.rs`, and its client predicted step. Symbols: research.md §Grounding.

### Task 1: Thin slice — `press` group, availability, client resolution

This slice falsifies the boundary assumptions: grouped members survive the table, the resolver sees availability at edge time, and the client has a candidate before the press.

**Rank.** Add `Action::contextual_rank() -> Option<u8>` in `input/types.rs` as an exhaustive match: `Assist` → 0, `Use` → 1, every other action → `None`.

**Grouping rule.** In `binding_table::conflicts`, before the existing same-context test, two gameplay-context bindings on one input are not a conflict when both commands' `gameplay_target` actions are ranked and their `activator` values are equal (kind and threshold). Every caller of `conflicts` follows from this one edit: `collides`, `resolve_collisions`, `restore_guard`, `rebind::holders_of`, `author_layer::report_collisions` and the test-only `conflicting_pairs`.

**Default.** Set `engine_default_inputs(Command::Assist)` to `KeyE` / `GilrsButton::West`, replacing whatever dedicated default revive Task 2 shipped.

**Facts.** Add `ContextualAvailability { assist: bool }` with `available(action)` in `input/contextual.rs`. `Use` is always available. `available` is never consulted for an unranked action. Add `InputSystem::set_contextual_availability`, a setter beside `set_hold_timing_scale` that stores the facts for `resolve_pending_edges` to pass to the resolver.

**Resolver.** In `ActivatorResolver::input_down`, gather the `press` bindings on the input whose actions are ranked and whose activators are equal; they form the group. Pick the member with the smallest rank the facts mark available. The member with the largest rank present is the fallback and counts as available, so a pick always exists. Set only that binding down and record it on `HeldInput` as the committed binding. `input_up` lifts it. Ungrouped bindings on the same input resolve as today. `cancel_all`, `park_gamepad` and `rebind` already lift or keep only bindings that are down, so the commitment needs no change there in this task.

**App availability.** In new module `app/contextual_availability.rs`, compute the facts once per frame after the fixed-tick loop. `assist` is available when all three hold:
- `assist` is `Relevance::Relevant` in the effective table.
- `player_capability(local pawn, Capability::Use)` (Spec A) is not `Some(false)`.
- The assist candidate query finds a target for the local pawn as helper.

The candidate query is the target-selection part of revive's pure step in `sim/src/sim/assist.rs`. If that step does not already expose selection on its own, extract it there as a behavior-preserving pure function. The query runs over these sources:
- **Single player and listen host:** registry pawns.
- **Connected client:** the inputs revive's predicted step already uses (interpolated remote pawns, replicated life states, `TuningPayload` profiles), read from the same call site.

`frame_loop/mod.rs` gets one call after the tick loop that computes the facts and passes them to `InputSystem::set_contextual_availability`.

**Tests.** AC1, AC2 and AC3 in `binding_table_tests.rs` and `activator_tests.rs`. O1, O2, O5 and O14 in single player. One two-peer test of O6 and O7 on the netcode harness used by revive's two-player test.

Delivers AC1, AC2, AC3, O1, O2, O5, O6, O7, O14.

### Task 2: Resolver breadth

Extend `ActivatorResolver` (`input/activator.rs`) so a group may fill either slot of a key: the short slot (`press`, `release` or `tap`, all members equal) or the hold slot (`hold`, all members equal). The member is chosen at the slot's fire moment, against the facts current at that `snapshot()`:
- **`release`, `tap`, and a short group beside a `hold`:** in `input_up`, at the `pulse`. Pulse only the chosen member.
- **`hold`:** in `fire_holds`, set only the chosen member down.

The existing Steam-rule timing is unchanged: `shares_hold`, strict min, and a tap's max within it. The edge cases need these behaviours:
- `HeldInput`'s committed binding survives `rebind` while the binding is unchanged, and goes `stale` otherwise.
- `cancel_all` and `park_gamepad` drop the commitment with no pulse.
- Two grouped inputs commit independently.

Tests go in `activator_tests.rs`.

Delivers AC4, AC5, AC6, O3, O4, O8, O9, O10, O11, O12, O13.

### Task 3: Authoring and the controls panel

**Validation.** Add `author_layer_tests.rs` cases. A block grouping `assist` and `use` on one input loads silently, in both TS and Luau drains through `ModInputBlock`. The refused pairs of AC1 log `report_collisions`' existing warning and unbind the later entry. No validator code changes beyond Task 1's `conflicts` edit; if a case fails, fix it in `conflicts`.

**Panel mark and note.** In `input/contextual.rs`, add two pure helpers:
- One returns whether an effective binding shares its input with a group partner on its class.
- One returns the note text, with members in rank order and labelled by `controls_panel::command_label`.

`app/controls_panel.rs` gets call-site edits only:
- `controls_rows` carries the shared flag with each `(input, activator kind)`.
- `row_cells` appends a shared mark after `activator_suffix`.
- The panel builder adds the note line under the same show-when-any rule as the displaced note.

**Capture.** `rebind::propose_rebind` returns `Clean` for a capture onto a group partner's input, through `holders_of`'s `conflicts` filter. A capture of a non-member onto a grouped input returns `Conflict` naming every member. Test both in `app/controls_panel_tests.rs`, plus the 1280×720 fit with the note shown.

Delivers AC9, AC10.

### Task 4: HUD slot, dev mod, docs, manual proof

**Slot.** Add `input.interaction` to `engine_state_catalog.rs`, modeled on `input.mode`:
- `EngineStateValueType::Enum` with values `["use", "assist"]` and default `"use"`.
- `Readonly`, `ReplicationScope::None`, not persisted.

Write it in a new frame system `scripting/frame_systems/interaction_prompt.rs`, called from the same post-tick point as Task 1's availability call, with the value `"assist"` iff the facts just published mark it available. The slot and the resolver therefore read one value. Regenerate SDK types and fixtures.

**Dev mod.** Add a prompt to `content/dev/scripts/hud.ts`: `Glyph({ command: "assist" })` plus "Revive", shown while `input.interaction` equals `"assist"`. Mirror it in any Luau HUD twin the dev mod ships.

**Docs.** In `docs/scripting-reference.md`'s *Input commands and the `input` block* section, cover:
- shared interaction keys and the rank
- that combat commands never share by context
- commit-at-press
- `input.interaction`
- the reload `tap` + interaction `hold` recipe

Every command shown runs from an SDK bundle.

**Manual proof**, recorded in the PR: two clients on the dev mod at 150 ms RTT (`POSTRETRO_NET_LAG`) on `coop-presentation-test`, from the helper's seat on keyboard and gamepad. Record the lag settings and what was seen for each case:
- a press beside a downed teammate
- a press with none in range, at a door
- holding Use when a teammate goes down in range
- a press at the radius edge while the target crawls

Delivers AC7, AC8 (its manual leg), AC11 (residual rows), AC12.

## Sequencing

**Phase 1 (sequential):** Task 1. It is the thin slice, falsifying the table, resolver-timing and client-availability assumptions. Every later task consumes its rank, `conflicts` edit and facts.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4. Task 2 owns `input/activator.rs`. Task 3 owns the author-layer tests, `input/contextual.rs`'s panel helpers and `app/controls_panel.rs`. Task 4 owns the catalog, the frame system, content and docs. Tasks 3 and 4 both add to `input/contextual.rs`; merge-coordinate.

## Rough sketch

- Resolution needs no new per-frame allocation: the group is the subset of `bindings` already scanned in `input_down`/`input_up`.
- `ContextualAvailability` stays `Copy`. Adding a ranked command later is one `contextual_rank` arm plus one fact field, and both matches fail to compile until it is classified.
- The facts are computed after the tick loop, so a frame with zero ticks still republishes them from the current pawn state.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| Interaction prompt slot | catalog entry `"input.interaction"` | not replicated | `getGameState().input.interaction` | same |
| Slot values | `["use", "assist"]` | n/a | `"use" \| "assist"` | same strings |
| Grouped binding | two `EffectiveBinding`s, one input, equal `Activator` | manifest: same `input` and `activator` under `use` and `assist` | same | same |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1. A press of a grouped input drives exactly one member, chosen when its slot fires, until release. No other member goes down on that press | Task 1 (`press`), Task 2 (other slots) | `cancel_all`, `park_gamepad`, `rebind`, a second grouped input | AC3, AC4, AC6, O3, O4, O8–O11 |
| I2. Prompt equals resolution: a press resolves against the facts whose value `input.interaction` showed on the last drawn frame | Task 1 (facts after tick loop), Task 4 (slot written from the same value) | Any second availability computation; a write moved before the tick loop | AC7, O5 |
| I3. Only ranked interaction commands group. A combat command's binding is decided by the player's input alone | Task 1 (`contextual_rank`, `conflicts`) | A new command classified ranked; a `conflicts` edit that drops the ranked test | AC1, AC9 |
| I4. The host never resolves a remote key. The wire carries one resolved command per press, and host disagreement yields no action (revive I4) | Task 1 | Any host path that derives Use from `assist_held` or the reverse | AC8, O6, O7 |
| I5. Input stays free of registry types; availability enters only as `ContextualAvailability` | Task 1 | Task 2 reaching registry state from the resolver | Review; AC3 tests build the resolver with facts alone |

## Script syntax examples

**Defaults need no authoring.** With an `assistable` life state declared, `E` and `west` carry `assist` and `use`.

**A Halo-style reload/interact key** (TS):

```ts
input: {
  commands: {
    reload: { gamepad: [{ input: "west", activator: "tap" }] },
    use:    { gamepad: [{ input: "west", activator: "hold", threshold: 0.3 }] },
    assist: { gamepad: [{ input: "west", activator: "hold", threshold: 0.3 }] },
  },
},
```

A tap reloads. A hold of 0.3 s or more assists when a downed teammate is in range, and uses otherwise.

**HUD prompt** (TS):

```ts
HStack({ gap: 8, visibleWhen: stateEquals(getGameState().input.interaction, "assist") }, [
  Glyph({ command: "assist" }),
  Text({ content: "REVIVE" }),
]),
```

## Open questions

1. **Author-declared ranks.** Should the `input` block let authors reorder or declare groups? Recommendation: no, for now. Commands and availability predicates are engine-closed, and the one group has one sensible order. Revisit when mod-defined commands exist.
2. **Owner of revive's shared-key default.** `E18--player-revive-assist` Phase 3 holds "Task 2's shared-key default and AC8". This spec delivers both (Task 1, AC8) so the mechanism ships with its consumer. Recommendation: retarget that Phase 3 line to "delivered by `input-contextual-priority`" when either spec is promoted.
3. **Assist beats a door in reach.** Revive has no facing requirement, so beside a downed teammate the shared key revives even while the player faces a door. This is Apex's door-versus-revive case inverted, and the prompt shows it. Recommendation: accept it, and revisit with revive's optional facing key if playtests complain.
4. **Player priority flip or opt-out.** Apex offers a priority flip. Recommendation: no option now. A player who wants Use alone rebinds `assist`, and the panel's shared mark and note make the grouping visible.
5. **Prompt while held.** `input.interaction` shows what a fresh press would do, not the held key's commitment. Recommendation: keep it that way. The revive bar already shows a committed assist.
