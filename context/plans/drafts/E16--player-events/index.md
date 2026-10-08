# E16--player-events

Brief · resumable · Epic 16 · reads: `context/lib/scripting.md` §5, §10.4, §11, §12 · `context/lib/networking.md` §Presentation events vs. replicated state, §Session-state ledger · `context/lib/development_guide.md` §Workspace · read at f29992e78

## Problem
A requested capability, raised by the owner and by E24's direction review. In co-op a mod cannot react to one player's state changing — "when a player's XP reaches 100, level that player up", "when a submerged player runs out of air, damage that player". `onStateCrossing` runs on every machine over that machine's own view, so on the host it sees only the host's player; engine player slots hold only the local pawn's value; and E16 rejects crossings on mod per-owner slots outright. The host-only sources that do see every player, trigger events and impact policies, fire on spatial and hit edges, not state, and any sound or screen effect they fire plays on the host's machine. When done, a mod declares a per-player event over any per-player value; the host fires it once per player per edge, naming that player as command target and read owner; every owner-private engine player value is readable per player on the host; and the event's sounds and screen effects play on that player's own machine. E24's swim slots and E16's per-player XP level-up are the first consumers.

## Decisions
- **A host-only sibling source, not a crossing mode.** `players().on` registers and evaluates on host and single player only, never on a connected client — the trigger-event model (`plans/done/E18--trigger-event-params`). `onStateCrossing` keeps its meaning: every machine, its own view (`scripting.md` §10.4). A mode flag on crossings was rejected: one name would cover two machine sets, and forgetting the flag silently watches the host's player only.
- **Spelled `players().on(edge, fire, options?)`, on the `players()` group `plans/done/sdk-addressing-model` shipped.** That brief's rule spells a source that fires once per member or group member as `.on(…)` on its target, publishing a subject token named for its kind; trigger members already do (`t.on("enter", fire)`). `.on` returns a pure descriptor that takes effect only when returned under `playerEvents`, as `t.on` does under `triggerEvents`. Its player set is the group's: seat-bound pawns, skipping a seat in a disconnect hold, and the local pawn in single player. It takes no filter in v1. Luau spells it `Postretro.players():on(…)`. `getGameState().player` stays a tree of state refs and gains no methods.
- **Evaluated once per authoritative tick, after the tick settles** — after the death sweep, so a condition sees this tick's damage and ammo. Frame-rate independent and inside the determinism gate. A slot written by a frame-end reaction drain is seen the following tick.
- **Condition plus edge word.** The condition is a Bool expression in the fluent algebra (`read(ref)`, `.ge`, `.and`, …). `becomes(cond)` fires false→true, `ceases(cond)` true→false, `changes(cond)` both, publishing `on.rising`. A non-Bool condition is rejected at install, naming the event. There is no threshold form; a threshold is `read(x).ge(t)`.
- **Inside the condition, a plain read of a per-player slot means the player being evaluated** — engine per-player slots and mod per-owner slots alike; global slots read as everywhere. The condition belongs to the source, so this binding is lexical. In a reaction this source fires, directly or through `fire(r)`, a plain per-player read is rejected at install, naming the slot and pointing to `byPlayer(on.player)`: a reaction is sourceless (`scripting.md` §12), and an implicit owner there is the wrong-owner bug E16's explicit-owner rule forbids (`plans/done/E16--per-player-currency` Decisions). Rival: `research.md` §After sdk-addressing-model.
- **First sight: an unobserved player counts as false.** Every player starts unobserved at level install, and a player without a pawn is unobserved. `becomes` fires at first observation if the condition holds — at install, on join, on reclaim. `ceases` never fires from first observation. Losing a pawn (disconnect hold) is silent and returns the player to unobserved; seat release drops the player's edge memory. No initial-state option: a milestone over persistent state guards on a value its reaction flips (`xp ≥ 100 && level < 2`). Diverges from crossings' arm-only rule on purpose; arm-only drops a player who joins already underwater.
- **`on.player` is the event's player, as command target and read owner.** It is a subject token like `on.activators`, carrying the same methods — `on.player.damage(n)`, `.grantHealth(n)`, `.grantAmmo(type, n)` and `.addSlot(slot, delta)` — which resolve to that player's pawn or seat (wire `target: "@player"`). As a subject token it is legal only before any `wait`; after one, the install check drops the reaction, as for `on.activators`. `ref.byPlayer(on.player)` reads that player's value in the fired reaction. Both are legal only in reactions this source fires; a reaction using either, subscribed to any other source, is rejected at install, naming the reaction and the source (E18 sentinel rule). `Seat` never reaches the authoring surface (E16).
- **Owner-private engine player slots become per-player.** The engine-state catalog marks them, and each has one host-side lookup from pawn to value that reads the pawn's components. Owner-private replication, condition reads and `byPlayer` reads all share that lookup, so no stored per-player copy can drift. The lookup lives below `netcode` so both the sim-side scope and replication reach it (`development_guide.md` §Workspace). Outside this source, a plain read keeps meaning the local player — HUD binds, `bindState`, local crossings, impact-policy ambient reads. `byPlayer` on these refs takes `on.player` or `impact.source`. E24 adds its swim slots on this seam.
- **Crossings are unchanged.** `onStateCrossing` on a mod per-owner slot stays rejected at bind (`plans/done/E16--per-player-currency` Decisions): accepting it would silently watch one player on the host, the trap this brief rejects in the mode-flag rival, and would fix crossing semantics ahead of the redesign this brief defers.
- **Own-state presentation stays client-local; `players().on` presentation is for host decisions.** An effect that reflects a player's own replicated state — a low-health vignette, an ammo warning, a swim splash — is a local `onStateCrossing` on that player's machine, with no host round trip and no loss (as `coop-trigger-screen-effects` and `movement--state-transition-feel` treat own-state feedback). `players().on` carries presentation that follows a host decision: a level-up fanfare, a scald on overheat. `scripting.md` teaches the split.
- **Presentation plays on the target player's own machine.** Each fire carries an internal target player, distinct from the `on.player` token, which routes presentation only. A `playSound`, `rumble`, `flashScreen`, `vignette` or `screenShake` reaction in this source's fire list runs locally when that player is the host's or single player's, and otherwise is sent to that client alone as a new Presentation-channel message kind, which the client turns back into the same local command. Unreliable, like other transient feedback (`networking.md` §Presentation events vs. replicated state); the receiving machine's accommodations (E23 flash limiter, reduce motion) apply. "World-anchored only" describes today's payloads, not the channel; the damage bearing rides slots because it is a continuous fact, and these are one-shot commands. The wire version bumps.
- **`fire(r)` keeps E18's context-free contract** (`plans/done/E18--timed-reaction-steps` V4b). A `players().on` reaction whose `fire` step, at any position, reaches a presentation reaction or a plain per-player read is dropped at install with an error naming both, as V4a/V4b drop what `fire` would lose. Presentation for the event's player goes in the source's fire list; `scripting.md` teaches it. `reaction-body-composition` adds inline system steps, which route without `fire`.
- **No emitter.** This source publishes no `emitter`; `playSound(…, { at: on.emitter })` in its reaction warn-skips as for other emitterless sources (`scripting.md` §12). Its sounds play unpositioned on the player's machine.
- **Non-goals.**
  - Whose screen a *trigger*-fired effect belongs to: `coop-trigger-screen-effects` owns that policy and reuses this delivery path. Until it settles, routing is engine-chosen — `players().on` presentation reaches the subject only, with no author override such as broadcasting to every player.
  - Redesigning `onStateCrossing`. The owner suspects it predates a netplay-aware design; `research.md` §Symptom watch records evidence as it appears.
  - Per-player named engine events (`playerDied` carrying its player), `changes` over non-Bool values, positioned sounds heard by every nearby player, per-owner `accumulate`, and E16's open environmental-damage policy.
  - E24's swim slots themselves.
  - Multi-effect reaction bodies and system reactions as sequence steps: `reaction-body-composition` owns both. This brief ships on today's body shapes, one effect per reaction, and does not wait on it.

