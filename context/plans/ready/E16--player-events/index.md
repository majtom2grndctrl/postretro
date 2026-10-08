# E16--player-events

Brief · resumable · Epic 16 · reads: `context/lib/scripting.md` §5, §10.4, §11, §12 · `context/lib/networking.md` §Presentation events vs. replicated state, §Session-state ledger · `context/lib/development_guide.md` §Workspace · read at f29992e78

## Problem
A requested capability, raised by the owner and by E24's direction review. In co-op a mod cannot react to one player's state changing — "when a player's XP reaches 100, level that player up", "when a player's weapon overheats, burn that player". `onStateCrossing` runs on every machine over that machine's own view, so on the host it sees only the host's player; engine player slots hold only the local pawn's value; and E16 rejects crossings on mod per-owner slots outright. The host-only sources that do see every player, trigger events and impact policies, fire on spatial and hit edges, not state, and any sound or screen effect they fire plays on the host's machine. When done, a mod declares a per-player event over any per-player value; the host fires it once per player per edge, naming that player as command target and read owner; every owner-private engine player value is readable per player on the host; and the event's sounds and screen effects play on that player's own machine. E24's swim slots and E16's per-player XP level-up are the first consumers.

## Decisions
- **A host-only sibling source, not a crossing mode.** `players().on` registers and evaluates on host and single player only, never on a connected client — the trigger-event model (`plans/done/E18--trigger-event-params`). `onStateCrossing` keeps its meaning: every machine, its own view (`scripting.md` §10.4).
- **Spelled `players().on(edge, fire, options?)`** on the `players()` group, per `plans/done/sdk-addressing-model`'s `.on`-on-target rule (`t.on` precedent). It returns a pure descriptor effective only under `playerEvents`, iterates the group's player set, and takes no filter in v1. `getGameState().player` stays a tree of state refs and gains no methods.
- **Composition follows trigger bindings** (`scripting.md` §12). `playerEvents` on `ModManifest` and on `setupLevel`'s manifest compose as matching mod-global entries, then the level's, each in authored order; `options.levels` scopes a `ModManifest` entry as crossings' `levels` does, and a level entry carrying `levels` is rejected with a warning naming the level script while its siblings install, as a misplaced trigger form is. Two entries resolving to the same condition, edge and reaction bind once, with one warning naming both; conditions match by structural equality, so `a.and(b)` and `b.and(a)` are distinct.
- **Evaluated once per authoritative tick, after the tick settles, as a snapshot.** Evaluation follows the death sweep, so a condition sees this tick's damage and ammo; it is frame-rate independent and inside the determinism gate. Every (event, player) condition reads the settled tick before any fire applies, so outcomes do not depend on player or event order; a fire's writes, like a frame-end drain's, are seen next tick.
- **Condition plus edge word.** The condition is a Bool expression in the fluent algebra (`read(ref)`, `.ge`, `.and`, …); a non-Bool condition is rejected at install, naming the event. `becomes(cond)` fires false→true and `ceases(cond)` true→false. There is no threshold form; a threshold is `read(x).ge(t)`.
- **Inside the condition, a plain read of a per-player slot means the player being evaluated** — engine per-player slots and mod per-owner slots alike; global slots read as everywhere. The binding is lexical: the condition belongs to the source. A reaction is sourceless (`scripting.md` §12), so a plain per-player read in a reaction this source fires is an implicit owner, the wrong-owner bug `plans/done/E16--per-player-currency` forbids; install rejects it, naming the slot and pointing to `byPlayer(on.player)`.
- **First sight: an unobserved player counts as false.**
  - A player is unobserved at level install, without a pawn, during a disconnect hold, and while their pawn has no value for a slot the condition reads (no active weapon). Becoming unobserved fires nothing; at first observation `becomes` fires if the condition holds, and `ceases` never does.
  - Seat release drops the player's edge memory. A recompose keeps it for every (condition, edge) that survives, keyed by content, not position, so editing a fire list re-fires nothing; a new or changed condition starts every player unobserved.
  - No initial-state option: milestones use the guard idiom (`research.md` §Milestones). This diverges from crossings' arm-only rule, which drops a player who joins already underwater.
