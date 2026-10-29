# reaction-body-composition — research

Facts read at 167d6885d. They inform the brief without deciding it.

## Body and step shapes today
- A reaction body is exactly one of `progress`, one primitive or `sequence`. `named_reaction_from_js` and its Luau twin tell them apart by which keys are present. Any other shape is `UnknownShape`.
- Array bodies do not exist.
  - In TS an array body is a type error: `ReactionBody` in `sdk/lib/data_script.ts` has no array arm.
  - At runtime, `defineReaction` (`sdk/lib/data_script/reactions.ts`) spreads the body into `{ name, ...body }`, so an array becomes numeric keys.
  - The Luau `defineReaction` copies only known keys and yields `{ name }`.
  - Both shapes reach the parser as `UnknownShape`. A level script's drain warns "reactions[i] is malformed and was skipped"; a mod manifest's drain rejects the manifest.
- The tracer `(on) => body` runs once at declaration, and its return takes the same path as a plain body. The unnamed `defineReaction(body)` form hashes the body for its auto id (`autoReactionId`).
- `sequence_steps_from_js`/`_lua` build a target from `kind` (group), `target` (subject token) or an `id` sentinel. With none of these they fall through to a required numeric `id`, so `{ primitive: "playSound", args }` fails to parse.
- `SequenceTarget` is `Entity | Group | Activators | FiredTrigger | FiredTeleporter | EventPlayer | Wait | Fire`.
- System reactions are `SystemReactionKind::ALL`, registered by `register_system_reaction_primitives`. Builders live in `sdk/lib/ui/reactions.{ts,luau}`, and `setGhostAudio` in `postretro/audio`. Each returns `{ primitive, args }` typed `PrimitiveReactionDescriptor`. The TS `SequenceStep` union has no system arm.
- Each kind's `SystemReactionClass` (`entities` `reactions/system_commands.rs`):

  | Class | Kinds |
  |---|---|
  | `Presentation` | `playSound`, `rumble`, `flashScreen`, `vignette`, `screenShake` |
  | `MachineLocal` | `showDialog`, `openMenu`, `closeDialog`, `cellWrite` and the text edits |
  | `HostConsequence` | level flow, sentiment and `setGhostAudio` |
  | `SlotWrite` | `setState` |

- `setGhostAudio` applies on the host only and replicates (`net/src/wire/ghost_audio.rs`, `netcode` `ghost_audio_wire.rs`).
- Group and subject-token verbs return one descriptor, legal unspread as a step. Member methods, `wait()` and `fire()` return step arrays that the author must spread.

## Dispatch today
- `dispatch_primitive` routes a primitive with no tag, kind or target to `dispatch_system_primitive` → `SystemReactionRegistry::dispatch` → `ScriptCtx::system_commands`. `dispatch_sequence` takes no system registry. `sequence_primitives_are_valid` checks only the sequence registry, so a sequence naming `playSound` is dropped at `setupLevel`. The comments in V4a and `scoped_fire_targets` rely on that drop.
- `SystemCommandFireContext` carries `source`, dispatch `values`, `emitter` and `origin: FireOrigin`.
  - `FireOrigin` is `EveryMachine`, or `HostOnly(FireSubject::{None, Trigger { player, trigger, occupants }, PlayerEvent { player }, Teleport { player, teleporter }})`.
  - `FireOrigin::follow_up` keeps the variant and drops the subject.
- `SystemCommandQueue::push` resolves `Presentation`-class commands through `resolve_recipients` into `Recipients::{Local, Seats, Everyone, Skip}`. App-side, `route_player_presentation` sends `ServerPresentationPayload::Command`.
- An IR-valued `setState` binds at install in `SystemReactionIrBindings::rebuild`, which walks only `Primitive` bodies.
- Trigger partition (`partition_direct_reaction`): before the first `wait`, it classifies each primitive with `classify`.
  - `CONSEQUENTIAL_PRIMITIVES` steps bind in-tick.
  - `LIFECYCLE_PRIMITIVES` steps are lifecycle.
  - Everything else is presentation and drains as residual, but only for Entity or Group steps; other targets warn "sentinel target on presentation sequence step".
  - `bind_sequence_step` refuses `setState` outright. `bind_primitive` accepts a system-targeted one through `bind_command_at`.
