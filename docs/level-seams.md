# Level Seams

A level seam joins two maps. A player who walks into it crosses behind a
loading screen into the next map, at the same place relative to the seam,
facing and moving the same way, carrying their gear. Build the same doorway or
hallway in both maps, put a seam over it in each with one shared name, and the
two maps play as one walk.

Players outside the seam when it fires arrive at the next map's player starts.
A crossing is also a checkpoint: Continue on that run starts the player where
they crossed.

Only players cross. Monsters, items and projectiles inside the seam stay
behind. Crossing back loads the earlier map fresh: "where they left off" means
the player's place, not the map's state.

---

## Placing a seam

In each of the two maps:

1. Draw one or more convex brushes over the crossing (a doorway, a hallway, an
   airlock). Select them and use **Entity > Tie to Entity** (or press `T`) to
   make them a `level_seam`. Several brushes make one seam, so a bent hallway
   is fine: a player counts as inside only where the brushes are, never in the
   rooms beside the bend.
2. Give the seam a `name`, the same in both maps, and set `target` to the other
   map's catalog id (its `id` in your mod's `defineMapCatalog`).
3. Optionally place a `teleport_destination` inside it, give it a `name`, and
   set the seam's `landmark` to that name. Do the same in the other map.

The player's place is measured from the landmark: their offset from it, up and
down included, and their view and motion turned by the difference between the
two landmarks' yaws. Build the second map's copy of the crossing around its
landmark exactly as the first map's sits around its own. Turn the landmark, and
the copy, to change which way the second map's crossing faces.

Without a `landmark`, the seam's landmark is the centre of its brushes' bounding
box at yaw 0. That works when both maps build the crossing at the same
coordinates and facing.

The seam is never solid and draws nothing. Build the walls of the crossing
yourself, in both maps, so the player cannot see that the two maps differ
beyond it.

### `level_seam` keys

| Key | Type | Default | What it does |
|-----|------|---------|--------------|
| `name` | string | **required** | The seam's name, shared by both maps. Unique among the seams of one map. |
| `target` | string | **required** | The catalog id of the map this seam leads to. |
| `activation` | `touch` or `use` | `touch` | `touch` crosses when a player walks in; `use` when a player inside presses use. |
| `mode` | `classic` or `seamless` | `classic` | `classic` crosses behind a loading screen. `seamless` is reserved for crossing with no loading screen; until it ships, a `seamless` seam crosses exactly as a classic one. |
| `landmark` | string | — | The `name` of a `teleport_destination` the crossing is measured from. Blank uses the seam's bounds centre at yaw 0. |
| `enabled_on_spawn` | `0` or `1` | `1` | Whether the seam is on when the level starts. `0` leaves it off until a script arms it. |
| `on_arrive` | string | blank | The name of a reaction to run for each player who arrives through this seam. |
| `_tags` | string | blank | Space-delimited tags that scripts find the seam by, and that `occupancy.tag(tag)` counts it under. |

The build fails, naming the entity, on a missing `name` or `target`, two seams
with one `name` in a map, or a `landmark` naming no `teleport_destination`. A
map with a seam but no `player_spawn` builds with a warning: players outside a
seam need a start to arrive at.

The compiler sees one map at a time, so it cannot check the other side. At run
time:

- A `target` the catalog does not know warns once when the seam fires, and
  nothing happens: the level keeps running.
- A destination map with no seam of the same name places every crossing player
  at a player start, with one warning.

---

## What crosses, and how

When a seam fires, the level change waits for the end of the frame. Then every
player inside the seam, by the seam's own shape, is read before the map
unloads: where they stand relative to the landmark, which way they look, and
every part of their motion, dash and slide included. In the next map each of
them arrives at the same place relative to that map's seam, turned by the
landmark difference, looking the same way relative to it and moving the same
way. A dash or slide continues; a vault in progress ends. Looking up or down
carries over unchanged.

