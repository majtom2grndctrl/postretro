# Ghost links

A ghost link lets one copy of a room see another. Build a room twice, at two
moments in time, and place a `ghost_link` over the first copy. While the link
is on, everything inside it that is allowed to ghost — players, monsters,
projectiles, tagged movers — also draws as a translucent ghost at the matching
spot in the second copy. With a style that carries sound, their sounds play
there too, through a muffled ghost bus.

Two players in co-op, one in each copy, then share a room across time:

- **A barrier puzzle.** A barrier blocks the exit in one copy. The player in
  the other copy presses a button there, watches the barrier's ghost open, and
  the first player walks through.
- **A battle swap.** Each player fights monsters only they can see, while
  watching the partner's ghost and aim. A script then swaps the players, so
  each faces what they could not see.

The swap itself is a seamless teleporter pair ([teleporters.md](teleporters.md#a-closet-loop-and-a-time-shift)).
A ghost link is independent of teleporters: a link can look into a copy nobody
teleports to.

Ghosts are presentation only. They never collide, block, take hits, set off
triggers or draw a monster's attention, and the copies stay physically
separate rooms: a shot or a blast in one copy never reaches the other.

---

## Placing a link

You place a brush for the region that is seen, and reuse two
`teleport_destination` points to say how it maps onto the other copy.

1. Build the room twice, the second copy far from the first. The copies can
   differ — rubble, other lights, a different era — but keep the parts players
   watch through the link lined up.
2. Put a `teleport_destination` in the first copy, say `era_a`, and one at the
   matching spot in the second, `era_b`. If the second copy is built rotated,
   give `era_b` the same rotation in its yaw.
3. Draw one or more convex brushes covering the first copy and tie them to a
   `ghost_link` (**Entity > Tie to Entity**, or `T`). Several brushes make one
   region.
4. Set `landmark` to `era_a` and `target` to `era_b`.
5. For the second copy to be seen from the first, cover it with a second link:
   `landmark` `era_b`, `target` `era_a`. A two-way link is two volumes.

A member's ghost draws at its pose carried from `landmark` to `target`, the
same way a seamless teleporter carries a player: a ghost and a seamless arrival
from the same spot always agree.

The region is never solid and draws nothing. It ignores walls, so it may reach
into them. Membership is judged by a single point — the middle of a player's
body, a projectile's origin, the centre of a mover's current bounds — so
extend the region wherever a member's point travels. A barrier that sinks into
the floor needs the region to reach below the floor, or its ghost stops at the
floor line and fades out.

### Place copies farther apart than the sound reaches

A sound in one copy plays normally there; the engine does not silence it in
the other copy. Place the copies farther apart than your mod's attenuation
`maxDistance` (`audio.attenuation` in the manifest, 60 metres by default,
about 2,400 map units), so a player never hears the real sound from the other
copy as well as its ghost.

### `ghost_link` keys

| Key | Type | Default | What it does |
|-----|------|---------|--------------|
| `landmark` | string | **required** | The `name` of a `teleport_destination` inside this copy. Must differ from `target`. |
| `target` | string | **required** | The `name` of the matching `teleport_destination` in the other copy, where ghosts draw. |
| `enabled_on_spawn` | `0` or `1` | `1` | Whether the link is on when the level starts. `0` leaves it off until a script turns it on. |
| `ghost_tags` | string | blank | Space-delimited `_tags` of the movers this link ghosts. Blank ghosts no movers. |
| `ghost_members` | string | blank | Space-delimited categories the link starts showing: any of `players`, `entities`, `projectiles`, `movers`. Blank shows all four. |
| `ghost_style` | string | blank | The name of a ghost style your manifest declares. Blank uses the engine's floor style. |
| `_tags` | string | blank | Space-delimited tags that scripts find the link by. |
| `name` | string | blank | An editor label. Scripts address links by tag. |

`teleport_destination` keys are in [teleporters.md](teleporters.md#teleport_destination-keys);
only the destination's yaw counts.

---

## What can ghost

Three layers decide, and a link only ever narrows what the first two allow.

1. **Descriptors** say which kinds of entity can ever ghost. A descriptor's
   `ghost` field, beside `canonicalName`, opts an archetype in. It defaults to
   `false` for every descriptor, your player included:

   ```typescript
   defineEntity({ canonicalName: "player", ghost: true, components: { /* ... */ } });
   defineEntity({ canonicalName: "phase_wraith", ghost: true, components: { /* ... */ } });
   ```

   A pawn ghosts holding its third-person weapon. A weapon's projectiles opt in
   on their own, with `ghost` in the weapon's `projectile` block, whoever fires
   them — a monster included:

   ```typescript
   defineEntity({ canonicalName: "rocket_launcher",
     components: { weapon: { projectile: { speed: 30, ghost: true /* ... */ } /* ... */ } } });
   ```

   A `ghost` that is not `true` or `false` stops the mod from loading, as any
   malformed descriptor field does.
2. **Movers** have no descriptor. A link's `ghost_tags` names the movers it
   ghosts, by their `_tags`. A mover listed by several links ghosts through
   each, at each link's mapping; a mover matching several tags of one link
   ghosts once through it.
3. **Member categories.** Each link shows four categories: `players`,
   `entities` (descriptor entities that are not players), `projectiles` and
   `movers`. `ghost_members` sets which it starts with; scripts change them
   during play.

Your own player, your own projectiles and your own sounds never ghost on your
own screen. Your partner sees and hears them.

Monsters placed in the map ghost only through their descriptor. Tags on a
placed monster do not opt it in: in co-op the other machines know a monster by
its descriptor, not by its map tags.

---

## How ghosts look

A **ghost style** is a named look your manifest declares under `ghostStyles`,
built with `defineGhostStyle`. A link names one with `ghost_style`.

```typescript
import { defineGhostStyle, defineMod } from "postretro";

export default defineMod({
  name: "MyMod", id: "example.my-mod", version: "1.0.0",
  ghostStyles: [
    defineGhostStyle({ name: "echo", tint: [0.6, 0.8, 1.0], opacity: 0.45, saturation: 0.2,
      audio: true, impactAudio: false }),
  ],
});
```

```lua
return defineMod({
  name = "MyMod", id = "example.my-mod", version = "1.0.0",
  ghostStyles = {
    defineGhostStyle({ name = "echo", tint = { 0.6, 0.8, 1.0 }, opacity = 0.45, saturation = 0.2,
      audio = true, impactAudio = false }),
  },
})
```

| Field | Type | Default | What it does |
|-------|------|---------|--------------|
| `name` | string | **required** | What a link's `ghost_style` names. |
| `tint` | `[r, g, b]`, each 0–1 | `[1, 1, 1]` | Multiplies the ghost's colour. |
| `opacity` | 0–1 | `0.5` | How solid the ghost is. |
| `saturation` | 0–1 | `0` | How much of the member's own colour survives: 1 keeps full colour, 0 is grey. |
| `audio` | boolean | `false` | Members' sounds also play at their ghosts (below). |
| `impactAudio` | boolean | `false` | Impacts of members' shots ghost too. Needs `audio`. |

A ghost is **unlit**: its colour comes from the member's own texture, its
saturation scaled, multiplied by the tint, drawn at the style's opacity. The
other copy's lights never touch it, it casts no shadow, and walls in front of
it hide it. It is drawn when the spot it appears at is in view, wherever the
real member is.

A link with no `ghost_style` uses the engine's **floor style**: grey (white
tint, saturation 0), half opacity, silent. It is meant to read as a ghost
against any lighting, not to be a game's look; declare your own. A style name
no declaration has also falls back to the floor style, and the log warns once
for that link.

Styles are checked like fluids: an entry with one bad field is dropped whole,
with a warning naming `ghostStyles[i].<field>`, and links naming it get the
floor style; a second entry with an existing name is dropped and the first
stands. Each machine resolves styles from its own manifest, and a hot reload
that changes a style changes every link naming it at once.

### Blend or dither

How every ghost is made translucent is a mod-wide choice in the manifest's
render profile:

```typescript
render: { ghosts: "dither" },
```

- `"blend"` (the default) blends the ghost over what is behind it.
- `"dither"` draws a fine fixed screen pattern instead: each pixel is either
  the ghost or the scene. Opacity rounds to the nearest sixteenth of a 4×4
  pattern, and the limiter judges that coverage. It suits a retro look.

Any other value logs a warning that `render.ghosts` must be `"blend"` or
`"dither"`, and ghosts blend; the rest of the render profile stays as written.

In both modes a ghost draws only its nearest surface, so its inner surfaces,
such as teeth behind closed lips, never show through. Ghosts draw farthest
first, so a nearer ghost blends over a farther one.

Ghosts sort as whole objects, not surface by surface. A ghost standing on or
inside a moving ghost, such as a platform, a lift or a cab, layers correctly
with it. Two ghosts that pass through each other, or a ghost close beside a
large moving ghost without standing on it, can hide each other where they
overlap, as if solid there.

Ghosts of projectiles drawn as sprites blend in either mode and sit outside
that ordering, like any sprite.

### Flashing and the photosensitivity limiter

Ghosts can never strobe past the player's photosensitivity limiter
(`accessibility.flashLimiter`), and no script can get around it:

- A ghost never pops. Turning a link on or off, changing its categories, and a
  member stepping in or out of the region all fade the ghost at a fixed rate,
  reaching full opacity within a quarter of a second. Members of a link that is
  on when the level starts appear at full strength on the first frame.
- With the limiter on, a ghost presents at most three flashes in any second,
  judged as it would draw over any background and any texture from black to
  white: a link toggled rapidly, a member jittering across the boundary, a
  colour filter flicked between bright and dark or into red, or a projectile
  sprite's own opacity flickering. Past that it holds its look until the
  second clears, though a fade-out always goes through. Ordinary reveals and
  slow fades are never held.

Design reveals as fades; a strobing set-piece simply will not strobe for a
player who has the limiter on.

---

## Ghost audio

When a link's style sets `audio`, a sound your machine plays at a member inside
the region also plays a second time at the member's ghost, through the **Ghost
bus**. The bus sits under the SFX volume and the underwater muffle, and filters
its echo so it reads as coming from elsewhere.

- The echo plays where the ghost was when the sound started, and plays out even
  if the member leaves or the link turns off.
- With `impactAudio` too, the impacts of a member's shots ghost as well,
  wherever they land inside the region.
- At most 16 echoes play at once, on their own budget, so they never crowd out
  sounds a player reacts to.
- Sounds placed at a fixed point rather than at a member, such as a
  teleporter's arrival sound, never ghost.

The manifest's `audio.ghost` block sets the bus's mix for the whole mod:

```typescript
audio: { ghost: { volume: 0.7, highPassHz: 250, lowPassHz: 3000, drive: 0.3 } },
```

| Field | Range | Floor value | What it does |
|-------|-------|-------------|--------------|
| `volume` | 0–1 | `0.6` | The bus volume, on the same curve as the player's volume options. |
| `highPassHz` | 20–20000 | `200` | Cuts the echo's lows below this. |
| `lowPassHz` | 20–20000 | `4000` | Cuts the echo's highs above this. Must stay above `highPassHz`. |
| `drive` | 0–1 | `0` | Soft distortion; 0 is clean. |

Every field is optional. A malformed field — out of range, not a number, or a
`highPassHz` at or above `lowPassHz` — warns, naming the field, and the whole
bus uses the floor values.

---

## Scripting

### Finding links

`getMapEntities("ghostLink", { tag })` returns the map's links carrying that
tag, in map order. Each member has `id`, `position` and `tags`, and these
methods, each returning steps to spread into a reaction's `sequence`:

| Method | What it does |
|--------|--------------|
| `arm()` | Turns the link on. |
| `disarm()` | Turns it off. |
| `setMembers({ players?, entities?, projectiles?, movers? })` | Shows or hides categories. An omitted category keeps its value. A category only narrows: it never ghosts what a descriptor or `ghost_tags` did not opt in. |
| `setColorFilter({ tint?, opacity?, saturation?, periodMs? })` | Changes the link's look. Each named field fades from its current value to the new one over `periodMs` (default 0, at once); an omitted field keeps its value and any fade in progress. A field set this way no longer follows the style. |

In Luau, call them as `l:arm()`, `l:setMembers({ ... })` and so on.

The ghost bus mix changes with `setGhostAudio`, from the `postretro/audio`
module. It names no target, so it is a whole reaction body, like
`setSentiment`:

```typescript
import { setGhostAudio } from "postretro/audio";
const swell = defineReaction("echo.swell", setGhostAudio({ volume: 0.9, lowPassHz: 5000, periodMs: 1500 }));
```

```lua
local Postretro = require("postretro")
local Audio = require("postretro/audio")
local swell = Postretro.defineReaction("echo.swell",
  Audio.setGhostAudio({ volume = 0.9, lowPassHz = 5000, periodMs = 1500 }))
```

It takes the `audio.ghost` fields plus `periodMs`. A field it sets fades on its
own and no longer follows the manifest; the rest keep following it. Unloading
the level returns the bus to the manifest's mix.

All of these apply on the host only, at the start of its next tick, in the
order they were called, and reach every player from there; on a connected
client they do nothing. An out-of-range value, a call naming no field, or a
`setGhostAudio` that would leave `highPassHz` at or above `lowPassHz`, warns
and drops that step. A non-finite value (NaN or Infinity) in any argument is
caught earlier: it warns when the level loads and drops the whole reaction, so
every other step in that reaction is lost too.

### A reveal, a fight and a swell

```typescript
import { defineReaction, getMapEntities } from "postretro";
import { setGhostAudio } from "postretro/audio";

export function setupLevel() {
  const echoes = getMapEntities("ghostLink", { tag: "vault_echo" }); // map order
  const reveal = defineReaction("echo.reveal", {
    sequence: [
      ...echoes.flatMap((l) => l.arm()),
      ...echoes.flatMap((l) => l.setColorFilter({ tint: [1.0, 0.4, 0.3], opacity: 0.6, periodMs: 1500 })),
    ],
  });
  const fight = defineReaction("echo.fight", { sequence: echoes.flatMap((l) => l.setMembers({ projectiles: false })) });
  const swell = defineReaction("echo.swell", setGhostAudio({ volume: 0.9, lowPassHz: 5000, periodMs: 1500 }));
  return {
    reactions: [reveal, swell, fight],
    triggerEvents: getMapEntities("trigger", { tag: "echo_plate" }).map((t) => t.on("enter", [reveal, swell])),
  };
}
```

```lua
local Postretro = require("postretro")
local Audio = require("postretro/audio")

function setupLevel(_ctx)
  local echoes = Postretro.getMapEntities("ghostLink", { tag = "vault_echo" })
  local revealSteps = {}
  for _, l in ipairs(echoes) do
    for _, step in ipairs(l:arm()) do table.insert(revealSteps, step) end
  end
  for _, l in ipairs(echoes) do
    for _, step in ipairs(l:setColorFilter({ tint = { 1.0, 0.4, 0.3 }, opacity = 0.6, periodMs = 1500 })) do
      table.insert(revealSteps, step)
    end
  end
  local reveal = Postretro.defineReaction("echo.reveal", { sequence = revealSteps })
  local fightSteps = {}
  for _, l in ipairs(echoes) do
    for _, step in ipairs(l:setMembers({ projectiles = false })) do table.insert(fightSteps, step) end
  end
  local fight = Postretro.defineReaction("echo.fight", { sequence = fightSteps })
  local swell = Postretro.defineReaction("echo.swell",
    Audio.setGhostAudio({ volume = 0.9, lowPassHz = 5000, periodMs = 1500 }))
  local triggerEvents = {}
  for _, t in ipairs(Postretro.getMapEntities("trigger", { tag = "echo_plate" })) do
    table.insert(triggerEvents, t:on("enter", { reveal, swell }))
  end
  return { reactions = { reveal, swell, fight }, triggerEvents = triggerEvents }
end
```

---

## Co-op

- Which links are on, their categories, and the looks and bus mix scripts set
  come from the host. A player who joins mid-level sees them as they are,
  mid-fade included.
- Styles, `audio.ghost` and descriptors' `ghost` fields come from each
  player's own mod. Players whose mods declare them differently see different
  ghosts, and can still play together.
- A script's first `setColorFilter` or `setGhostAudio` on a field fades from
  the host's style or `audio.ghost` value. A player whose mod declares that
  field differently sees it jump once to the host's value as the fade starts.
- A ghost moves exactly as its member does on that screen, so an arm and the
  movement it reveals appear together.
- Each player hears echoes only of sounds their own machine plays. A connected
  client hears the partner's shots and impacts ghosted, but not movement
  sounds, and on a client a mover opens silently, its ghost included.

---

## Compile errors and warnings

`prl-build` fails the build on any of these, naming the entity:

| Problem | Fix |
|---------|-----|
| `landmark` or `target` is blank or missing | Set both to destination `name`s. |
| `landmark` or `target` names no `teleport_destination` | Check the spelling against the destination's `name`. |
| `landmark` equals `target` | The link needs a point in each copy. |
| `enabled_on_spawn` is not `0` or `1` | Use `0` or `1`. |
| `ghost_members` names an unknown category | Use `players`, `entities`, `projectiles` or `movers`. |
| A `ghost_link` has no brushes | Tie at least one brush to it. |

These load, with one warning each when the level starts:

- a `ghost_style` no manifest entry declares (the link uses the floor style);
- a `ghost_tags` entry that matches no mover;
- a level with links while no descriptor sets `ghost`, so no pawn or other
  entity can ghost. The warning names what still can: movers a link's
  `ghost_tags` matched, and projectiles whose weapon sets `projectile.ghost`.
  With neither, it says nothing can ghost.

---

## Trying it

The SDK bundle's dev mod ships a two-copy fixture with both set-pieces:
`content/dev/maps/ghost-links.map` (its Luau twin is `ghost-links-luau.map`).
Copy A has a yellow barrier in its east doorway; copy B has a red button on its
north wall. The blue plate in either copy reveals the second link and swells
the echo. The first press of a purple pad sends whoever stands on it alone into
the other copy, so two players who start in the same copy can split up. Every
later press swaps both players' copies and wakes the monsters. The split stands
in for a game condition such as a timer or a health threshold, and it fires only
once, so press it alone from the middle of the pad. From the bundle root:

```bash
bin/prl-build content/dev/maps/ghost-links.map \
  --baked-root baked --cache-dir .build-caches/prl-cache \
  -o content/dev/maps/ghost-links.prl
bin/postretro-tool run maps/ghost-links.prl
```

Alone, press the purple pad to cross into copy B, press the red button, and
watch the barrier's ghost sink; press the pad again to return. For two players,
run a host and a client on one machine as in
[diagnostics.md](diagnostics.md). If both start in the same copy, one of you
presses the purple pad first to cross over alone.

```bash
# Terminal 1
bin/postretro-tool run maps/ghost-links.prl --host
# Terminal 2
POSTRETRO_NET_LAG=45,60,5 bin/postretro-tool run maps/ghost-links.prl --connect 127.0.0.1:27015
```

Compile your own maps the same way, with your mod's folder in place of
`content/dev`; see [Compiling Your Map](level_design.md#compiling-your-map).