- Machine-local today is the private `machine_local_effect` in `sim` `player_events/validate.rs`. It returns `MachineLocal` kinds, plus `SlotWrite` when the slot's `network` is `ReplicationScope::None`. It inspects `Primitive` bodies only.
  - It excludes `Presentation` on purpose: `direct_violation` calls it, and E16 forwards presentation to the event player instead of rejecting it. So the shared predicate cannot be today's function unchanged. It includes `Presentation` (anything never replicated and not consequential), and E16 composes "machine-local and not `Presentation`". `ui-timed-reactions` reads the predicate the same way.
  - The hosted-trigger UI warning (`descriptor_has_machine_local_effect`) is a second classifier: it walks steps by primitive name and counts only the `MachineLocal` class, never a non-replicated `setState`. Composed as E16's is, "machine-local and not `Presentation`", it gains that `setState` and does not warn on every hosted `playSound`.

## Scheduler and landings today
- `ReactionScheduler` is host and single-player only. `Session` calls `set_enabled(false)` on a connected client, and `enroll` then refuses, warning once per reaction (E18 O43).
- `InstanceKey` is `(address, body_ordinal, Option<(EntityId, PlayerId)>)`. `begin_origin` sets the origin for:
  - trigger residuals: `(trigger, player)`;
  - player-event residuals: `(pawn, PlayerId::Local(pawn))`;
  - teleport residuals: `(teleporter, player)`.
  
  Every other source parks with `None`.
- `drain_landings` dispatches every tail under `FireOrigin::HostOnly(FireSubject::None)`, whatever source parked it. Its follow-ups inherit that origin through `follow_up`, but dispatch after `drain_landings` has restored the previous fire context, through `dispatch_deferred_named_events_with_sequences`.
- Waits round up to whole ticks (`ms_to_ticks`), and landings due on one tick run in ascending enrollment id. `levelLoad` A `[wait(1010), …]` authored before B `[wait(1001), …]` both land on tick 61 at 60 Hz, A first; a client that sorted by due time would invert them.
- A non-interruptible re-fire under a parked key is ignored, and so is one whose landing is queued. Every sourceless fire keys `None`, so today a trigger's `fire(r)` child (`HostOnly(None)`) and a crossing's fire of `r` (`EveryMachine`) share key `(r, 0, None)`: the host parks the first and drops the second. Keying by origin class keeps both, so each machine plays a beat as often as it would before the wait.
- A sourceless interruptible wait is demoted on the host with a warning; V3 installs an interruptible reaction that has any Enter binding, so a crossing can fire the same reaction. A client role reusing that path would warn on every client.
- `resolve_recipients` maps `(HostOnly(Trigger | Teleport | None), no audience)` to `Everyone` for a sound. A host landing's post-wait `playSound`, or one reached through `fire`, therefore reaches every client today. That holds even for a `levelLoad` tail, while the client refuses its own copy.
- `presentation_capture::after_apply` records light, fog and emitter overrides when `fire_context().origin` is `HostOnly`, or when the target is networked. Member steps in a host landing are captured for that reason (`plans/done/coop--networked-by-default` AC10). Restoring `EveryMachine` on a landing without separating capture from routing would stop capturing them, both in the tail and in a `fire(r)` it reaches: `follow_up` keeps `EveryMachine`, so `r`'s member light step would run uncaptured on the host while the client drops it as host-only.
- Capture keyed on "any host landing" would over-reach for own-state sources. Before a wait, a host own-state crossing's member step on a client-local light runs `EveryMachine`, uncaptured, so it stays on the host. Captured after the wait, the host's low health would fade every client's light. So capture follows host-only and host-mirrored landings only.
- Seat at landing. A player-event key holds `PlayerId::Local(pawn)` and resolves its seat through `seat_for_pawn`, which is `None` both for the host's unseated pawn and for a departed one. Today `None` routes a player event or teleport to `Recipients::Local`, the host. The teleport drain already skips a residual whose seated player left for this reason; a landing has no such guard. `Registry::local_player_pawn` tells the host's pawn apart. A trigger subject resolves to an empty seat list, so only the teleport and player-event arms fall back to the host. A trigger or teleport sound with no audience resolves `Everyone` whatever the subject.