- **`on.player` is the event's player, as command target and read owner.** It is a subject token like `on.activators`, carrying its verbs (`scripting.md` §12), resolving to that player's pawn or seat, and legal only before any `wait`. `ref.byPlayer(on.player)` reads that player's value in the fired reaction; `Seat` never reaches the authoring surface (E16). Both are legal only in reactions this source fires; under any other source, install rejects that subscription, naming the reaction and the source (E18 sentinel rule).
- **Install rejections cost one subscription, not the reaction.** When any reaction at an address in a player event's fire list breaks this brief's rules, the event drops that address, with an error naming the address, the reaction and the rule; the address still runs under every other source. A mixed address is dropped whole, because this source has no runtime skip for a misrouted effect. E18 V4a's reaction-wide drop still governs what a `wait` loses.
- **`updateState` accepts fluent values** — any `read(…)` expression, `byPlayer(on.player)` reads included — in TypeScript and Luau. It is the consumer of in-reaction per-player reads.
- **Owner-private engine player slots become per-player.** The engine-state catalog marks them, and each has one host-side lookup from pawn to value that reads the pawn's components. Owner-private replication, condition reads and `byPlayer` reads all share that lookup, so no stored per-player copy can drift. Outside this source a plain read keeps meaning the local player — HUD binds, `bindState`, local crossings, impact-policy ambient reads — and `byPlayer` on these refs takes `on.player` or `impact.source`.
- **Crossings are unchanged.** `onStateCrossing` on a mod per-owner slot stays rejected at bind (`plans/done/E16--per-player-currency` Decisions): accepting it would silently watch only the host's player and would fix crossing semantics ahead of the deferred redesign.
- **Own-state presentation stays client-local; `players().on` presentation is for host decisions.** An effect reflecting a player's own replicated state — a low-health vignette, an ammo warning, a swim splash — is a local `onStateCrossing` on that player's machine, with no host round trip and no loss (as `coop-trigger-screen-effects` and `movement--state-transition-feel` treat own-state feedback). `players().on` carries presentation that follows a host decision: a level-up fanfare, a scald on overheat.
- **Presentation plays on the target player's own machine.** Each fire carries an internal target player, distinct from `on.player`, that routes only the presentation reactions (`playSound`, `rumble`, `flashScreen`, `vignette`, `screenShake`) in its fire list: local for the host's or single player's own player, else a new unreliable Presentation-channel message to that client alone (`networking.md` §Presentation events vs. replicated state). The receiving machine's accommodations (E23 flash limiter, reduce motion) apply.
- **A machine-local effect this source cannot forward is rejected.** A UI-stack verb (`showDialog`, `openMenu`, `closeDialog`), a text edit or a `setState` on a non-replicated slot in a reaction this source fires would land on the host's screen, not the event's player's, so install rejects it, naming the reaction. Widening forwarding later lifts the rejection without breaking content.
- **Context-free routes are checked at install.** A `fire` step and a primitive's completion follow-up (`onComplete`) dispatch with no context. This extends E18's V4b to `onComplete`, which E18 warn-skips at runtime (`plans/done/E18--timed-reaction-steps`). A `players().on` reaction from which either route, at any depth, reaches presentation, a machine-local effect or a plain per-player read is rejected at install, naming both reactions.
- **No emitter.** This source publishes no `emitter`; `playSound(…, { at: on.emitter })` in its reaction warn-skips as for other emitterless sources (`scripting.md` §12), and its sounds play unpositioned on the player's machine.
- **Non-goals.**
  - Whose screen a *trigger*-fired effect belongs to: `coop-trigger-screen-effects` owns that policy and reuses this delivery path.
  - Author-chosen routing for `players().on` presentation, such as broadcasting to every player: it reaches the subject only; a shared slot plus local crossings broadcasts today.
  - Redesigning `onStateCrossing`. The owner suspects it predates a netplay-aware design; `research.md` §Symptom watch records evidence as it appears.
  - A both-edges word (`changes`): no consumer uses both edges, and it is addable later without breaking content.
  - Per-player named engine events (`playerDied` carrying its player): those are event edges, not state edges, and no first consumer needs one; `research.md` §Symptom watch records `players().on` as their likely home.
  - Per-owner `accumulate` and E16's environmental-damage policy, which a per-player air meter needs: this brief supplies the edge and `on.player.damage`, not the meter.
  - E24's swim slots themselves.
  - Multi-effect reaction bodies and system reactions as sequence steps: `reaction-body-composition` owns both. This brief ships on today's body shapes, one effect per reaction, and does not wait on it.

