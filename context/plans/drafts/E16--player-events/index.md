# E16--player-events

Brief · resumable · Epic 16 · reads: `context/lib/scripting.md` §5, §10.4, §11, §12 · `context/lib/networking.md` §Presentation events vs. replicated state, §Session-state ledger · `context/lib/development_guide.md` §Workspace · read at 04f049215

## Problem
A requested capability, raised by the owner and by E24's direction review. In co-op a mod cannot react to one player's state changing — "when a player's XP reaches 100, level that player up", "when a submerged player runs out of air, damage that player". `onStateCrossing` runs on every machine over that machine's own view, so on the host it sees only the host's player; engine player slots hold only the local pawn's value; and E16 rejects crossings on mod per-owner slots outright. The host-only sources that do see every player, trigger events and impact policies, fire on spatial and hit edges, not state, and any sound or screen effect they fire plays on the host's machine. When done, a mod declares a per-player event over any per-player value; the host fires it once per player per edge, naming that player as command target and read owner; every owner-private engine player value is readable per player on the host; and the event's sounds and screen effects play on that player's own machine. E24's swim slots and E16's per-player XP level-up are the first consumers.

## Decisions
- **A host-only sibling source, not a crossing mode.** `onPlayerEvent` registers and evaluates on host and single player only, never on a connected client — the trigger-event model (`plans/done/E18--trigger-event-params`). `onStateCrossing` keeps its meaning: every machine, its own view (`scripting.md` §10.4). A mode flag on crossings was rejected: one name would cover two machine sets, and forgetting the flag silently watches the host's player only.
- **Evaluated once per authoritative tick, after the tick settles** — after the death sweep, so a condition sees this tick's damage and ammo. Frame-rate independent and inside the determinism gate. A slot written by a frame-end reaction drain is seen the following tick.
- **Condition plus edge word.** The condition is a Bool expression in the fluent algebra (`read(ref)`, `.ge`, `.and`, …). `becomes(cond)` fires false→true, `ceases(cond)` true→false, `changes(cond)` both, publishing `on.rising`. A non-Bool condition is rejected at install, naming the event. There is no threshold form; a threshold is `read(x).ge(t)`.
- **Inside the condition, a plain read of a per-player slot means the player being evaluated** — engine per-player slots and mod per-owner slots alike; global slots read as everywhere. This is the one place a plain per-owner read binds. E16's explicit-owner rule (`plans/done/E16--per-player-currency` Decisions) holds everywhere else; here the owner is the event's subject and cannot be the wrong player.
- **First sight: an unobserved player counts as false.** Every player starts unobserved at level install, and a player without a pawn is unobserved. `becomes` fires at first observation if the condition holds — at install, on join, on reclaim. `ceases` never fires from first observation. Losing a pawn (disconnect hold) is silent and returns the player to unobserved; seat release drops the player's edge memory. No initial-state option: a milestone over persistent state guards on a value its reaction flips (`xp ≥ 100 && level < 2`). Diverges from crossings' arm-only rule on purpose; arm-only drops a player who joins already underwater.
- **`on.player` is the event's player, as command target and read owner.** It joins the `@activators` family: `damage`, `grantHealth`, `grantAmmo` and `addSlot` resolve it to that player's pawn or seat. `ref.byPlayer(on.player)` reads that player's value in the fired reaction. Both are legal only in reactions this source fires; a reaction using either, subscribed to any other source, is rejected at install, naming the reaction and the source (E18 sentinel rule). `Seat` never reaches the authoring surface (E16).
- **Owner-private engine player slots become per-player.** The engine-state catalog marks them, and each has one host-side lookup from pawn to value that reads the pawn's components. Owner-private replication, condition reads and `byPlayer` reads all share that lookup, so no stored per-player copy can drift. The lookup lives below `netcode` so both the sim-side scope and replication reach it (`development_guide.md` §Workspace). A plain read anywhere else keeps meaning the local player — HUD binds, `bindState`, local crossings, impact-policy ambient reads. `byPlayer` on these refs takes `on.player` or `impact.source`. E24 adds its swim slots on this seam.
- **Lift E16's crossing rejection.** `onStateCrossing` on a mod per-owner slot binds and watches this machine's own projection, as it already does for engine player slots. `scripting.md` steers per-player gameplay to `onPlayerEvent`. Amends `plans/done/E16--per-player-currency` Decisions.
- **Presentation plays on `on.player`'s own machine.** A `playSound`, `rumble`, `flashScreen`, `vignette` or `screenShake` step in a reaction this source fires runs locally when that player is the host's or single player's, and otherwise is sent to that client alone as a new Presentation-channel message kind, which the client turns back into the same local command. Unreliable, like other transient feedback (`networking.md` §Presentation events vs. replicated state); the receiving machine's accommodations (E23 flash limiter, reduce motion) apply. "World-anchored only" describes today's payloads, not the channel; the damage bearing rides slots because it is a continuous fact, and these are one-shot commands. The wire version bumps.
- **No emitter.** This source publishes no `emitter`; `playSound(…, { at: on.emitter })` in its reaction warn-skips as for other emitterless sources (`scripting.md` §12). Its sounds play unpositioned on the player's machine.
- **Non-goals.**
  - Whose screen a *trigger*-fired effect belongs to: `coop-trigger-screen-effects` owns that policy and reuses this delivery path.
  - Redesigning `onStateCrossing`. The owner suspects it predates a netplay-aware design; `research.md` §Symptom watch records evidence as it appears.
  - Per-player named engine events (`playerDied` carrying its player), `changes` over non-Bool values, positioned sounds heard by every nearby player, per-owner `accumulate`, and E16's open environmental-damage policy.
  - E24's swim slots themselves.