## Prior commitments this brief amends
- `plans/done/E18--timed-reaction-steps`:
  - resumed presentation is host-local;
  - client-local wait loss is "a defect the spec ships";
  - a client scheduler for presentation-only tails is the named follow-up.
- `plans/done/E18--trigger-event-fanout`: co-op stings are host-local in v1.
- `plans/done/coop--networked-by-default` absorbed `coop-trigger-screen-effects`.
  - Audience defaults by origin:

    | Origin | Sound | Screen effects |
    |---|---|---|
    | Trigger | everyone | activator |
    | Player event | the event player | the event player |
    | Host-only, no subject | everyone | host |
    | Every machine | local | local |

  - It also replicates host-only light, fog and emitter overrides.
  - Its Decision 11 and `scripting.md` §12 rest on "a presentation builder is never a sequence step, so no audience token reaches a `wait` landing". The token half stands: V4a keeps dropping a post-wait token (`audience_token`, which excludes `"everyone"`). The literal `"everyone"`, the only literal the builders accept, survives a wait and resolves as before it under the restored origin: `Everyone` from a host-only origin, `Local` from an every-machine one. No content uses a post-wait audience.
  - Its Drain origins table lists `wait` landings as "host-only, no subject"; the brief replaces that row with the parked origin.
- `plans/done/E16--player-events`:
  - presentation in a `players().on` fire list routes to the event player;
  - install rejects a machine-local effect the source cannot forward, and a `fire` that reaches presentation;
  - an interruptible `wait` is rejected for this source.
  
  Its research hands body shape, system steps and post-wait routing to this brief.
- `drafts/ui-timed-reactions` builds on this brief: system steps, array bodies and the machine-local predicate "cell writes included". It adds `uiWait` to this brief's client warn-once when it lands, and argues for one scheduler whose instances carry their own clock.

### Lib text to amend at promotion
- `scripting.md` §10.4: system reactions as steps; `setGhostAudio` as "a target-less reaction body".
- `scripting.md` §12 Role check: "Steps after a `wait` run on the host only, because the scheduler is disabled on clients."
- `scripting.md` §12 Presentation audience: "A presentation builder is never a sequence step, so no audience token reaches a `wait` landing." Presentation builders are steps; a token after a wait drops at install; `"everyone"` survives and routes as before the wait. This is the lib side of `coop--networked-by-default` Decision 11's "No presentation builder can follow a `wait`", which this brief amends the same way.
- `networking.md` §Presentation state: `wait` landings in the host-only list, and "every `wait` tail runs on the host alone".

## Where a post-wait presentation step should play
| Rule | Every-machine source (`levelLoad`, crossing) | Host-only source (trigger, teleport, player event) | Verdict |
|---|---|---|---|
| Today: host lands, origin `HostOnly(None)` | the host broadcasts sounds and keeps screen effects; clients refuse their copy | sounds everywhere; screen effects on the host, not the activator | Rejected: the wait changes who sees the effect |
| Host forwards over E16's lane | a client's own-state crossing tail is still lost | right audience | Rejected: a host own-state crossing (a low-health vignette) would shake every screen |
| A `delayMs` option on presentation builders | every machine delays its own one-shot | delayed on the host's routing | Rejected: no Exit cancel or re-fire restart, and a second timing model beside `wait` |
| A wait changes when, not where (chosen) | each machine lands its own machine-local steps; the host presents locally | the restored subject routes as before the wait | Chosen: one rule, no new message kind, E18's follow-up widened |