### Scripting surface
```ts
// leveling.ts — a store module; XP is the dev mod's per-owner `progression.xp`, credited per kill
import { defineStore } from "postretro";
export const leveling = defineStore("leveling", {
  level: { type: "number", default: 1, perOwner: true, network: "ownerPrivate" },
  lastLevelUpXp: { type: "number", default: 0, network: "shared" },
});

// level script
import { players, becomes, ceases, read, defineReaction, getGameState } from "postretro";
import type { PlayerEventParams } from "postretro";
import { playSound, flashScreen, vignette, updateState, onStateCrossing } from "postretro/ui";
import { progression } from "./combat-lifecycle";
import { leveling } from "./leveling";
const player = getGameState().player;

// One effect per reaction; a source's fire list runs several.
const levelUp = defineReaction("leveling.levelUp", (on: PlayerEventParams) => on.player.addSlot(leveling.level, 1));
const recordLevelUp = defineReaction("leveling.recordLevelUp", (on: PlayerEventParams) =>
  updateState(leveling.lastLevelUpXp, read(progression.xp.byPlayer(on.player))));
const fanfare = defineReaction("leveling.fanfare", playSound("level_up"));   // on that player's machine
const goldFlash = defineReaction("leveling.goldFlash", flashScreen([1, 0.9, 0.3, 0.4], 300));
const scald = defineReaction("heat.scald", (on: PlayerEventParams) => on.player.damage(5));
const scaldHiss = defineReaction("heat.scaldHiss", playSound("scald"));
const cooled = defineReaction("heat.cooled", playSound("vent_hiss"));
const bleeding = defineReaction("health.bleeding", vignette(0.6, 800, [0.8, 0, 0]));

export function setupLevel() {
  return {
    reactions: [levelUp, recordLevelUp, fanfare, goldFlash, scald, scaldHiss, cooled, bleeding],
    playerEvents: [   // host decisions: run on the host, once per player
      players().on(becomes(read(progression.xp).ge(100).and(read(leveling.level).lt(2))),
        [levelUp, recordLevelUp, fanfare, goldFlash]),
      players().on(becomes(read(player.overheated)), [scald, scaldHiss]),  // overheating burns the wielder
      players().on(ceases(read(player.overheated)), [cooled]),
    ],
    crossings: [      // own-state feedback: each machine, its own player
      onStateCrossing(player.health, { below: 25 }, [bleeding]),
    ],
  };
}
```
The start script registers `leveling` in `defineMod({ stores })`, as `content/dev/start-script.ts` registers `closetStore`; the Luau twin re-declares the store inline, as the Luau closet twin does. `PlayerEventParams` carries `player`. Luau mirrors it with colon calls — `Postretro.players():on(…)`, `on.player:damage(5)` — and spells `.and` as `["and"]`.

## Acceptance
### Automated
**Per-player firing**
- [ ] Two players on the host: the guarded XP milestone fires once for the player who crosses, and `on.player.addSlot(…)` credits only that player. The other player crossing later fires once more, for them.
- [ ] Two players crossing on the same tick fire twice, once per player, in the player group's resolution order — the order a `players()` command reaches them — and in the same order on every run (pin P1).
- [ ] Two players whose conditions both become true on one tick both fire, even when the first fire's consequence would make the second condition false; the second evaluation sees that consequence on the next tick (pin P2).
- [ ] `becomes` fires on false→true only, `ceases` on true→false only.
- [ ] A condition over a store slot written by an in-tick trigger fires on that tick; over a slot written by a frame-end reaction drain, on the next.
- [ ] A non-Bool condition is rejected at install, naming the event; a Bool one beside it installs.
- [ ] A condition over a player's health sees every hit that lands on its tick on that tick — a remote client's authorized hit as well as a host hit — and sees a global accumulated slot's advance on the tick the row's pin states (pins P4, P5).
- [ ] In a frame that runs two ticks, a condition true on the first and false on the second fires `becomes` and then `ceases`, both drained that frame in tick order; a value that crosses and returns within one tick fires nothing (pin P3).
- [ ] A trigger fire and a player-event fire on one tick both drain in that tick's frame, each in its own authored order, in the inter-source order the plan of record pins; a test fails if the two batches swap (pin P11).
- [ ] With no player pawn bound, a player event fires nothing and logs nothing above debug (pin P15).
- [ ] A mod-global player event scoped by `levels` fires only in matching levels; a level's own player events fire only in that level.
- [ ] A mod-global and a level player event resolving to the same condition, edge and reaction fire it once, with one warning naming both; distinct events on one tick fire mod-global first, then the level's, each in authored order (pin P16).
- [ ] A `players().on` descriptor that is built but not returned under `playerEvents` registers nothing and fires nothing.