### Scripting surface
```ts
import { onPlayerEvent, becomes, ceases, read, addSlot, damage, defineStore, defineReaction, getGameState } from "postretro";
import type { PlayerEventParams } from "postretro";
import { playSound, flashScreen, vignette, updateState } from "postretro/ui";

const progress = defineStore({ namespace: "progress", schema: {
  xp:    { type: "number", default: 0, perOwner: true, network: "ownerPrivate" },
  level: { type: "number", default: 1, perOwner: true, network: "ownerPrivate" },
  lastLevelUpXp: { type: "number", default: 0, network: "shared" },
}});
const player = getGameState().player;

const levelUp = defineReaction((on: PlayerEventParams) => [
  addSlot(on.player, progress.level, 1),
  updateState(progress.lastLevelUpXp, read(progress.xp.byPlayer(on.player))),
  playSound("level_up"),                   // on that player's machine
  flashScreen([1, 0.9, 0.3, 0.4], 300),
]);
const bleeding = defineReaction([vignette(0.6, 800, [0.8, 0, 0])]);
const steadied = defineReaction([playSound("heartbeat_settle")]);
const scald = defineReaction((on: PlayerEventParams) => [damage(on.player, 5)]);

export function setupLevel() {
  return {
    reactions: [levelUp, bleeding, steadied, scald],
    playerEvents: [
      onPlayerEvent(becomes(read(progress.xp).ge(100).and(read(progress.level).lt(2))), [levelUp]),
      onPlayerEvent(becomes(read(player.health).le(25)), [bleeding]),
      onPlayerEvent(ceases(read(player.health).le(25)), [steadied], { levels: ["campaign"] }),
      onPlayerEvent(becomes(read(player.overheated)), [scald]),   // overheating burns the wielder
    ],
  };
}
```
`onPlayerEvent(edge, fire, options?)` returns a descriptor for the `playerEvents` key on `setupLevel`'s manifest and on `ModManifest`; `options.levels` scopes it like crossings. `PlayerEventParams` carries `player` and `rising`. Luau mirrors it.

## Acceptance
### Automated
**Per-player firing**
- [ ] Two players on the host: the guarded XP milestone fires once for the player who crosses, and `addSlot(on.player, …)` credits only that player. The other player crossing later fires once more, for them.
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
- [ ] `byPlayer(impact.source)` on an engine player slot in an impact policy reads the source player's value.
- [ ] A reaction using `on.player` as a target or a `byPlayer` owner, subscribed to `levelLoad`, a crossing or a trigger event, is rejected at install, naming the reaction and the source; the same reaction under `onPlayerEvent` installs.
- [ ] `on.player` resolving to a player whose pawn despawned before the drain warn-skips the command and leaves sibling commands applying.

**Roles**
- [ ] A connected client neither registers nor evaluates `onPlayerEvent`, and fires nothing from it.
- [ ] `onStateCrossing` on a mod per-owner slot binds on host and client and fires on each for that machine's own value; the E16 rejection tests are inverted.