Movement events fire only for the machine's own pawn: the host emits them for its local pawn, never for remote pawns it simulates (`audio.md` §4). `networking.md` §Presentation state lists them among every-machine drains, but they are an own-state source. Classed as host-mirrored, a client would drop their non-machine-local steps silently although no machine runs them; classed as own-state, it warns once.

Crossings split by what they read. One that reads only shared slots fires when replication arrives, the host decision as observed. One that reads any own, owner-private or local-only slot fires per machine, so it is own-state.

Client tails run machine-local steps only. Running every client-legal step after a wait would re-simulate delayed mover commands on clients, breaking E18's invariant that delayed consequences are host-authoritative and replicated, and it would let a client run `loadLevel` unguarded. Member light, fog and emitter steps from host-only and host-mirrored tails need no client landing: the host's capture replicates them. From own-state tails they stay on the host, uncaptured, and clients drop them with the warn-once.

Why one brief: shipping system steps under today's landing would ship post-wait presentation on the wrong machines — a host own-state crossing's sound on every client, a trigger tail's screen effect on the host, not the activator — and every client tail still lost. The landing rules are what make system steps safe after a wait.

## Clock
- `levelLoad` fires at each machine's own install, so a client counting from its own install would sound a late joiner's klaxon long after the host's.
- On the host, `levelLoad` fires only inside `install_world_cpu`, and install resets `script_time` to 0 after it, so the host's fire time is host `script_time` 0. `restartLevel` goes through a full level load. The staged-manifest reload never fires `levelLoad`. Settling holds both clocks.
- The client reads host time through `ClientReplication::presentation_host_time`: the latest applied snapshot's `server_script_time`, plus half the RTT, plus local elapsed time. It is `None` until a snapshot and an RTT sample arrive, so a tail parked at install cannot test "already due" until its first reading.
- Warrant: `networking.md` §Presentation state rebases replicated animations to this clock "so a late joiner sees a finite animation end when the host's does". `scripting.md` §12's "Own state or host decision" paragraph is about player-event fire lists, not timelines.
- Stale anchor: level unload clears the anchor, but any snapshot with a newer sequence re-anchors it. The sequence is connection-scoped, `SnapshotMessage` carries no level, and the apply loop gates only on descriptors being loaded. A previous-level snapshot in flight can anchor the new level's tail to the old clock, so the first reading per level needs a guard.
- Whole ticks: each segment's wait rounds up to whole ticks on the host, and a nested `wait` re-parks during the landing. A client that compares host time with the summed milliseconds lands early by up to a tick per segment, and can split two same-tick landings across frames (S17, S25). Counting host ticks per segment matches the host.
- Late joiner. A tail parked at the joiner's install can be due at its first reading while its pre-wait steps already ran, as in `[showDialog(d), wait(3000), closeDialog()]`. Dropping it leaves `d` open forever on that machine. Replaying it sounds a klaxon the host heard long ago. Applying due state and skipping due presentation gives the level as it is now. A due segment's re-park still happens, so later segments land on schedule.
- Drift: the host scheduler counts whole ticks, the tick accumulator clamps at 250 ms, and host `script_time` adds unclamped frame time. After a host stall past the clamp, host `script_time` runs ahead of the host's tick count, so a client lands early by the clamped amount. Landing host tails on `script_time` instead would change every source's E18 landing for one stall case; the brief accepts the drift.
- A crossing over shared state fires when replication arrives, the host decision as observed, so the client's own clock already tracks it. Each machine applies the same-key rule to the edges it sees, so a crossing that flaps near the landing can play a different number of times per machine.

## `fire(r)` after this brief
| Need | Inline system step | `fire(r)` |
|---|---|---|
| One effect in a beat | yes | wrapper reaction; redundant |
| `at: on.emitter` before a wait | resolves | install drops the reaction |
| Origin and subject routing before a wait | inherits the fire | `follow_up` drops the subject |
| Order against neighbouring presentation | authored order | dispatched after the body's presentation (E18 O37b) |
| Reuse across reactions, brush KVPs, Luau/TS agreement | no | yes |
| On a client after a wait | machine-local steps land | `r` lands through the same filter |