**First sight**
- [ ] A player for whom the condition holds at level install fires `becomes` on the first tick; one for whom it is false does not fire `ceases`.
- [ ] A remote player joining while the condition holds fires `becomes` on their first observed tick; one joining while it is false fires nothing.
- [ ] A disconnect hold fires nothing. Reclaiming within the hold while the condition holds fires `becomes` again; reclaiming while it is false fires nothing.
- [ ] After seat release no edge memory remains for that player: edge memory holds one entry per live player per event, and a player admitted on a new seat before the next tick fires `becomes` once if their condition holds (pin P8).
- [ ] A guarded milestone does not re-fire across a level transition or a reclaim; the same milestone without its guard re-fires at each.
- [ ] A remote player whose pawn is bound but who has sent no input yet is observed: a condition holding for them fires `becomes` on the first tick after binding, not at their first input (pin P7).
- [ ] A player at zero health stays observed: a condition over their health fires `becomes` once at death and no first-sight fire follows; damage a fire applies after the tick's death sweep reports the death on the next tick, once (pins P19, P20).
- [ ] A hot reload that keeps an event's condition and edge fires nothing for players whose condition already held, even when its fire list changed; a changed condition starts every player unobserved (pin P10).
- [ ] A player event whose fire list names an address where one reaction plays a sound and another holds a plain per-player read drops that whole address from the event at install, naming the address, reaction and rule; the same address still fires under a crossing.
- [ ] A level script returning a `players().on` entry with `levels` gets a warning naming the script and that entry does not install; its sibling entries do.
- [ ] A player whose pawn has no value for a slot the condition reads (no active weapon) is unobserved for that event: losing the value fires nothing, and regaining it while the condition holds fires `becomes` once (pin P18).

**Reads and targets**
- [ ] On the host, with two players at different health, a condition over `player.health` fires for exactly the player whose own health crossed, and `read(player.health.byPlayer(on.player))` in the fired reaction yields that player's value, not the host's.
- [ ] For every engine slot the catalog marks owner-private, derived from the catalog itself, two pawns with different component state read different per-player values on the host and in their owner-private snapshots; a newly marked slot with no per-player source fails the test. The existing per-client snapshot tests pass unchanged.
- [ ] A plain read of an engine player slot still means the local player in a HUD bind, `bindState`, a local `onStateCrossing` and an impact policy.
- [ ] A plain `read(player.health)` or plain mod per-owner read in a reaction this source fires is rejected at install, naming the slot and pointing to `byPlayer(on.player)`; the same read inside the event's condition installs and binds to the evaluated player.
- [ ] A reaction holding a plain per-player read, listed in a `players().on` fire list and a local crossing, loses its player-event subscription with an error naming both and still fires under the crossing, reading that machine's own player.
- [ ] In TypeScript and Luau, `updateState` with a `read(…)` expression over `byPlayer(on.player)` in a fired reaction writes the event player's value, not the host's; a literal value writes as before.
- [ ] `byPlayer(impact.source)` on an engine player slot in an impact policy reads the source player's value.
- [ ] A reaction using `on.player` as a target or a `byPlayer` owner, subscribed to `levelLoad`, a crossing or a trigger event, is rejected for that source at install, naming the reaction and the source; the same reaction under `players().on` installs.
- [ ] `on.player` resolving to a player whose pawn despawned before the drain warn-skips the command and leaves sibling commands applying.
- [ ] A `byPlayer(on.player)` engine-slot read in a fired reaction yields the value the row's pin states (fire tick or drain); when that player's pawn is gone before the drain, the reaction warn-skips rather than reading a default, and its presentation still reaches the player (pins P12, P17).
- [ ] A reaction with an `on.player` step after a `wait` is dropped at install, reaction-wide, with an error naming it (E18 V4a); the same step before the `wait` installs and lands on the event's player.

**Roles**
- [ ] A connected client neither registers nor evaluates `players().on`, and fires nothing from it.
- [ ] `onStateCrossing` on a mod per-owner slot is still rejected at bind on every role, naming the slot (regression guard); a local crossing on `player.health` still fires on each machine for its own player.

