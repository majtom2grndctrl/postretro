# E16--player-events — research

Read at 04f049215; re-read after the `sdk-addressing-model` merge at f29992e78. Findings that inform the brief without deciding it.

## Which machine runs each source
| Source | Host | Connected client | Single player | Gate |
|---|---|---|---|---|
| `onStateCrossing` | yes | yes, after snapshot apply | yes | none; every machine over its own slot table (`run_crossing_stage`) |
| Trigger events, map `on_fire`/`on_exit` | yes | no | yes | structural: clients run `simulate_client_wieldable_tick`, not `simulate_tick` |
| Impact policies | yes | overlay only | yes | structural |
| Named gameplay events | yes | yes, over locally produced edges | yes | per command |
| `levelLoad` | yes | yes | yes | per command |

Command-side gates on a client: `owner_slot_writes_enabled` suppresses `addSlot` and sentiment writes; `SpawnContext` suppresses runtime spawns; the scheduler and auto-close timers are disabled. Presentation commands run on whichever machine drains them. Group commands (`players()`, `npcs()`) are gated by `group_commands_apply_here`, the same predicate as `owner_slot_writes_enabled`. Irrelevant to `players().on`, which never runs on a client.

Crossings are presentation observers by design: E18 trigger-event-fanout's acceptance has a host trigger write a shared slot and a client crossing fire a local reaction after replication. A code comment on the client branch of `dispatch_primitive` says clients compose the same descriptors "for presentation work".

## Engine player slots today
On the host, `player.health` and the weapon slots hold the local pawn's value only (`publish_health_values` → `pawn_with_health` → `local_player_movement_pawn`). Owner-private replication projects every pawn from its components in `owner_private_source_value`: health first, then weapon cooldown, then the weapon slot projection, then mod per-owner values by seat, then the global scalar. That chain is the per-pawn lookup the brief lifts below `netcode`. `EngineStateCatalogEntry::slot_record` hard-codes `per_owner: false`. Ambient script reads of a per-owner record are rejected (`read_script_store_slot`), so engine slots cannot simply set `per_owner: true` without breaking every HUD read — the reason a plain read keeps meaning the local player.