### Scripting surface
```ts
import { players, becomes, ceases, read, defineStore, defineReaction, getGameState } from "postretro";
import type { PlayerEventParams } from "postretro";
import { playSound, flashScreen, vignette, updateState, onStateCrossing } from "postretro/ui";

const progress = defineStore({ namespace: "progress", schema: {
  xp:    { type: "number", default: 0, perOwner: true, network: "ownerPrivate" },
  level: { type: "number", default: 1, perOwner: true, network: "ownerPrivate" },
  lastLevelUpXp: { type: "number", default: 0, network: "shared" },
}});
const player = getGameState().player;

// One effect per reaction; a source's fire list runs several.
const levelUp = defineReaction((on: PlayerEventParams) => on.player.addSlot(progress.level, 1));
const recordLevelUp = defineReaction((on: PlayerEventParams) =>
  updateState(progress.lastLevelUpXp, read(progress.xp.byPlayer(on.player))));
const fanfare = defineReaction(playSound("level_up"));          // on that player's machine
const goldFlash = defineReaction(flashScreen([1, 0.9, 0.3, 0.4], 300));
const scald = defineReaction((on: PlayerEventParams) => on.player.damage(5));
const scaldHiss = defineReaction(playSound("scald"));
const cooled = defineReaction(playSound("vent_hiss"));
const bleeding = defineReaction(vignette(0.6, 800, [0.8, 0, 0]));

export function setupLevel() {
  return {
    reactions: [levelUp, recordLevelUp, fanfare, goldFlash, scald, scaldHiss, cooled, bleeding],
    playerEvents: [   // host decisions: run on the host, once per player
      players().on(becomes(read(progress.xp).ge(100).and(read(progress.level).lt(2))),
        [levelUp, recordLevelUp, fanfare, goldFlash]),
      players().on(becomes(read(player.overheated)), [scald, scaldHiss]),  // overheating burns the wielder
      players().on(ceases(read(player.overheated)), [cooled], { levels: ["campaign"] }),
    ],
    crossings: [      // own-state feedback: each machine, its own player
      onStateCrossing(player.health, { below: 25 }, [bleeding]),
    ],
  };
}
```
`players().on(edge, fire, options?)` returns a descriptor for the `playerEvents` key on `setupLevel`'s manifest and on `ModManifest`; `options.levels` scopes it like crossings. `PlayerEventParams` carries `player` and `rising`. Luau mirrors it with colon calls: `Postretro.players():on(…)`, `on.player:damage(5)`.