- **Who arrives at the seam.** Only players inside the seam when the map
  unloads. One who triggered it and stepped back out before the load began
  arrives at a player start.
- **Gear and health.** As for any level change: the seam changes where a player
  arrives, never what they carry or their health. A downed player inside the
  seam crosses too.
- **Blocked arrivals.** If a player's arrival would put them inside a wall, a
  moving platform or another player, they arrive at a player start instead,
  with one warning.
- **Only the first spawn.** The arrival is used once, by the player's first
  spawn in the next map. A respawn uses the player starts, and so does the map
  after that one unless it too is reached by a seam.
- **Which start.** A player who arrives at a seam spawns as the player class of
  the start they would otherwise have taken.

A seam never bounces a player straight back. A player who arrives inside a
seam, or respawns inside one, does not set it off until they step fully out
and back in. Arming a seam does not change that.

### What fires it

- A seam that is off never fires.
- A player at zero health cannot set a seam off, as they cannot teleport. If
  your mod declares player life states, each state's `touch` and `use`
  capabilities decide instead, so a downed player can be allowed to take the
  exit.
- A script `loadLevel`, `restartLevel` or `returnToFrontend` in the same frame
  wins over any seam.
- If several seams fire in one frame, the first in map order wins.
- A seam in a map shown behind the frontend menu never fires.

---

## Seam checkpoints

A crossing during a run saves the run at the destination seam: the seam's name,
and the host's (or single player's) place there if they crossed inside it.
Continue then starts that player at the seam, at rest, as if they had just
arrived.

The seam's name also names a spawn set (see
[Checkpoint spawns](level_design.md#checkpoint-spawns)): after a crossing, and
after Continue from one, respawns, late joiners and players who were outside
the seam use the `player_spawn`s keyed `checkpoint <seam name>`. Most seams key
none, and then the level-start spawns apply with no warning. A later
`checkpoint(name)` replaces the seam's set and its saved place, and the next
level start clears both.

---

## Waiting for everyone

A seam carrying a tag counts towards that tag's occupancy, by its own shape,
whether it is on or off (see
[Occupancy by tag](scripting-reference.md#occupancy-by-tag)). Leave the seam off
(`enabled_on_spawn 0`) and arm it once every player is inside it, and the whole
party crosses together.

---

## Scripting

`getMapEntities("seam", { tag })` returns each seam carrying the tag. A seam
member arms and disarms the seam and binds its `arrive` event:

```typescript
import { defineReaction, getMapEntities } from "postretro";
import type { SeamEventParams } from "postretro";

export function setupLevel() {
  const airlock = getMapEntities("seam", { tag: "airlock" });
  const cycle = defineReaction("airlock.cycle", { sequence: airlock.flatMap((s) => s.arm()) });
  const sealed = defineReaction("airlock.sealed", (on: SeamEventParams) => on.seam.disarm());
  return {
    reactions: [cycle, sealed],
    triggerEvents: getMapEntities("trigger", { tag: "airlock_button" }).map((t) => t.on("enter", [cycle])),
    seamEvents: airlock.map((s) => s.on("arrive", [sealed])),
  };
}
```

Luau mirrors it: `s:arm()`, `s:disarm()`, `s:on("arrive", { reaction })` under
`seamEvents`, and `on.seam:disarm()`.

### The `arrive` event

The destination seam fires `arrive` once for each player who arrives through it
(Continue at a saved seam place included), on the host, in the first game tick
after that player appears. It fires whether the seam is on or off, and never for
a player of the same crossing who arrived at a player start. Its reaction
receives `on.activators` (the arriving player) and `on.seam` (the seam, which
can `arm` and `disarm`). A brush `on_arrive` and a script binding both run.

Arming and disarming take effect at the start of the next game tick, as they do
for teleporters.

The dev mod's `seam-pair-a` and `seam-pair-b` maps are a working pair: each
seam disarms behind an arriving player, and a button in each room arms it again.
