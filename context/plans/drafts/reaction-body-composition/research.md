# reaction-body-composition — research

Facts read at f29992e78. These inform the brief without deciding it.

## Body and step shapes today
- A reaction body is exactly one of `progress`, one primitive, or `sequence`. `named_reaction_from_js` and its Luau twin discriminate by key presence; anything else is `UnknownShape`.
- An array body does not exist. In TS it is a type error (`ReactionBody` has no array arm). At runtime, `defineReaction` spreads the body into `{ name, ...body }`, so an array becomes numeric keys; the Luau `defineReaction` copies only known keys and yields `{ name }`. Both reach the parser as `UnknownShape`. A level script's drain warns "reactions[i] is malformed and was skipped"; a mod manifest's drain propagates the error and rejects the manifest.
- The tracer form `(on) => body` is typed `ReactionTracer<S> = (params: S) => ReactionBody`. It runs once at declaration and its return value takes the same path as a plain body, so widening `ReactionBody` widens both forms.
- `sequence_steps_from_js` (and `sequence_steps_from_lua`) build a target from `kind` (group), `target` (subject token) or `id`. With none of them, they fall through to a required numeric `id`, so `{ primitive: "playSound", args }` fails to parse. The TS `SequenceStep` union lists member, group, subject-token, `wait` and `fire` shapes only.
- `SequenceTarget` is `Entity | Group | Activators | FiredTrigger | Wait | Fire`.
- System builders (`playSound`, `rumble`, `flashScreen`, `vignette`, `screenShake`, `showDialog`, `openMenu`, `closeDialog`, `loadLevel`, `restartLevel`, `returnToFrontend`, `setSentiment`, `adjustSentiment`, `updateState`, `appendText`, `backspaceText`, `clearText`) return `{ primitive, args }` typed `PrimitiveReactionDescriptor`. That type also admits `kind`, `tag`, `target` and `onComplete`.
- Group and subject-token verbs return one descriptor, legal unspread as a step. Member handle methods, `wait()` and `fire()` return step arrays the author must spread.
- The Luau bridge emits an empty table as `{}`; the build-time light pass already accepts `sequence: {}` as empty.

## Dispatch today
- `dispatch_primitive` routes a tagless, kindless, targetless primitive to `dispatch_system_primitive` → `SystemReactionRegistry::dispatch` → `ScriptCtx::system_commands`. `dispatch_sequence` takes no system registry.
- `App::dispatch_system_commands` (`main.rs`) drains the queue once per frame with no role gate on presentation arms. `setState` on a client writes the client's slot table; sentiment helpers self-gate; `addSlot` returns early on clients; `loadLevel` reaches `enqueue_level_request`, which itself has no role gate (not traced further).
- An IR-valued `setState` binds at install in `SystemReactionIrBindings::rebuild`, which walks only `Primitive` bodies. The drain looks a queued command up by `(slot, value)`; an unbound IR command warns "was not bound at level install".
- Trigger partition (`partition_direct_reaction`): before the first `wait`, `setState` and `addSlot` are consequential (in-tick); `loadLevel`/`restartLevel`/`returnToFrontend` are lifecycle; every other primitive is presentation and drains as residual. `bind_sequence_step` refuses `setState` outright; `bind_primitive` accepts a system-targeted one.
- `SystemCommandFireContext` carries source, dispatch values and emitter while a named fire dispatches. A system step walked inside that dispatch inherits it with no new plumbing.
- `ReactionScheduler` is host/single-player only (`enabled` latch). Its `InstanceKey` origin is `Option<(EntityId, PlayerId)>`, populated for trigger-fired instances. The fire's activator set and trigger do not reach the tail's dispatch, which is why V4a drops subject tokens after a wait, but the origin player is retained.
- Install Pass A (`reaction_validation.rs`) already drops a reaction whose post-wait step's raw args carry `at: on.emitter` (V4a). V4b reads `SystemReactionIrBindings` only, so a `fire` target is checked only when it is a `Primitive` `setState`.
- The build-time light pass skips a whole sequence when any step fails `runtime_sequence_step_shape_is_valid` (sdk-addressing-model A12 precedent).