**Presentation**
- [ ] Loopback harness, host plus two clients: a `flashScreen` from an event for client A arrives on A only — not the host, not client B. The host's own player's event presents on the host only.
- [ ] Each presentation command round-trips the new message kind with identical fields; a dropped message presents nothing and breaks nothing.
- [ ] A forwarded `flashScreen` passes through the receiving machine's flash limiter and reduce-motion scaling.
- [ ] `playSound` with `at: on.emitter` in an `onPlayerEvent` reaction warn-skips; without `at` it plays.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` script, and its TypeScript and Luau twins produce byte-identical wire data.

### Manual
- [ ] Two-client co-op playtest: the low-health vignette and the level-up sound and flash appear only on the affected player's machine.

## Path
- Evaluation seam: the tail of `simulate_tick_with_presentation_aim`, after `run_death_sweep`, where the tick's `AuthoritativePlayer` list is still in scope. `canonical_player_pawns` picks one pawn per player.
- Firing: reuse the in-tick trigger command machinery — a `TriggerFireContext` whose activator is the player's pawn, `BoundTarget::Activators`, residuals carrying `PlayerId` to the frame-end drain. The rival extends `NamedEventDispatchContext`/`SystemCommandFireContext` with a player field; weigh it against how presentation steps reach the drain.
- Condition scope: a player-seeded scope reusing `EntityScope`'s owned-read pieces (`seed_owner_seat_from_registry`, `resolve_owner_input`, `read_owned_store`), routing plain per-owner names to the seeded seat and engine per-player names to the lookup. `DispatchScope` opts into `resolve_owned_input` for the `@player` owner only.
- Engine lookup: lift `descriptor_health_for_pawn`, the weapon slot projection and the per-owner step of `owner_private_source_value` (`netcode/src/state_slots.rs`) into `sim` or `combat-model`; `netcode` calls it back.
- Presentation: a new `ServerPresentationPayload` variant, appended (bitcode tags are positional), sent with `NetServer::send_presentation`; client intake beside `ingest_client_presentation_messages` pushes onto the client's system command queue. Confirm that queue drains on the connected-client frame path.
- SDK: `read()` widens to engine per-player refs; `byPlayer` is generated for them (non-enumerable, per `scripting-state-convergence`); `@player` joins the `@activators` parsers in both runtimes.
- First slice: one engine slot (`player.health`) through the lookup, a `becomes` condition on two loopback players, a `damage(on.player, …)` landing on the right pawn. This falsifies the scope routing and the lookup placement, the riskiest pieces.
- Split first, behavior-preserving: `main.rs`, `netcode/src/state_slots.rs`, `reaction_dispatch.rs`, `system_reactions.rs`, `scopes.rs`, `engine_state_catalog.rs`, `state_crossings.rs`, `data_script.ts` are all past ~800 lines; split only the ones this work extends.

## Open questions
- Where a player-event fire is ordered against same-tick trigger fires in the frame-end drain — **delegated**: the executor pins it and reports it in the plan of record.
- Whether the shared engine lookup lands in `sim` or `combat-model` — **delegated**.

## Boundary inventory
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Source builder | player-event descriptor | manifest JSON `playerEvents[]` | `onPlayerEvent(edge, fire, options?)` | `onPlayerEvent(edge, fire, options?)` | n/a |
| Edge words | edge enum `Becomes \| Ceases \| Changes` | `"becomes"`, `"ceases"`, `"changes"` | `becomes(cond)`, `ceases(cond)`, `changes(cond)` | same | n/a |
| Params | dispatch inputs | `@player` (target and owner token), `@rising` (Bool) | `PlayerEventParams { player, rising }` | same | n/a |
| Manifest key | `LevelManifest`/`ModManifest` field | `playerEvents` | `playerEvents` | `playerEvents` | n/a |
| Per-player engine slots | catalog per-player flag + lookup | owner-private (unchanged) | `player.health`, `player.maxHealth`, `player.ammo`, `player.ammoReserve`, `player.heat`, `player.overheatAt`, `player.overheated`, `player.cell`, `player.cellCapacity`, `player.reloadActive`, `player.reloadProgress`, `player.weaponCooldownMs` | same | n/a |
| Presented command | system reaction command | new `ServerPresentationPayload` variant, appended; `WIRE_VERSION` bump | n/a | n/a | n/a |

## Wire format
The new Presentation payload variant mirrors `SystemReactionCommand`'s presentation arms as a net-crate wire type — the engine-to-wire conversion lives in `netcode`, so no engine type enters `net`. One variant carrying a nested command enum: `PlaySound { sound: String, bus: Option<String> }`, `Rumble { strong: f32, weak: Option<f32>, duration_ms: f32 }`, `FlashScreen { color: [f32; 4], duration_ms: f32 }`, `Vignette { color: Option<[f32; 3]>, strength: f32, duration_ms: f32 }`, `ScreenShake { amplitude: f32, duration_ms: f32, frequency: Option<f32> }`. Bitcode, appended after `OverlayFact`; non-finite floats are dropped at client intake as `Spawn` anchors are. `WIRE_VERSION` advances by one.
