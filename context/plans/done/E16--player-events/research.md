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
On the host, `player.health` and the weapon slots hold the local pawn's value only (`publish_health_values` → `pawn_with_health` → `local_player_movement_pawn`). Owner-private replication projects every pawn from its components in `owner_private_source_value`: health first, then weapon cooldown, then the weapon slot projection, then mod per-owner values by seat, then the global scalar. That chain is the per-pawn lookup the brief lifts below `netcode`. `EngineStateCatalogEntry::slot_record` hard-codes `per_owner: false`. Ambient script reads of a per-owner record are rejected (`read_script_store_slot`), but setting `per_owner: true` on an engine slot would not break HUD binds or `bindState`, which read the retained scalar projection. It would break every IR read of the slot: local crossings (the threshold arm skips per-owner slots at bind, and predicate binds refuse them), impact-policy ambient reads and `setState` runtime values. The script `storeRead` path rejects them too. That is the reason a plain read keeps meaning the local player.

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
- Building surfaced two rejection units for one token family: a crossing strips an address only when every reaction there needs trigger context, while a player event drops an address when any reaction there breaks a rule. A redesign that unifies the sources should pick one.
Add a line here whenever building or using either source turns up another.

## Consumers
- E24 swim slots (`player.swimming`, `player.immersion`, `player.fluid`): per-player engine slots on the lookup seam, conditions via `becomes`/`ceases`.
- E16 per-player XP level-up: the guarded milestone over mod per-owner slots.
- E23 damage bearing (decided, not built): per-owner engine slots; a candidate for the same catalog flag.
- `coop-trigger-screen-effects`: reuses present-to-player for trigger-fired effects once its whose-screen policy is settled.

## After sdk-addressing-model
That brief's research §Impact on E16--player-events listed this brief's edits; all are applied. Calls it handed over:
- **Body shape.** A reaction body is one primitive, one `{ sequence }` or `{ progress }` (`ReactionBody`; the JS and Lua reaction parsers reject anything else). Several effects from one source are several reactions in its fire list, the existing idiom (`t.on("enter", [r1, r2])`). The owner ruled that this brief ships on that shape; multi-effect bodies and a tracer returning an array go to `reaction-body-composition`.
- **System steps.** Sequence entries must name a target (`SequenceTarget`); `playSound`-style system reactions are rejected there. The owner kept sequences target-only here and sent system steps to `reaction-body-composition`. Presentation routing needs neither: it keys on the source's fire context, which every reaction in the fire list carries.
- **`fire(r)` and plain reads in fired reactions.** The first draft let `fire` drop the subject silently, so its presentation played on the host. Direction review caught the mode-flag trap again; an owner ruling then carried one internal target player through `fire` and bound plain per-player reads to it. A second review reversed that, and the owner adopted the reversal:
  - Binding a sourceless reaction's reads to its caller makes them dynamic, against `scripting.md` §12 ("no knowledge of what fires it"), `plans/done/E16--per-player-currency` (an implicit owner in a sourceless reaction is a wrong-owner bug) and E18 V4b/O54 (`fire` carries no context).
  - One field driving routing and reads forecloses `coop-trigger-screen-effects`: routing trigger presentation to the activator through it would silently move every trigger reaction's plain reads off the host's player.
  - Rejecting is reversible (binding can arrive later as sugar); binding is not (undoing it moves reads silently).
  - Shipped: plain reads bind in the condition only and are rejected in fired reactions; `fire` stays context-free and a `players().on` reaction whose `fire` reaches presentation or a plain per-player read is rejected at install; the internal target player routes fire-list presentation only. Cost: explicit `byPlayer(on.player)` in reactions, and no presentation reuse through `fire`, which listing the reaction in the fire list covers.
  - Unnamed rival, ranked second: presentation verbs on the token (`on.player.flashScreen(…)`), making routing authored. Reverses `plans/done/E18--trigger-event-params`' presentation placement.
