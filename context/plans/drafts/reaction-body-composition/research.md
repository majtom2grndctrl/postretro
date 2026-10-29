# reaction-body-composition — research

Facts read at 7eab3e3e7. They inform the brief without deciding it.

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
  | `Presentation` | the five presentation verbs |
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
- Machine-local today is the private `machine_local_effect` in `sim` `player_events/validate.rs`. It returns `MachineLocal` kinds, plus `SlotWrite` when the slot's `network` is `ReplicationScope::None`. It inspects `Primitive` bodies only. The hosted-trigger UI warning (`descriptor_has_machine_local_effect`) already walks steps by primitive name.

## Scheduler and landings today
- `ReactionScheduler` is host and single-player only. `Session` calls `set_enabled(false)` on a connected client, and `enroll` then refuses, warning once per reaction (E18 O43).
- `InstanceKey` is `(address, body_ordinal, Option<(EntityId, PlayerId)>)`. `begin_origin` sets the origin for:
  - trigger residuals: `(trigger, player)`;
  - player-event residuals: `(pawn, PlayerId::Local(pawn))`;
  - teleport residuals: `(teleporter, player)`.
  
  Every other source parks with `None`.
- `drain_landings` dispatches every tail under `FireOrigin::HostOnly(FireSubject::None)`, whatever source parked it. Its follow-ups inherit that origin through `follow_up`.
- `resolve_recipients` maps `(HostOnly(Trigger | Teleport | None), no audience)` to `Everyone` for a sound. A host landing's post-wait `playSound`, or one reached through `fire`, therefore reaches every client today. That holds even for a `levelLoad` tail, while the client refuses its own copy.
- `presentation_capture::after_apply` records light, fog and emitter overrides when `fire_context().origin` is `HostOnly`, or when the target is networked. Member steps in a host landing are captured for that reason (`plans/done/coop--networked-by-default` AC10). Restoring `EveryMachine` on a landing without separating capture from routing would stop capturing them.

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
  - Its Decision 11 and `scripting.md` §12 rest on "a presentation builder is never a sequence step, so no audience token reaches a `wait` landing". The brief keeps that rule for tokens and lets the literal `"everyone"` cross a wait.
  - Its Drain origins table lists `wait` landings as "host-only, no subject"; the brief replaces that row with the parked origin.
- `plans/done/E16--player-events`:
  - presentation in a `players().on` fire list routes to the event player;
  - install rejects a machine-local effect the source cannot forward, and a `fire` that reaches presentation;
  - an interruptible `wait` is rejected for this source.
  
  Its research hands body shape, system steps and post-wait routing to this brief.
- `drafts/ui-timed-reactions` builds on this brief: system steps, array bodies, the machine-local predicate "cell writes included", and the client warn-once naming `uiWait`. It argues for one scheduler whose instances carry their own clock.

## Where a post-wait presentation step should play
| Rule | Every-machine source (`levelLoad`, crossing) | Host-only source (trigger, teleport, player event) | Verdict |
|---|---|---|---|
| Today: host lands, origin `HostOnly(None)` | the host broadcasts sounds and keeps screen effects; clients refuse their copy | sounds everywhere; screen effects on the host, not the activator | Rejected: the wait changes who sees the effect |
| Host forwards over E16's lane | a client's own-state crossing tail is still lost | right audience | Rejected: a host own-state crossing (a low-health vignette) would shake every screen |
| A wait changes when, not where (chosen) | each machine lands its own machine-local steps; the host presents locally | the restored subject routes as before the wait | Chosen: one rule, no new message kind, E18's follow-up widened |

Client tails run machine-local steps only. Running every client-legal step after a wait would re-simulate delayed mover commands on clients, breaking E18's invariant that delayed consequences are host-authoritative and replicated, and it would let a client run `loadLevel` unguarded. Member light, fog and emitter steps need no client landing: the host's capture replicates them.

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
| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| S1 | Mixed drain | name-fired `[m.start(), updateState(a, 1), playSound(x), updateState(b, read(a))]` | the mover starts during the walk; `a` then `b` are written at the drain, `b` reading 1; the sound plays the same frame |
| S2 | Trigger-bound pre-wait write | Enter-bound `[updateState(alarm, 1), playSound(x), wait(800), playSound(y)]` | `alarm` in the firing tick; `x` at that frame's drain; `y` at landing |
| S3 | Every-machine tail on a client | `levelLoad` `[wait(500), npcs().update({ aggro: true }), playSound(x), m.start()]` | the client lands `x` only; the host lands all three, presenting `x` locally |
| S4 | Own-state crossing tail | crossing on `player.health` below 25 → `[vignette(…), wait(600), playSound(x)]` on host and client | each machine plays `x` for its own crossing; the host never sends it |
| S5 | Cancelled tail | Enter-bound `[wait(800, { interruptible: true }), screenShake(…), updateState(s, 1)]`; Exit before landing | neither runs |
| S6 | Emitter before and after a wait | mover-event `[playSound(x, { at: on.emitter }), wait(300), playSound(y)]`; and one with `at` after the wait | the first installs, with `x` positioned and `y` unpositioned; the second drops at install |
| S7 | Dispatch input after a wait | crossing `[wait(300), updateState(s, read(on.rising))]` | dropped at install, naming the reaction |
| S8 | `fire` target with a scoped step | `fire(r)` where `r = [updateState(s, read(on.rising))]` | dropped at install (V4b over steps) |
| S9 | Array lowering parity | `[a, [b, wait(5)], c]` vs `{ sequence: [a, b, ...wait(5), c] }` | byte-identical wire, TS and Luau |
| S10 | No double play | loopback `levelLoad` `[wait(500), playSound(x)]`, and `[wait(500), fire(r)]` with `r` playing `x` | each machine hears `x` once |
| S11 | Trigger tail audience | the client's player fires `[wait(300), flashScreen(…), playSound(y)]` | the flash on the client only; `y` on every machine once |

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
- Token audiences after a wait. They would follow once subject tokens survive a wait.
- The client `wait` warning demotion (sdk-addressing-model Doors) is absorbed: a client tail is now normal.