## Array bodies
- Lowering an array to `{ sequence: [...] }` in the SDK leaves the wire, parser, partition, scheduler and light pass unchanged. It needs system steps first: `[on.activators.grantHealth(25), playSound("medkit")]` has no lowering without them.
- Rival: an engine-side array body (`ReactionDescriptor::Effects`). Rejected. It would be a third body shape for partition, scheduler, validators, light pass and recompose to learn, with semantics identical to a sequence.
- Rival: arrays without waits only. Rejected. It is a second rule to learn and protects nothing, since the lowering is identical.
- Nesting: member methods, `wait()` and `fire()` return arrays, so `[door.map((m) => m.start()), playSound("x")]` nests by construction. Flattening at author time makes spreads optional. Applying it to `sequence` lists too keeps one rule.
- Empty: `getMapEntities` returns `[]` for no match, so an array can be empty in valid content. Luau cannot tell an empty array from an empty map, and an empty table body has no other meaning.
- A reaction handle in a list most likely meant `fire(r)`; the error says so.

## Precedents
Recalled, not fetched.
- Unreal Level Sequences and Unity Timeline put audio, camera shake and events on one timeline with actor tracks.
- Source I/O chains outputs with per-output delays and targets, including `!activator`. Sound and entity outputs share one list.
- Roblox and Godot tween sequencers chain callbacks and waits in one builder.