## Acceptance
### Automated
**Per-player firing**
- [ ] Two players on the host: the guarded XP milestone fires once for the player who crosses, and `on.player.addSlot(…)` credits only that player. The other player crossing later fires once more, for them.
- [ ] Two players crossing on the same tick fire twice, once per player, in the same order on every run.
- [ ] `becomes` fires on false→true only, `ceases` on true→false only; `changes` fires on both with `on.rising` true then false.
- [ ] A condition over a store slot written by an in-tick trigger fires on that tick; over a slot written by a frame-end reaction drain, on the next.
- [ ] A non-Bool condition is rejected at install, naming the event; a Bool one beside it installs.

**First sight**
- [ ] A player for whom the condition holds at level install fires `becomes` on the first tick; one for whom it is false does not fire `ceases`.
- [ ] A remote player joining while the condition holds fires `becomes` on their first observed tick.
- [ ] A disconnect hold fires nothing. Reclaiming within the hold while the condition holds fires `becomes` again; reclaiming while it is false fires nothing.
- [ ] After seat release no edge memory remains for that player.
- [ ] A guarded milestone does not re-fire across a level transition or a reclaim; the same milestone without its guard re-fires at each.

**Reads and targets**
- [ ] On the host, with two players at different health, a condition over `player.health` fires for exactly the player whose own health crossed, and `read(player.health.byPlayer(on.player))` in the fired reaction yields that player's value, not the host's.
- [ ] Every owner-private engine player slot resolves through the shared lookup; owner-private snapshots for two clients are byte-identical before and after the conversion.
- [ ] A plain read of an engine player slot still means the local player in a HUD bind, `bindState`, a local `onStateCrossing` and an impact policy.
- [ ] A plain `read(player.health)` or plain mod per-owner read in a reaction this source fires, directly or through a `fire` step, is rejected at install, naming the slot and pointing to `byPlayer(on.player)`; the same read inside the event's condition installs and binds to the evaluated player.
- [ ] The same reaction, holding a plain `read(player.health)`, installs under a local crossing alone and reads that machine's own player there.
- [ ] `byPlayer(impact.source)` on an engine player slot in an impact policy reads the source player's value.
- [ ] A reaction using `on.player` as a target or a `byPlayer` owner, subscribed to `levelLoad`, a crossing or a trigger event, is rejected at install, naming the reaction and the source; the same reaction under `players().on` installs.
- [ ] `on.player` resolving to a player whose pawn despawned before the drain warn-skips the command and leaves sibling commands applying.
- [ ] A reaction with an `on.player` step after a `wait` is dropped at install with an error naming it; the same step before the `wait` installs and lands on the event's player.

**Roles**
- [ ] A connected client neither registers nor evaluates `players().on`, and fires nothing from it.
- [ ] `onStateCrossing` on a mod per-owner slot is still rejected at bind on every role, naming the slot (regression guard); a local crossing on `player.health` still fires on each machine for its own player.