## Is post-wait presentation host-only in co-op? Yes, and it is documented
- Every-machine sources (`levelLoad`, crossings, UI presses, named gameplay events): pre-wait steps run on each machine; the client scheduler refuses enrollment and warns once per reaction (`plans/done/E18--timed-reaction-steps` O24, O43). A client-local crossing reaction with a wait loses its tail on the client; the host lands the host's own copy.
- Host-only sources (trigger events): the client never fires the reaction, so pre- and post-wait presentation both play on the host only.
- Already recorded as a limit or defect:
  - `plans/done/E18--trigger-event-fanout` Out of scope: reliable co-op delivery of `playSound`/`flashScreen` stings is host-local in v1; the shared-slot atmosphere channel is the co-op idiom.
  - `plans/done/E18--timed-reaction-steps` Out of scope (resumed presentation is host-local) and Owner decisions (the client-local-wait loss is "a defect the spec ships"; a client-side scheduler restricted to presentation-only tails is the named additive follow-up).
  - `drafts/coop-trigger-screen-effects` calls trigger-fired screen effects never reaching a client a defect and leaves whose screen they belong to open.
- Lights and fog stay off the wire (`netcode/src/replication.rs`), so a post-wait light or fog member step is host-only visually too. Outside this brief.
- E18's ordering table already writes system steps into sequences (O13 `[playSound, wait(17), screenFlash]`, O20 `[wait(1000), loadLevel("next")]`), though the parser has never accepted them.

## Where a post-wait presentation step should play
| Rule | Every-machine source (`levelLoad`, crossing) | Host-only source (trigger) | Verdict |
|---|---|---|---|
| Host only (today; a step is sugar for `fire(r)`) | host only; clients lose it | host only | Rejected: the wait changes who sees the effect |
| Host broadcasts to every client over Presentation | host's copy reaches everyone; a client's own crossing tail is still lost | everyone | Rejected: a host own-state crossing (low-health vignette) would shake every screen |
| Wait changes when, not where (chosen) | each machine lands its own presentation | the pre-wait audience | Chosen: one rule, no new message kind, matches E18's named follow-up |

Client tails run presentation steps only. Running every client-legal step after a wait (member steps, `fire`, `setState`) would make the rule total, but it re-sims delayed mover commands on clients, breaking E18's invariant that delayed consequential effects are host-authoritative and replicated. It would also let a client `loadLevel` run unguarded. Narrowing to presentation keeps that invariant and covers the effects players notice.

## Subject routing across a wait
- E16 routes a player-event reaction's presentation to `on.player`'s machine through a new Presentation-channel message kind. Weighed as the mechanism for every post-wait presentation step: it fits host-only sources whose audience is one player, and it is wrong for every-machine sources, which land locally.
- The scheduler instance's retained origin player is the seam: a host-landed tail whose pre-wait audience was one player can be forwarded to that player through E16's message kind, with no author-visible token surviving the wait. A trigger-fired instance already carries its player; a player-event instance would carry its player the same way.
- Today no host-only source addresses presentation to a player, so the seam has no consumer until E16 or `coop-trigger-screen-effects` lands.

## `fire(r)` after this brief
| Need | Inline system step | `fire(r)` |
|---|---|---|
| One effect in a beat | yes | wrapper reaction; redundant |
| `at: on.emitter` before a wait | resolves | install drops the reaction |
| E16 subject routing before a wait | inherits the fire | lost; `fire` dispatches with no fire context |
| Order against neighbouring presentation | authored order | dispatched after the body's presentation (E18 O37b) |
| Reuse across reactions, brush KVPs, Luau/TS agreement | no | yes |
| On a client after a wait | presentation lands | host only |
`fire` stays the reuse idiom; nothing deprecates it.