## Ordering pins
Loopback host plus client unless stated.
| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| S1 | Mixed drain | name-fired `[m.start(), updateState(a, 1), playSound(x), updateState(b, read(a))]` | the mover starts during the walk; `a` then `b` are written at the drain, `b` reading 1; the sound plays the same frame |
| S2 | Trigger-bound pre-wait write | Enter-bound `[updateState(alarm, 1), playSound(x), wait(800), playSound(y)]` | `alarm` in the firing tick; `x` at that frame's drain; `y` at landing |
| S3 | IR `setState` across a wait | name-fired `[updateState(a, 1), wait(300), updateState(b, read(a))]`; `a` set to 2 during the wait | `b` reads 2 at landing; an IR-valued `setState` step never logs "not bound at level install"; one targeting a per-owner slot is rejected at install as a body is |
| S4 | Emitter before and after a wait | mover-event `[playSound(x, { at: on.emitter }), wait(300), playSound(y)]`; and one with `at` after the wait | the first installs, `x` positioned and `y` unpositioned; the second drops at install, naming the reaction |
| S5 | Dispatch input across a wait | crossing `[wait(300), updateState(s, read(on.rising))]`; and with the step before the wait | the first drops at install, naming the reaction; the second installs and writes the crossing's direction |
| S6 | `fire` target with a scoped step | `fire(r)` at any step position, `r = [updateState(s, read(on.rising))]` | the firing reaction drops at install, naming it (V4b over steps) |
| S7 | Cancelled tail | Enter-bound `[playSound(z), wait(800, { interruptible: true }), screenShake(…), updateState(s, 1)]`; Exit before landing; and an accepted re-fire restart | after the Exit neither tail step runs; the re-fire restarts the body, and `z` plays once more |
| S8 | Exit and Enter on one tick | Enter-bound `[updateState(alarm, 1), playSound(x), wait(800, { interruptible: true }), updateState(s, 1)]` parked; exit and re-entry on one tick | the old tail writes nothing; `alarm` and `x` once for the re-entry; its fresh tail writes `s` once |
| S9 | Every-machine tail on a client | `levelLoad` `[wait(500), npcs().update({ aggro: true }), playSound(x), m.start()]` | the client plays `x` and dispatches neither other step locally; the mover and NPC change reach it by replication; the host lands all three, presenting `x` locally; neither logs above debug about the tail |
| S10 | No double play | `levelLoad` `[wait(500), playSound(x)]`; and `[wait(500), fire(r)]`, `r` playing `x` | each machine hears `x` once, from its own landing, in both forms; no Presentation command for `x`; neither logs above debug about the tail |
| S11 | Member step in a landing | `levelLoad` `[wait(500), l.fade({ from: 1, to: 0.2, periodMs: 500 }), playSound(x)]` on a client-local light `l`; and `[wait(500), fire(r)]` with `r` holding those two steps | the host captures `l`'s change as a host-only override in both forms; the client gets it by replication; `x` once per machine, each from its own landing, with no Presentation command for `x` |
| S12 | One reaction, two origin classes | a trigger's `fire(r)`, then a crossing over shared state firing `r` inside its wait; `r = [wait(2000), playSound(x)]` | the two park separately; each machine plays `x` twice, as with `x` before the wait |
| S13 | Own-state crossing tail | crossing on `player.health` below 25 → `[vignette(…), wait(600), playSound(x)]` | each machine plays `x` for its own crossing; the host never sends it |
| S14 | Trigger tail audience | the client's player fires `[wait(300), flashScreen(…), playSound(y)]` | the flash on the client only; `y` on every machine once |
| S15 | Teleport tail audience | the client's player teleports through `t` bound to `[wait(300), flashScreen(…), playSound(y)]` | the flash on the client only; `y` on every machine once |
| S16 | Event player at landing | host plus clients A and B; `players().on` fire `[on.player.damage(5), wait(300), flashScreen(…)]` for A; and A disconnecting during the wait; and `[wait(300), fire(flash)]` in the same reaction | A only; after the disconnect, no machine, the host included, nothing above debug; the `fire` form drops at install |
| S17 | Same-frame landings on a client | `levelLoad` A `[wait(1010), closeDialog()]` then B `[wait(1001), showDialog(d)]`, authored in that order; the client's frame boundary falls between host `script_time` 1001 and 1010 ms | every machine lands A then B, by enrollment order; `d` ends shown; both tails count whole host ticks, so they share a landing frame on every machine |
| S18 | Host-timeline tail | `levelLoad` `[wait(5000), playSound(x)]`: a client installing 2 s after the host; a client whose first host-clock reading falls one tick before, or one tick after, the landing; a client joining after it | slow loader: `x` from its own landing at host `script_time` 5 s ± one tick, with no Presentation command from the host; one tick before: `x` once, no later than one tick past 5 s; one tick after, or later: never, skipped and logged at debug. The decision is taken at the first clock reading, not at park |
| S19 | Tail clock across a host level change | the host runs to `script_time` 30 s, then `restartLevel`; a previous-level snapshot reaches the client after its reinstall; same tail | `x` once, at the new level's host `script_time` 5 s ± one tick |
| S20 | Shared-state crossing tail clock | conditioned loopback, 100 ms one-way; crossing over a replicated slot → `[wait(500), playSound(x)]`; a second edge 480 ms after the first on the host and, under jitter, 520 ms on the client; and `r = [wait(800, { interruptible: true }), playSound(x)]` bound to that crossing and to a trigger's Enter | the client plays `x` 500 ms after its own crossing, within one tick; each machine applies the same-key rule on its own clock, so the host plays `x` once and the client twice; neither logs above debug. The client's crossing tail of `r` plays `x` after 800 ms and logs nothing above debug |
| S21 | Array lowering parity | `[a, [b, wait(5)], c]` vs `{ sequence: [a, b, ...wait(5), c] }`; and `{ sequence: [a, [b, [c]]] }` vs `{ sequence: [a, b, c] }` | byte-identical wire, TS and Luau |
| S22 | Host's own player at landing | the host's player fires `players().on` `[on.player.damage(5), wait(300), flashScreen(…)]`; and the host's player teleports through `t` bound to `[wait(300), flashScreen(…)]` | the flash on the host only, in both forms, as with the flash before the wait |
| S23 | Teleporter gone at landing | the client's player teleports through `t` bound to `[wait(300), flashScreen(…), m.start()]`; the client disconnects during the wait | the flash on no machine, the host included; `m` starts on the host; nothing above debug |
| S24 | Activator gone at landing | the client's player fires a trigger bound to `[wait(300), flashScreen(…), playSound(y)]`; the client disconnects during the wait | the flash on no machine; `y` once on every remaining machine |
| S25 | Two waits in a client tail | `levelLoad` `[wait(1001), m.start(), wait(1001), playSound(x)]` | the client parks the tail though its first segment holds no machine-local step; it dispatches nothing at the first landing and plays `x` within one tick of the host's landing of `x` |
| S26 | Late joiner | `levelLoad` `[showDialog(d), wait(3000), closeDialog(), playSound(k)]`; a client joining at host `script_time` 10 s | `d`, shown at the joiner's install, closes at its first host-clock reading; `k` never plays on the joiner and is logged at debug |
| S27 | `fire` among system steps | name-fired `[updateState(a, 1), fire(r), updateState(b, read(a))]`, `r = [updateState(a, 2)]` | the firing body's steps apply first, in authored order, then `r`'s: `b` reads 1 and `a` ends 2 |
| S28 | Own-state tail with a member step | crossing on `player.health` below 25 → `[wait(600), l.fade({ from: 1, to: 0.2, periodMs: 500 })]` on a client-local light `l`; the host's player and the client's player each cross | the host fades its own `l` and records no override, so no client sees the host's fade; the client drops its fade and warns once, naming the reaction and the step |
| S29 | Joiner between two waits | `levelLoad` `[wait(300), updateState(u, 1), playSound(x), wait(300), playSound(y)]`, `u` non-replicated; a client's first host-clock reading at 450 ms | `u` is written at that reading; `x` is skipped at debug; `y` plays at host `script_time` 600 ms ± one tick |
| S30 | `fire` from a client `levelLoad` tail | `levelLoad` `[wait(5000), fire(r)]`, `r = [wait(5000), playSound(x)]`; a client installing 2 s after the host | the client plays `x` at host `script_time` 10 s ± one tick |