**Presentation**
- [ ] Loopback harness, host plus two clients: a `flashScreen` from an event for client A arrives on A only — not the host, not client B. The host's own player's event presents on the host only.
- [ ] Each presentation command round-trips the new message kind with identical fields; a dropped message presents nothing and breaks nothing.
- [ ] A forwarded `flashScreen` passes through the receiving machine's flash limiter and reduce-motion scaling.
- [ ] `playSound` with `at: on.emitter` in a `players().on` reaction warn-skips; without `at` it plays.
- [ ] A fire list holding a consequence and two presentation reactions runs all three, each dispatch path in listed order (`scripting.md` §12 trigger ordering): the credit applies on the event's tick, and the sound and flash present on the event's player only.
- [ ] A `players().on` reaction whose `fire` step reaches a presentation reaction is dropped at install with an error naming both, before and after a `wait`; the same presentation reaction listed directly in the event's fire list installs and presents on the event's player only.
- [ ] A `players().on` reaction whose `fire` step reaches a reaction with no presentation and no plain per-player read installs and runs.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` script, and its TypeScript and Luau twins produce byte-identical wire data.

### Manual
- [ ] Two-client co-op playtest: the level-up sound and flash and the scald sound appear only on the affected player's machine; the low-health vignette (a local crossing) appears on the bleeding player's machine with no perceptible delay after the hit.

## Path
- Evaluation seam: the tail of `simulate_tick_with_presentation_aim`, after `run_death_sweep`, where the tick's `AuthoritativePlayer` list is still in scope. `canonical_player_pawns` picks one pawn per player.
- Firing: reuse the in-tick trigger command machinery — a `TriggerFireContext` whose activator is the player's pawn, `BoundTarget::Activators`, residuals carrying `PlayerId` to the frame-end drain. The rival extends `NamedEventDispatchContext`/`SystemCommandFireContext` with a player field; weigh it against how presentation steps reach the drain.
- Condition scope: a player-seeded scope reusing `EntityScope`'s owned-read pieces (`seed_owner_seat_from_registry`, `resolve_owner_input`, `read_owned_store`), routing plain per-owner names to the seeded seat and engine per-player names to the lookup. `DispatchScope` opts into `resolve_owned_input` for the `@player` owner only.
- Engine lookup: lift `descriptor_health_for_pawn`, the weapon slot projection and the per-owner step of `owner_private_source_value` (`netcode/src/state_slots.rs`) into `sim` or `combat-model`; `netcode` calls it back.
- Presentation: a new `ServerPresentationPayload` variant, appended (bitcode tags are positional), sent with `NetServer::send_presentation`; client intake beside `ingest_client_presentation_messages` pushes onto the client's system command queue. Confirm that queue drains on the connected-client frame path.
- SDK: `read()` widens to engine per-player refs; `byPlayer` is generated for them (non-enumerable, per `scripting-state-convergence`); `@player` joins the `@activators` parsers in both runtimes.
- First slice: one engine slot (`player.health`) through the lookup, a `becomes` condition on two loopback players, a `on.player.damage(…)` landing on the right pawn. This falsifies the scope routing and the lookup placement, the riskiest pieces.
- Split first, behavior-preserving: `main.rs`, `netcode/src/state_slots.rs`, `reaction_dispatch.rs`, `system_reactions.rs`, `scopes.rs`, `engine_state_catalog.rs`, `state_crossings.rs`, `data_script.ts` are all past ~800 lines; split only the ones this work extends.

## Open questions
- Where a player-event fire is ordered against same-tick trigger fires in the frame-end drain — **delegated**: the executor pins it and reports it in the plan of record.
- Whether the shared engine lookup lands in `sim` or `combat-model` — **delegated**.

## Boundary inventory
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Source builder | player-event descriptor | manifest JSON `playerEvents[]` | `players().on(edge, fire, options?)` | `players().on(edge, fire, options?)` | n/a |
| Edge words | edge enum `Becomes \| Ceases \| Changes` | `"becomes"`, `"ceases"`, `"changes"` | `becomes(cond)`, `ceases(cond)`, `changes(cond)` | same | n/a |
| Params | dispatch inputs | `@player` (target and owner token), `@rising` (Bool) | `PlayerEventParams { player, rising }` | same | n/a |
| Manifest key | `LevelManifest`/`ModManifest` field | `playerEvents` | `playerEvents` | `playerEvents` | n/a |
| Per-player engine slots | catalog per-player flag + lookup | owner-private (unchanged) | `player.health`, `player.maxHealth`, `player.ammo`, `player.ammoReserve`, `player.heat`, `player.overheatAt`, `player.overheated`, `player.cell`, `player.cellCapacity`, `player.reloadActive`, `player.reloadProgress`, `player.weaponCooldownMs` | same | n/a |
| Presented command | system reaction command | new `ServerPresentationPayload` variant, appended; `WIRE_VERSION` bump | n/a | n/a | n/a |

## Wire format
The new Presentation payload variant mirrors `SystemReactionCommand`'s presentation arms as a net-crate wire type — the engine-to-wire conversion lives in `netcode`, so no engine type enters `net`. One variant carrying a nested command enum: `PlaySound { sound: String, bus: Option<String> }`, `Rumble { strong: f32, weak: Option<f32>, duration_ms: f32 }`, `FlashScreen { color: [f32; 4], duration_ms: f32 }`, `Vignette { color: Option<[f32; 3]>, strength: f32, duration_ms: f32 }`, `ScreenShake { amplitude: f32, duration_ms: f32, frequency: Option<f32> }`. Bitcode, appended after `OverlayFact`; non-finite floats are dropped at client intake as `Spawn` anchors are. `WIRE_VERSION` advances by one.