## Array bodies
- Lowering an array to `{ sequence: [...] }` in the SDK keeps the wire, the parser, the trigger partition, the scheduler and the light pass unchanged, so it is not a second dispatch mechanism. It depends on system steps: without them, `[on.activators.grantHealth(25), playSound("medkit")]` has no valid lowering.
- Rival: an engine-side array body (`ReactionDescriptor::Effects`). Rejected: a third body shape every consumer must learn (partition, scheduler funnel, validators, light pass, hot-reload recompose), for semantics identical to a sequence.
- Rival: wait-free arrays only. Rejected: a second rule for authors to learn, and the restriction protects nothing, because the lowering is identical.
- Nested arrays: member handle methods, `wait()` and `fire()` return arrays, so `[door[0].start(), playSound("x")]` nests by construction. Flattening at author time makes spreads optional. Applying the same flatten to `sequence` lists keeps one rule; already-flat content lowers byte-identically.
- Empty arrays: `getMapEntities` returns `[]` for no match, so `lights.flatMap(…)` can be empty in valid content. An empty body is an inert sequence, as a dropped reaction's body already is.
- Entries that are not steps (`progress`, a nested `{ sequence }`, a `defineReaction` handle, a primitive carrying `onComplete`, a raw tag-keyed descriptor) throw at author time naming `defineReaction`. A handle in a list most likely meant `fire(r)`; the error says so.
- Luau cannot tell an empty array from an empty map. An empty table body has no other meaning, so it lowers to an empty sequence.

## Precedents
Recalled, not fetched.
- Unreal Level Sequences and Unity Timeline put audio, camera shake and events on the same timeline as actor tracks; neither forces a sound onto a separate asset.
- Source I/O chains outputs with per-output delays (`OnTrigger → ambient_generic PlaySound, delay 0.3`), each with its own target, including `!activator`. Sound and entity outputs share one list.
- Roblox and Godot tweens/sequencers chain callbacks and waits in one builder.

## Ordering pins
| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| S1 | Mixed drain | name-fired `[m.start(), updateState(a, 1), playSound(x), updateState(b, read(a))]` | mover starts during the walk; `a` then `b` written at the system drain in authored order, `b` reading 1; sound plays the same frame |
| S2 | Trigger-bound pre-wait write | Enter-bound `[updateState(alarm, 1), playSound(x), wait(800), playSound(y)]` | `alarm` written in the firing tick; `x` at that frame's drain; `y` at landing |
| S3 | Every-machine tail on a client | client `levelLoad` `[wait(500), npcs().update({ aggro: true }), playSound(x), m.start()]` | client lands `x` only; host lands all three |
| S4 | Own-state crossing tail | crossing on `player.health` below 25 → `[vignette(…), wait(600), playSound(x)]` on host and client | each machine plays `x` for its own crossing; the host never sends it to the client |
| S5 | Cancelled tail | Enter-bound `[wait(800, { interruptible: true }), screenShake(…), updateState(s, 1)]`; Exit before landing | neither runs |
| S6 | Emitter before and after a wait | mover-event reaction `[playSound(x, { at: on.emitter }), wait(300), playSound(y)]`; and one with `at` after the wait | first installs, `x` positioned, `y` unpositioned; second drops at install |
| S7 | Dispatch input after a wait | crossing reaction `[wait(300), updateState(s, read(on.rising))]` | dropped at install, naming the reaction |
| S8 | `fire` target with a scoped step | `fire(r)` where `r = [updateState(s, read(on.rising))]` | dropped at install (V4b over steps) |
| S9 | Array lowering parity | `defineReaction(name, [a, [b, wait(5)], c])` vs `{ sequence: [a, b, ...wait(5), c] }` | byte-identical wire, TS and Luau |

## Consumers
- `content/dev/scripts/closet-reveal.{ts,luau}`: the `raiseAlarm` and `resupply` wrapper reactions become inline steps; the Scripting surface example replaces it.
- `E16--player-events`: ships on today's shape, one effect per reaction with several reactions in a fire list. First consumer that can later collapse to array bodies, and the first source whose post-wait presentation would route to one player.
- `coop-trigger-screen-effects`: decides trigger-fired audience; this brief's rule makes its answer cover post-wait steps for free.
- Docs: `docs/scripting-reference.md` reaction bodies and sequences; `context/lib/scripting.md` §10.4 and §12.

## Doors
- Post-wait member light and fog steps on clients: lights and fog are off the wire, so a client would need its own landing of member presentation steps. Same client tails, wider step class.
- Subject tokens after a wait: a trigger-fired instance already retains one activator and its trigger, so `on.activators` after a wait could resolve to that one player. It would change V4a and the meaning of "activators".
- Client `wait` warning demotion (sdk-addressing-model Doors) is absorbed: a client tail is now normal.