- **Post-wait routing.** The scheduler keys each waiting instance by its triggering `(EntityId, PlayerId)` (`InstanceKey` in `reaction_scheduler.rs`), so a host-landed tail can reach one player. `reaction-body-composition` owns it.

## Ordering pins
Acceptance rows cite these by id. P5, P11 and P17 leave the choice to the plan of record; the row states it.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Players A and B both cross on tick N | Fires order by tick, then event in composed order, then player in the `players()` group's resolution order (registry slot order) | Two fires, in group order, identical across runs and across identical registry histories |
| P2 | A's fire on tick N writes state another evaluation that tick reads (a shared `updateState`, `players().damage`, `on.player.damage`, a second event's condition) | Snapshot: every (event, player) condition evaluates against the settled tick before any fire's in-tick commands apply | No same-tick fire sees another same-tick fire; the write is seen on tick N+1, whatever the player or event order |
| P3 | Condition flips inside one frame | Evaluated once per tick, never per frame | Over and back within one tick: no fire. True on tick 1 and false on tick 2 of one frame: `becomes` then `ceases`, both drained that frame in tick order |
| P4 | A remote client's hit authorized on tick N lands after the fixed-tick sim stage, with its own death sweep | Evaluation follows every damage application and death sweep of the tick | The condition sees that damage on tick N, as it does a host hit |
| P5 | Global accumulator slot in a condition | Accumulators advance after the fixed-tick sim stage | Pinned either way: seen on tick N if evaluation follows accumulators, else N+1. The row states which |
| P6 | Slot written by a frame-end reaction drain | Drain runs after the frame's ticks | Seen on the following tick (existing row) |
| P7 | Seat-bound remote pawn whose client has sent no input yet | Player set is the group's, not the tick's resolved-command list | Observed from its first tick bound; first sight fires then, not at its first input |
| P8 | Seat released and a new seat admitted before the next tick | Release drops edge memory before the next evaluation | New seat is unobserved; `becomes` fires once for it if its condition holds; edge memory returns to the live player count |
| P9 | Fire on the last tick before a level transition | Residuals drain before teardown or are discarded | No command or presentation from the old level lands in the next; next level starts every player unobserved |
| P10 | Hot reload or level-tag change recomposes `playerEvents` | Edge memory is keyed by (condition, edge), not position, and carried across recompose | A surviving condition and edge fire nothing for players whose condition already held, whatever its fire list; a new or changed condition starts every player unobserved |
| P11 | Player-event fire and trigger fire on the same tick | Inter-source drain order delegated to executor | Must hold regardless: both drain in the frame of their tick; each source keeps its tick and authored order; order identical across runs; a test fails if the two batches swap |
| P12 | Target pawn despawns between evaluation and drain | Residual resolves `on.player` at drain | Commands warn-skip, siblings apply; engine-slot `byPlayer(on.player)` read warn-skips its reaction rather than reading a default; presentation still routes to the player while they participate |
| P13 | Forwarded presentation reaches a client after it is held, demoted or between levels | Client presentation lane discards while not participating | Nothing presents; nothing presents later in the next level |
| P14 | Peer on the previous wire version | Version gate precedes decode | Refused before decode; existing presentation payloads keep their tags |
| P15 | Zero players (no pawn bound) | Evaluation over an empty set | Nothing fires, nothing logs above debug |
| P16 | Two `playerEvents` over one condition (mod-global and level, or two in one manifest) | Dedupe by resolved (condition, edge, reaction), as trigger bindings do (`scripting.md` §12); distinct entries order mod-global before level, authored order within each | A shared triple fires once, with one warning naming both entries; distinct entries all fire in that order |
| P17 | `byPlayer(on.player)` read in a fired reaction | In-tick commands read at the fire's tick; residual reads at the drain | Row states which; a two-tick frame where the value changes between fire and drain distinguishes them |
| P18 | Condition reads an engine slot with no value for that pawn (no weapon in active slot) | The player is unobserved for that event while the value is absent | Losing the value fires nothing; regaining it applies first sight, so `becomes` fires once if the condition holds |
| P19 | Player at zero health | Pawns persist at zero HP | Dead player stays observed; no first-sight re-fire follows death |
| P20 | `on.player.damage` from a fire drops a player to zero after the death sweep | Death sweep already ran | Death reported on the next tick, once |

