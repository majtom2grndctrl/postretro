# Teleporters

A teleporter moves a player who walks into a region to somewhere else in the
level. It comes in two modes:

- **Classic** drops the player on a destination point, facing the way it faces.
  This is the Quake teleporter, except the player keeps their speed.
- **Seamless** carries the player relative to a pair of points, keeping their
  offset, view and momentum. Built over two near-identical copies of a room, it
  makes the world appear to change around the player while they stand still.

Scripts turn teleporters on and off from gameplay: a button, a monster closet
opening, the closet's monsters dying. Turning on a seamless teleporter that
players are standing in shifts all of them at once, with no input from them.

Only players teleport. Monsters and projectiles inside a teleporter stay where
they are, and a dead player never teleports.

---

## Placing a teleporter

You place two entities: a brush for the region the player walks into, and a point
for where they arrive.

1. Draw one or more convex brushes covering the region. Select them and use
   **Entity > Tie to Entity** (or press `T`) to make them a `trigger_teleport`.
   Several brushes make one region: the player can enter anywhere in their union.
2. Place a `teleport_destination` point where the player should arrive and give
   it a `name`.
3. Set the teleporter's `target` to that name.

The region is never solid and draws nothing. Like the other volumes in
[level_design.md](level_design.md#environment-volumes-fluids-gravity-and-push),
it ignores walls, so keep it inside the space you mean it for.

### `trigger_teleport` keys

| Key | Type | Default | What it does |
|-----|------|---------|--------------|
| `mode` | `classic` or `seamless` | `classic` | How the player arrives (below). Set it explicitly: a seamless teleporter is never inferred from a `landmark`. |
| `target` | string | **required** | The `name` of the `teleport_destination` the player arrives at. |
| `landmark` | string | — | Seamless only, and required there: the `name` of a `teleport_destination` inside the source region that the target corresponds to. Forbidden in classic mode, and must differ from `target`. |
| `enabled_on_spawn` | `0` or `1` | `1` | Whether the teleporter is on when the level starts. `0` leaves it off until a script turns it on. |
| `arrival_sound` | string | blank | A sound key under `sounds/`, without the extension, played at the arrival point. Blank plays nothing, which keeps a seamless shift silent. |
| `on_teleport` | string | blank | The name of a reaction to run each time a player teleports through it, as `on_fire` works on a `trigger_volume`. |
| `_tags` | string | blank | Space-delimited tags that scripts find the teleporter by. |
| `name` | string | blank | An editor label. Scripts address teleporters by tag. |

### `teleport_destination` keys

| Key | Type | What it does |
|-----|------|--------------|
| `name` | string | Required, and unique among destinations. Teleporters name it in `target` or `landmark`. |
| `angle`, `angles` or `mangle` | yaw, or pitch yaw roll | Only the yaw counts: the direction the arrow points in TrenchBroom. They read as on any other point entity; when more than one is set, `angles` or `mangle` wins over `angle`. A pitch or roll compiles with a warning and is ignored. |

The destination's origin is where the middle of the player's body arrives, not
their feet. The player arrives exactly there; the engine does not nudge them to
the floor or out of a wall. Set the origin at least the player's capsule
`halfHeight` plus `radius` above the floor, plus about one unit of margin. The
dev mod's player (`halfHeight` 0.8 m, `radius` 0.2 m) needs 1 m, so about 41
units. A destination set lower than that puts the player inside the floor, and
the arrival is [refused](#arrival). Up to about two units of margin still lands
them standing on the floor; set higher, the player arrives in the air and falls.
The margin is measured from the player's capsule as it is when they arrive, so a
crouching or sliding player reaching a destination set for standing arrives in
the air and drops the difference.

This is not the rule for `player_spawn`, whose box you rest on the floor (see
[Player Spawns](level_design.md#player-spawns)). Your mod's player script sets
the capsule (`movement.capsule` on the `player`
entity); use its numbers if they differ.

---

## Classic and seamless

Both modes turn the player by one angle on the tick they teleport: their view,
and every part of their motion — running speed, knockback, and the drift of a
push volume. Speeds and the view's pitch are unchanged.

- **Classic.** The player arrives exactly at the destination, facing its yaw.
  The turn is whatever brings their view to that yaw, so their momentum leaves
  the destination in the direction they now face.
- **Seamless.** The turn is the target's yaw minus the landmark's yaw. The
  player's offset from the landmark, height included, turns by the same angle
  and is laid out from the target. A player standing two metres east of the
  landmark and facing it arrives two metres from the target on the matching
  side, facing the target the same way.

### Building room copies for a seamless shift

1. Build the room twice: the original, and a copy far enough away that neither
   can see the other. The copy can differ — rubble, other lights, a different
   era — as long as its floor and walls line up where players will stand.
2. Put a `teleport_destination` in the original, say `era_a`, and one at the
   matching spot in the copy, `era_b`. If the copy is built rotated, give
   `era_b` the same rotation in its yaw.
3. Cover the original's floor area with a seamless `trigger_teleport`:
   `target` `era_b`, `landmark` `era_a`, `enabled_on_spawn` `0`.
4. For a way back, cover the copy with a second one: `target` `era_a`,
   `landmark` `era_b`. The pair shares its two points.
5. Turn them on from a script when the shift should happen.

Players standing in the original when it turns on shift on the next tick. They
arrive standing inside the copy's teleporter but are not sent straight back:
see [Arrival](#arrival).

`prl-build` records each teleporter as a shortcut to its destination, so the
engine starts loading the destination's lighting while players are still
approaching the teleporter, whether or not it is on. A shift does not arrive in
a room whose lighting is still streaming in.

---

## When a teleporter fires

- **The body point decides.** After the player moves each tick, a teleporter
  fires when the point the player's gravity and push are measured at is inside
  its region. Brushing the region with a shoulder does not.
- **One teleport per tick.** Where regions overlap, the one with the smaller
  total brush volume wins. Only an exact tie falls to the one that comes first
  in the `.map` file: near-identical overlapping teleporters may rank by
  floating-point rounding, so don't author congruent overlaps whose winner you
  depend on.
- **Turning on and off takes effect on the next tick.** A script that turns a
  teleporter off on the tick a player crosses in does not stop that crossing;
  it stops the next one. Several changes in one tick apply in the order they
  were made.
- **Turning on an occupied teleporter fires it.** Every player standing in a
  teleporter that a script turns on teleports on the next tick. Turning on one
  that is already on changes nothing.

### Arrival

- **Feet on the floor, the player is grounded.** The engine checks for static
  floor beneath the arrival position without moving the player. If their feet
  are within about two units of it, they are standing on it: a slide or run
  carries on across a seamless shift with no moment in the air, and they can
  jump at once.
- **Otherwise the player is airborne.** They fall from the destination, and
  there is no grace period for a late jump: a mid-air arrival owes no coyote
  jump, though double jumps and other air jumps still work. A player arriving
  while rising, mid-jump, is airborne too.
- A moving platform the player stood on never carries them after a teleport,
  even when the arrival floor is another platform.
- A vault in progress stops, and the `vault_ended` event fires as for any other
  end of a climb. A slide or dash carries on in the new direction.
- **No ping-pong.** A player who arrives inside a teleporter that is on does not
  fire it until they step out of it and back in. This is what lets a two-way
  seamless pair share its region and its points.
- **Blocked arrivals are refused.** If the player would arrive inside world
  geometry or a moving platform, they stay where they are, nothing fires, and
  the log warns once per teleporter. Leave room around destinations.

### Co-op

A player's own walk-in teleport happens instantly on their machine. A shift a
script causes reaches each remote player one network delay later, with their
position, view, motion and the arrival sound agreeing. Everyone else sees the
player cut from one place to the other, never sliding between them, and hears
the arrival sound at the arrival point.

---

## Scripting

### Finding teleporters

`getMapEntities("teleporter", { tag })` returns the map's teleporters carrying
that tag, in map order. Each member has `id`, `position` and `tags`, and these
methods:

| Method | Returns |
|--------|---------|
| `arm()` | Steps that turn the teleporter on. List them in a reaction body as they are; no spread is needed. |
| `disarm()` | Steps that turn it off. |
| `on("teleport", [reactions])` | A binding to return under `setupLevel`'s `teleportEvents`. The reactions run each time a player teleports through this teleporter. |

In Luau, call them as `t:arm()`, `t:disarm()` and `t:on("teleport", { reaction })`.

`arm` and `disarm` take effect on the host only, at the start of its next tick,
whatever reaction runs them. On a connected client they do nothing; the host's
change reaches it.

### The `teleport` event

A `teleport` reaction runs on the host once for each player each time that
player teleports, after the move. Its reactions receive `TeleportEventParams`:

| Field | What it is |
|-------|------------|
| `on.activators` | The player who teleported: `damage`, `grantHealth`, `grantAmmo`, `addSlot`. |
| `on.teleporter` | The teleporter that fired: `arm`, `disarm`. |

There is no `on.occupancy`. Like other subject tokens, `on.activators` and
`on.teleporter` are usable only before any `wait`. A brush's `on_teleport` key
runs its reaction on the same event.

### A one-way gate

A gate that closes behind the first player through it:

```typescript
import { defineReaction, getMapEntities } from "postretro";
import type { TeleportEventParams } from "postretro";

export function setupLevel() {
  const gate = getMapEntities("teleporter", { tag: "vault_gate" });
  const oneWay = defineReaction("gate.used", (on: TeleportEventParams) => on.teleporter.disarm());
  return {
    reactions: [oneWay],
    teleportEvents: gate.map((t) => t.on("teleport", [oneWay])),
  };
}
```

```lua
local Postretro = require("postretro")

function setupLevel(_ctx)
  local oneWay = Postretro.defineReaction("gate.used", function(on)
    return on.teleporter:disarm()
  end)
  local teleportEvents = {}
  for _, t in Postretro.getMapEntities("teleporter", { tag = "vault_gate" }) do
    table.insert(teleportEvents, t:on("teleport", { oneWay }))
  end
  return { reactions = { oneWay }, teleportEvents = teleportEvents }
end
```

The gate turns off on the tick after the first player crosses, so a second
player one tick behind does not follow.

### A closet loop and a time shift

The gate starts off (`enabled_on_spawn` `0`). A button turns it on, a pressure
plate turns it off and opens a monster closet, and killing the closet's
monsters turns it back on. A pad inside the original room turns on both
directions of a seamless pair, shifting everyone standing there.

```typescript
import { defineReaction, getMapEntities, npcs } from "postretro";
import type { TeleportEventParams } from "postretro";

export function setupLevel() {
  const gate = getMapEntities("teleporter", { tag: "vault_gate" });
  const shift = getMapEntities("teleporter", { tag: "era_shift" });
  const doors = getMapEntities("mover", { tag: "closet_door" });

  const open = defineReaction("gate.open", gate.map((t) => t.arm()));
  const shut = defineReaction("gate.shut", gate.map((t) => t.disarm()));
  const reveal = defineReaction("closet.reveal", [
    doors.map((d) => d.start()),
    npcs({ tag: "closet" }).update({ aggro: true }),
  ]);
  const cleared = defineReaction("closet.cleared", { progress: { tag: "closet", at: 1.0, fire: "gate.open" } });
  const timeShift = defineReaction("era.shift", shift.map((t) => t.arm()));
  const oneWay = defineReaction("gate.used", (on: TeleportEventParams) => on.teleporter.disarm());

  return {
    reactions: [open, shut, reveal, cleared, timeShift, oneWay],
    triggerEvents: [
      ...getMapEntities("trigger", { tag: "vault_button" }).map((t) => t.on("enter", [open])),
      ...getMapEntities("trigger", { tag: "closet_plate" }).map((t) => t.on("enter", [shut, reveal])),
      ...getMapEntities("trigger", { tag: "era_plate" }).map((t) => t.on("enter", [timeShift])),
    ],
    teleportEvents: gate.map((t) => t.on("teleport", [oneWay])),
  };
}
```

To let players in the two copies see each other while the shift is off — a
partner's ghost, a barrier opening in the other time — lay a ghost link over
each copy, reusing the same two points: see [ghost-links.md](ghost-links.md).

`progress` counts a closet monster when it is removed from the level, not the
moment its health reaches zero. Give the monsters a death policy that removes
them, such as an impact event whose effect is `impact.target.despawn()`, and the
gate opens when the last one goes. Only monsters placed in the
map count; monsters an `entity_spawner` releases do not. See
[Spawner members and `spawned_tags`](scripting-reference.md#spawner-members-and-spawned_tags).

---

## Compile errors

`prl-build` fails the build on any of these, and the error names the entity:

| Problem | Fix |
|---------|-----|
| `target` or `landmark` names no `teleport_destination` | Check the spelling against the destination's `name`. |
| Two destinations share a `name` | Rename one. |
| A seamless teleporter has no `landmark` | Add the source-side point. |
| A classic teleporter has a `landmark` | Remove it, or set `mode` to `seamless`. |
| `landmark` equals `target` | A seamless pair needs two different points. |
| `mode` is neither `classic` nor `seamless` | Fix the value. |
| `target` is blank or missing | Set it to a destination's `name`. |
| `enabled_on_spawn` is not `0` or `1` | Use `0` or `1`. |
| A `trigger_teleport` has no brushes | Tie at least one brush to it. |
| A `teleport_destination` has brushes | Make it a point entity; it is not a region. |

A destination with a pitch or roll in `angle`, `angles` or `mangle` compiles
with a warning; only its yaw is used.

## Trying it

Compile and run a map with teleporters as any other. From your project root, with `<mod-root>` the mod's folder (`content/dev` in the
SDK bundle):

```bash
bin/prl-build <mod-root>/maps/<name>.map \
  --baked-root baked --cache-dir .build-caches/prl-cache \
  -o <mod-root>/maps/<name>.prl
bin/postretro-tool run maps/<name>.prl
```

The compiler takes paths from the project root. `postretro-tool run` takes the
level path relative to the mod's folder, so it drops the `<mod-root>/` prefix.
`--cache-dir` keeps the compiler's scratch files out of your `maps/` folder; see
[Compiling Your Map](level_design.md#compiling-your-map) for both flags.

Rebuild the map after editing its data script; the engine runs the copy
compiled into the `.prl`.