## Consumers
- `content/dev/scripts/ghost-links.{ts,luau}`, the motivating consumer and the proof surface:
  - `echo.swell` wraps a single `setGhostAudio`, and `echo_plate` fires `[reveal, swell]`;
  - every reaction is `{ sequence: [...spreads] }`;
  - the Luau twin defines `append` and `each` only to flatten;
  - `[southCloset, limitators]` and `[returnSettle, wave]` are split only because one half waits;
  - `northTimer` (`levelLoad` `[...wait(15000), ...fire(northCloset)]`) is refused with a warning on every client. Its `fire` is real reuse: `barrier_approach` fires `closet.a1`.
  
  `docs/ghost-links.md` teaches the spreads and the whole-body `setGhostAudio`. The surface tests are in `startup/ghost_links_surface_tests.rs`.
- `content/dev/scripts/player-events.ts` has single-effect wrappers: `fanfare`, `goldFlash`, `levelShake`, `scaldHiss`, `cooled`, `bleeding`. `coop-presentation-test.ts` has more.
- `content/dev/scripts/closet-reveal.{ts,luau}`: `raiseAlarm` wraps a single `updateState`. `fire(resupply)` wraps a group verb that is already a legal step.
- Docs: `docs/scripting-reference.md` (reaction bodies, sequences, audience), `docs/ghost-links.md`, and `context/lib/scripting.md` §10.4 and §12 at promotion.

## Doors
- Subject tokens after a wait. A trigger instance already retains one activator and its trigger, so `on.activators` after a wait could resolve to that one player. That would change V4a and the meaning of "activators".
- Audience tokens after a wait. They would follow once subject tokens survive a wait.
- The client `wait` warning demotion (sdk-addressing-model Doors) is absorbed: a client tail is normal.