## Rivals
- **A mode on `onStateCrossing`** (`eachPlayer` vs `local`, required on per-player slots). One concept, but one name covering two machine sets; a default either breaks existing crossings or silently watches the host's player. Rejected for the sibling.
- **Stored per-player engine values.** Engine systems write each pawn's value into the slot's per-seat map, as mod per-owner slots store theirs. Needs a readonly-bypassing per-seat write, a write per pawn per tick, and a second copy of component state. Rejected for the lookup.
- **Arm-only first sight** (crossings' rule). Avoids milestone re-fires for free, but a player who joins underwater never "enters" water. The owner chose experience-correct firing and the guard idiom.
- **`on.player` only as a command target.** Cheaper — no reaction-side owned reads — but the fired reaction could not read the crossing player's other values. The owner chose both.
- **Present-to-player via slots** (the damage-bearing precedent: owner-private slots, no message kind). Right for a continuous fact a HUD binds; wrong for one-shot commands, which would need a write-then-observe idiom and a per-owner absolute reaction write that does not exist. The channel is world-anchored only by its current payloads: `ServerPresentationPayload::{Spawn, OverlayFact}`; `NetServer::send_presentation` addresses one client.
- **Lift E16's per-owner crossing rejection** so a crossing watches the local projection. Drafted, then reversed after direction review: on the host it silently watches one player — the same trap as the mode-flag rival — and it would fix crossing semantics ahead of the deferred redesign. Content written against it would be the brief's one costly reversal.
- **Own-state presentation through `players().on`.** Drafted in the first sample (a low-health vignette). Reversed: a client-local crossing over the player's own replicated value has no host round trip and no loss, matching `coop-trigger-screen-effects` ("crossing-driven effects already run on the owning client") and `movement--state-transition-feel`.
- **Full split by authority.** The host source carries consequences only; all presentation is client-local crossings over replicated state, with no new message kind. Loses on host decisions that leave no replicated trace a client can observe — a non-replicating per-owner slot, a one-tick guarded edge, an effect declared once beside its consequence — which need forwarding. Wins the own-state case, which the brief adopts.
- **Step-class partition across machines** (the `scripting.md` §12 trigger partition: consequential steps in-tick, presentation steps app-side), extended so a reaction's presentation steps run on the subject's machine automatically. Considered as the mechanism under present-to-player rather than a rival to it.
- **Evaluate per frame** (the crossing stage). Sees settled slots but is frame-rate dependent — several ticks collapse into one observation — and sits outside the determinism gate. Evaluating in the Triggers stage would miss this tick's impacts, weapons and death sweep.

## Milestones and the guard idiom
Per-owner values survive level transitions, disconnect holds and (with `E16--per-player-persistence`) sessions, while edge memory is rebuilt at every level install. Under fire-at-first-sight, a milestone over such a value re-fires at each install, join and reclaim unless its condition goes false once handled. The guard (`xp ≥ 100 && level < 2`, with the reaction raising `level`) makes the condition self-extinguishing, so it is correct under any edge-memory loss — more robust than relying on edge memory, which arm-only also loses across installs. The reference example and the scripting docs teach it.

## Symptom watch: is `players().on` the shape `onStateCrossing` should have had?
The owner asked to record evidence that the crossings API was designed before the engine's netplay shape was understood. Evidence so far:
- The threshold form is redundant with the fluent algebra (`read(x).ge(t)` plus an edge word); it survives only as a second spelling with its own normalization (`raw / max`).
- Crossings' arm-only first-sight rule suits HUD presentation but not gameplay.
- `playerDied` is a single global event with no player token; in co-op a mod cannot tell who died. `players().on(died, …)` would be its natural home.
- A local twin — `onStateEvent(becomes(cond), fire)` running on every machine over its own view — would make the two sources differ only in which machines run them. It is also where a per-owner HUD watcher belongs: designing it would retire the E16 per-owner crossing rejection without the trap a bare lift carries.
- Spelling: resolved by `plans/done/sdk-addressing-model`. Per-member and per-group sources are `.on` on their target (`t.on`, `players().on`); only tag-keyed declarations and crossings wait for the redesign.
Add a line here whenever building or using either source turns up another.

## Consumers
- E24 swim slots (`player.swimming`, `player.immersion`, `player.fluid`): per-player engine slots on the lookup seam, conditions via `becomes`/`ceases`.
- E16 per-player XP level-up: the guarded milestone over mod per-owner slots.
- E23 damage bearing (decided, not built): per-owner engine slots; a candidate for the same catalog flag.
- `coop-trigger-screen-effects`: reuses present-to-player for trigger-fired effects once its whose-screen policy is settled.

## After sdk-addressing-model
That brief's research §Impact on E16--player-events listed this brief's edits; all are applied. Two calls it handed over:
- **Body shape.** A reaction body is one primitive, one `{ sequence }` or `{ progress }` (`ReactionBody`; the JS and Lua reaction parsers reject anything else). Several effects from one source are several reactions in its fire list, the existing idiom (`t.on("enter", [r1, r2])`). The owner ruled that this brief ships on that shape; multi-effect bodies and a tracer returning an array go to `reaction-body-composition`.
- **System steps.** Sequence entries must name a target (`SequenceTarget`); `playSound`-style system reactions are rejected there. The owner kept sequences target-only here and sent system steps to `reaction-body-composition`. Presentation routing needs neither: it keys on the source's fire context, which every reaction in the fire list carries.
- **`fire(r)` and routing.** The first draft let `fire` drop the subject, so a presentation reaction it reached played on the host. Direction review caught it as the mode-flag trap again — right in single player, the host's player in co-op — and silent, against the `scripting.md` §12 / E18 V4a–V4b rule that what `fire` loses is rejected loudly. Under one-effect-per-reaction bodies `fire` is the only way to put presentation inside a sequence, so it hit the main idiom. Owner ruling: an internal target player rides the fire context, separate from the `on.player` token, and `fire` passes it. After a `wait` the scheduler still keys each waiting instance by its triggering `(EntityId, PlayerId)` (`InstanceKey` in `reaction_scheduler.rs`), so post-wait routing is buildable, but it belongs to `reaction-body-composition`, which lands second; until then install drops what a wait would lose.
- **Plain reads in fired reactions.** Direction review found the condition binding `player.health` to the evaluated player while the reaction it fires read the host's. Rival: reject plain per-player reads in reactions this source fires, pointing to `byPlayer(on.player)`. It was the reversible choice (binding can be added later without breaking content) and keeps a reaction's meaning source-independent. Owner ruling: bind, keyed on the same target player as routing. One field drives both, the condition and its reactions agree, and the source-dependence is the correct one — a crossing has no target player, so a shared reaction reads its own machine's player there.