## Review-brief rulings
Owner rulings on the `/review-brief` findings, with the reason each went the way it did.
- **Same-tick snapshot (P2).** Interleaving makes outcomes depend on player order — the race "first to 100 XP claims the bonus" would fire for whichever player the registry walks first. A snapshot keeps every same-tick outcome order-independent and inside the determinism gate; a guard written by a fire takes effect next tick.
- **`updateState` takes fluent values.** In-reaction `byPlayer(on.player)` reads had no consumer: `updateState` took only literals and raw `runtime.*` values, and `addSlot` takes a literal delta. Rivals were an owner form of `runtime.read`, or dropping in-reaction reads from v1 with `recordLevelUp`. Widening the existing verb keeps one write spelling.
- **`changes` and `on.rising` deleted.** No first consumer uses both edges: XP uses `becomes`, swim uses `becomes` or `ceases`. A mod that wants both registers one event per edge; the word, its param and its wire token are additive later.
- **Recompose keeps edge memory for unchanged events (P10).** Resetting re-fires every holding `becomes` at each hot reload; the guard idiom covers milestones, but an unguarded scald re-damages every overheated player. Keying by content rather than position follows `scripting.md` §12 ("Addresses survive it; positions do not").
- **Duplicates bind once (P16).** Trigger manifest bindings already dedupe by resolved triple with one warning, because running a reaction twice doubles its effects; a mod-global milestone repeated by a level would double-credit XP.
- **Rejection strips one subscription.** V4a empties a reaction's body, which would silently kill a reaction shared with a crossing that is valid there. The crossing sentinel rule strips only the offending subscription; every rejection in this brief follows it.
- **Every context-free route is checked.** A completion follow-up (`onComplete`) dispatches with no context, as `fire` does, so a dialog or plain read reached that way lands on the host just as one reached through `fire` would.
- **No value means unobserved (P18).** Reading the slot's default would fire `becomes(read(player.ammo).lt(5))` on holstering and on every weapon swap. Unobserved matches crossings, which disarm on a missing value, and the pawnless rule.
- **Untestable rows replaced.** The flash limiter runs in the renderer's GPU frame, unreachable from `cargo test`; the automated row now asserts identical client screen-effect state, and a Manual row covers the limiter. "Byte-identical before and after the conversion" compared two builds; the row now derives every owner-private slot from the catalog.
- **Problem example.** The air example needed per-owner `accumulate` and E16's open environmental-damage policy, both out of scope; overheating burns the wielder with slots that exist today.
- **Lookup placement (premise F4).** The owned-read scopes sit in `scripting-core`, below `sim` and `combat-model`, so either home needs a callback into those scopes, or the lookup moves into `scripting-core`. Left to the executor.

## Rejection granularity
This brief's install rejections strip one subscription; E18 V4a still empties the whole reaction for a post-`wait` subject token. The scopes match their defects: a source-specific rejection is wrong only under that source, so the reaction keeps running elsewhere, while a post-`wait` token is wrong under every source that reaches it.

## Second premise pass
Applied without owner questions, each settled by PostRetro's goals or precedent: rejection unit is the address (a mixed address is dropped whole, since no runtime skip exists for a misrouted effect); recompose memory keys on (condition, edge), so fire-list edits during iteration re-fire nothing; a level entry with `levels` is rejected as a misplaced trigger form is; condition dedupe is structural; the example reads the dev mod's per-owner `progression.xp`, which kills already credit, so the playtest can observe a level-up; store-ref `byPlayer` widens to `on.player`; `onComplete` install rejection is a new V4b extension; post-`wait` `on.player` stays V4a's reaction-wide drop.