**Presentation**
- [ ] Loopback harness, host plus two clients: a `flashScreen` from an event for client A starts A's screen flash through A's frame drain — not the host's, not client B's. The host's own player's event presents on the host only; in single player it presents locally.
- [ ] Each presentation command round-trips the new message kind with identical fields; a dropped message presents nothing and breaks nothing.
- [ ] The existing presentation payloads encode to the same bytes as before the new kind; a peer on the previous wire version is refused at the version gate before any decode; the wire version is exactly one above main's (pin P14).
- [ ] A forwarded presentation command carrying a non-finite number is dropped at client intake with a warning and presents nothing; finite commands beside it still present.
- [ ] A forwarded presentation command reaching a client while it is held, demoted or between levels presents nothing; a fire on the last tick before a level transition lands none of its commands in the next level (pins P9, P13).
- [ ] In co-op, a trigger event's `flashScreen` and `playSound` still present on the host only, unchanged, while the same reactions in a `players().on` fire list present on the event's player.
- [ ] A forwarded `flashScreen`, `vignette` or `screenShake` leaves the client's screen-effect state identical to the same reaction fired locally there, so the client's flash limiter and reduce-motion apply unchanged.
- [ ] `playSound` with `at: on.emitter` in a `players().on` reaction warn-skips; without `at` it plays.
- [ ] A `showDialog` reaction, or an `updateState` on a non-replicated slot, in a `players().on` fire list is rejected from the player event at install with an error naming it; an `updateState` on a shared slot in the same fire list installs and lands on the host.
- [ ] One classification, exhaustive over every system reaction kind, sorts each as forwardable presentation, machine-local, or host consequence; a new kind fails to build until classified. A `players().on` fire list holding any machine-local kind rejects it at install, naming it.
- [ ] A fire list holding a consequence and two presentation reactions runs all three, each dispatch path in listed order (`scripting.md` §12 trigger ordering): the credit applies on the event's tick, and the sound and flash present on the event's player only.
- [ ] A `players().on` reaction whose `fire` step or completion follow-up reaches a presentation reaction, a machine-local effect or a plain per-player read, directly or through further `fire` steps, before or after a `wait`, is rejected from the player event at install with an error naming both; the same presentation reaction listed directly in the event's fire list installs and presents on the event's player only.
- [ ] A `players().on` reaction whose `fire` step reaches a reaction with no presentation, no machine-local effect and no plain per-player read installs and runs.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` script, and its TypeScript and Luau twins produce byte-identical wire data.
- [ ] Grep gate: no seat identifier appears in the generated SDK typedefs or either runtime's scripting surface, and the player state tree exposes no methods.
- [ ] networking.md §Presentation events vs. replicated state and §Channel model describe the player-addressed presentation command alongside damage numbers. They no longer call the presentation layer world-anchored only, and they give the damage bearing's slot placement as a continuous fact a HUD binds.

### Manual
- [ ] Two-client co-op playtest: the level-up sound and flash and the scald sound appear only on the affected player's machine; the low-health vignette (a local crossing) appears on the bleeding player's machine with no perceptible delay after the hit.
- [ ] With the flash limiter on and reduce motion set on a client, a forwarded flash and shake are limited and scaled there.

## Path
- Evaluation seam: the tail of `simulate_tick_with_presentation_aim`, after `run_death_sweep`. The player set is the group's (`group_resolution`), not the tick's `AuthoritativePlayer` list, which omits a pawn whose client has sent no input; `canonical_player_pawns` picks one pawn per player. Evaluate every condition before applying any fire's in-tick commands.
- Firing: reuse the in-tick trigger command machinery — a `TriggerFireContext` whose activator is the player's pawn, `BoundTarget::Activators`, residuals carrying `PlayerId` to the frame-end drain. The rival extends `NamedEventDispatchContext`/`SystemCommandFireContext` with a player field; weigh it against how presentation steps reach the drain. Presentation routing must not key on the trigger activator, or every trigger's presentation moves with it.
- Condition scope: a player-seeded scope reusing `EntityScope`'s owned-read pieces (`seed_owner_seat_from_registry`, `resolve_owner_input`, `read_owned_store`), routing plain per-owner names to the seeded seat and engine per-player names to the lookup. `DispatchScope` opts into `resolve_owned_input` for the `@player` owner only.
- Engine lookup: lift `descriptor_health_for_pawn`, the weapon slot projection and the per-owner step of `owner_private_source_value` (`netcode/src/state_slots.rs`) below `netcode`, so the sim-side scope and replication both reach it (`development_guide.md` §Workspace). The owned-read scopes live in `scripting-core`, which cannot reach `sim` or `combat-model`: inject the lookup into them as a callback, or place it in `scripting-core`, since it reads only `entities` components.
- Rejections: strip the subscription as the crossing sentinel rule does (`startup/lifecycle.rs`), not by emptying the body as V4a does (`reaction_validation.rs`). The `fire` walk extends to `onComplete` addresses. One predicate classifies machine-local effects; `reaction-body-composition` needs the same one, so whichever lands first builds it. On recompose, where the crossing detector reinitializes, player-event edge memory carries over keyed by descriptor content.
- `updateState`: lower a fluent value to IR as impact builders do (`sdk/lib/util/expression_refs.ts`), in `sdk/lib/ui/reactions.ts` and `reactions.luau`; the `@player` owner binds through `DispatchScope`, where `RuntimeValue` binds through `StoreScope` today.
- Presentation: a new `ServerPresentationPayload` variant, sent with `NetServer::send_presentation`; client intake beside `ingest_client_presentation_messages` pushes onto the client's system command queue. Confirm that queue drains on the connected-client frame path.
- SDK: `read()` widens to engine per-player refs; `byPlayer` is generated for them (non-enumerable, per `scripting-state-convergence`); `@player` joins the `@activators` parsers in both runtimes; `defineReaction` gains named and unnamed `PlayerEventParams` tracer overloads; store-ref `byPlayer` accepts `on.player`, which today lowers to `@invalid` for any owner but `impact.source`.
- Docs: `scripting.md` teaches the own-state vs host-decision split and that the event player's presentation goes in the fire list, not behind `fire`; `reaction-body-composition`'s inline system steps will route without `fire`.
- First slice: one engine slot (`player.health`) through the lookup, a `becomes` condition on two loopback players, a `on.player.damage(…)` landing on the right pawn. This falsifies the scope routing and the lookup placement, the riskiest pieces.
- Split first, behavior-preserving: `main.rs`, `netcode/src/state_slots.rs`, `reaction_dispatch.rs`, `system_reactions.rs`, `scopes.rs`, `engine_state_catalog.rs`, `state_crossings.rs`, `data_script.ts` are all past ~800 lines; split only the ones this work extends.

## Open questions
- Where a player-event fire is ordered against same-tick trigger fires in the frame-end drain — **delegated**: the executor pins it and reports it in the plan of record.
- Where the shared engine lookup lands — `sim`, `combat-model` with a callback into the `scripting-core` scopes, or `scripting-core` itself — **delegated**.

## Boundary inventory
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Source builder | player-event descriptor | manifest JSON `playerEvents[]` | `players().on(edge, fire, options?)` | `players():on(edge, fire, options?)` | n/a |
| Edge words | edge enum `Becomes \| Ceases` | `"becomes"`, `"ceases"` | `becomes(cond)`, `ceases(cond)` | same | n/a |
| Params | dispatch inputs | `@player` (target and owner token; `target: "@player"` on primitives) | `PlayerEventParams { player }`; `defineReaction` tracer overload | same | n/a |
| Manifest key | `LevelManifest`/`ModManifest` field | `playerEvents` | `playerEvents` | `playerEvents` | n/a |
| `updateState` value | `setState` IR value | fluent IR expression in `args.value` | `updateState(ref, T \| RuntimeValue \| NumberRef \| BoolRef)` | same | n/a |
| Per-player engine slots | catalog per-player flag + lookup | owner-private (unchanged) | `player.health`, `player.maxHealth`, `player.ammo`, `player.ammoReserve`, `player.heat`, `player.overheatAt`, `player.overheated`, `player.cell`, `player.cellCapacity`, `player.reloadActive`, `player.reloadProgress`, `player.weaponCooldownMs` | same | n/a |
| Presented command | system reaction command | new `ServerPresentationPayload` variant, appended; `WIRE_VERSION` bump | n/a | n/a | n/a |

## Wire format
The new Presentation payload variant mirrors `SystemReactionCommand`'s presentation arms as a net-crate wire type — the engine-to-wire conversion lives in `netcode`, so no engine type enters `net`. One variant carrying a nested command enum: `PlaySound { sound: String, bus: Option<String> }`, `Rumble { strong: f32, weak: Option<f32>, duration_ms: f32 }`, `FlashScreen { color: [f32; 4], duration_ms: f32 }`, `Vignette { color: Option<[f32; 3]>, strength: f32, duration_ms: f32 }`, `ScreenShake { amplitude: f32, duration_ms: f32, frequency: Option<f32> }`. Bitcode, appended after `OverlayFact`; non-finite floats are dropped at client intake as `Spawn` anchors are. The client turns each into the same local system command a local reaction enqueues. `WIRE_VERSION` advances by one.
