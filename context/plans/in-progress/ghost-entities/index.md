# ghost-entities

Brief · resumable · reads: `context/lib/movement.md` §6 (Teleport), `context/lib/networking.md` §Current contract, §What gates, and what replicates instead, §Version gates, `context/lib/rendering_pipeline.md` §4, §7.4, §7.8 (Photosensitivity limiter, Fluid view tint), `context/plans/ready/E23--accessibility/index.md` (I2, I9, D4), `context/lib/audio.md` §1, §4, §5, `context/lib/scripting.md` §1, §12, `context/lib/entity_model.md` §4 (Map declarations), `context/lib/build_pipeline.md` §PRL section IDs · read at d6f3471

## Problem
The game owner wants a new capability for The Dreadgate Trials, the first game on the engine: co-op players in the same room at different moments in time. Each player sees the partner in the other time as a ghost. A barrier one player cannot pass is watched opening from the other time when the partner hits a button there. In battles, each player sees only their own monsters plus the partner's ghost and aim, and a script swaps them so each faces what they could not see. The swap already exists: room copies joined by a seamless teleporter pair (`docs/teleporters.md` "A closet loop and a time shift"). The missing piece is perceiving the other copy. When done, a mapper places a `ghost_link` volume over one room copy, naming a landmark in it, a target in the other copy, the tagged movers it ghosts and a ghost style. A mod's descriptors declare which kinds of entity can ghost, and its manifest declares ghost styles: look and whether sound carries. While the link is armed, its members draw as translucent ghosts at the mapped pose in the other copy. When the style carries sound, their sounds also play there through a muffled ghost bus. Scripts arm links, narrow which kinds of member a link shows, and change look and ghost audio during play, with ramps.

## Decisions
- **Room copies, not phase tags.** Timelines are separate physical copies, so collision, hits, splash, AI perception and triggers stay isolated with no filter (none exists). Copies may differ in geometry and baked light. Ghosts are presentation only: they never collide, block, take hits or touch triggers. Rival rejected: tags in one shared space need a filter in every interaction path, and both timelines would have to share geometry and lighting.
- **No network work for the ghosted entities.** Members are player pawns, descriptor entities, projectiles and tagged movers. Every peer has all of them: networked entities replicate to every client wherever its pawn is, with no relevance filter, and each record carries the descriptor class a client resolves locally. Clients spawn movers, tags included, from the level file. Ghosts derive locally on every peer, the listen host included. Only link state and ghost audio state cross the wire.
- **Each layer answers one question** (`scripting.md` §1, "Engine owns the floor; scripts own the taste").
  - Descriptors say which kinds of entity can ever ghost. This is a game-design fact about an archetype, true through every link.
  - Map data says where, and which map-placed movers: volume, points, `ghost_tags`, the starting member categories, spawn state and style name.
  - The mod manifest says how ghosts present: named styles and the ghost bus profile.
  - Level scripts say what changes during play, including which member categories a link shows.
  - The link holds no raw look values or presentation flags, and the manifest names no map entity.
- **`ghost_link` is a brush entity that reuses `teleport_destination` points.** `landmark` names a point inside the source copy, `target` a point in the other copy. They must exist and differ, under the seamless `trigger_teleport` rules and error wording. Links carry `_tags` for script queries. A link is independent of teleporters, so a link can look into a copy nobody teleports to. A two-way link is two volumes. Like teleporter regions, a link is a baked region scripts arm, so it is its own amendment to `entity_model.md` §4's map declarations, recorded at promotion.
- **One mapping function, in physics beside the teleport resolve.** The seamless landmark → target transform becomes one shared pure function that teleports and ghosts both call, so a ghost and a seamless arrival from the same pose always agree. This follows the one-chokepoint rule for pawn relocation (`movement.md` §6).
- **Membership: an entity must be able to ghost, and the link must show its category.**
  - A descriptor's top-level `ghost` field, beside `canonicalName`, marks an archetype that can ghost. It is a descriptor attribute, not a component kind, and it defaults to false for every descriptor: a mod opts its player descriptor in like any other, so the engine never branches on actor kind. A level with links while no loaded descriptor sets `ghost` warns once at install, so a forgotten opt-in is not silent. A non-boolean `ghost` rejects mod init in both runtimes, as every malformed descriptor field does. A pawn ghosts with its third-person weapon.
  - A weapon's `projectile` block takes its own `ghost` field, default false. A projectile that opts in can ghost whoever fired it. Every peer resolves it from the firing weapon's descriptor, the host included, for a monster's projectiles as for a player's. The field is stripped from the host tuning payload like sound keys, so flipping it never rebuilds a client's pawn.
  - Movers have no descriptor: a link's `ghost_tags` names the movers carrying those `_tags`. A listed tag that matches no mover warns once at install.
  - Each link shows four member categories: `players`, `entities` (non-player descriptor entities), `projectiles` and `movers`. A category only narrows: it never ghosts what the descriptor or tag did not opt in. The FGD key `ghost_members` sets a link's starting categories, all four by default, and `setMembers` changes them during play.
  - An impact belongs to a link when its shot's owner is a member, resolved from the shot id normalized the way observer cues normalize it. An impact without a shot id belongs to no link.
  - The viewing player's own pawn, its projectiles and its sounds never ghost on that viewer's machine.
- **Ghost styles are named manifest declarations, following fluids.**
  - `ModManifest.ghostStyles` lists `defineGhostStyle` entries: `name`, `tint`, `opacity`, `saturation`, `audio`, `impactAudio`. `impactAudio` takes effect only with `audio`.
  - A link's `ghost_style` names a style. Each peer resolves it locally, by name, at level install and on each committed staged reload, as fluid volumes resolve.
  - Validation follows fluids: a malformed entry or a later duplicate name drops with a warning naming `ghostStyles[i].<field>`, and the first declaration stands.
  - A link naming no style uses the engine floor style. A name no style declares also uses it, warning once at install.
  - Styles stay out of the mod compatibility digest and off the wire. Peers whose declarations differ see different ghosts, the visibility difference co-op accepts for fluid look (`networking.md` §What gates, and what replicates instead).
- **The floor style is legible and neutral, not a game's taste.** It has saturation 0, a neutral tint and translucent, yet readable against both copies' lighting and under the high-contrast theme. It carries no sound. The floor ghost bus profile is band-limited, quieter than SFX and undistorted. A game's look, Dreadgate's "punchy but muffled" included, is a style and a bus profile in its own manifest.
- **Future relevance must treat an armed link as a coupling.** Any relevance, cue-recipient or audible-set filter that lands later gets it through the cell-visibility relation, never by knowing about links (`context/research/cell-visibility-substrate.md`, generalizability contract). The relation is symmetric by contract, so an armed link couples the cells under its two volumes both ways. That over-includes the reverse direction, which the substrate accepts as safe. The shipped relation stores connected-component ids, and folding a link into a component would make the coupling permanent. So the coupling waits for the relation's runtime-masked couplings, and nothing is built now. At promotion it is recorded in `build_pipeline.md` beside id 46 as an obligation on that extension.
- **Membership is evaluated per frame from presented state.**
  - An eligible entity is a target member while the link is armed, its category is shown, and its presented point lies inside the volume: the body point for pawns, the origin for projectiles, the current bounds center for movers.
  - The ghost draws at the mapped presented pose, so aim pitch, upper-body yaw, pose modifiers and teleport cuts carry over.
  - A ghost whose membership ends, its entity's despawn included, fades out at its last in-volume mapped pose. It never draws at the mapping of a pose outside the volume, so a teleport swap draws no ghost between copies.
  - An entity inside two armed links ghosts through each, one transform per link. A mover matching several of one link's tags ghosts once through it.
  - At level install, members of links armed on spawn present at full membership on the first frame.
- **Descriptor flags resolve locally, like styles.** Each peer reads `ghost` from its own descriptors by the class the entity was materialized from, at install and on each committed reload, which re-resolves membership through the membership ramp. Peers whose descriptors differ see different ghosts, as with styles. The flag never rides the wire and never enters a digest.
- **Ghost draw.**
  - Unlit: the base color's saturation is scaled by `saturation` (1 keeps full color, 0 is grey), then multiplied by `tint`, and drawn translucent at the presented opacity.
  - Depth-tested against the scene, never writing scene depth; no shadows. Skinned and mover ghosts draw farthest first in one pass, into a ghost-only depth target seeded from scene depth. Each ghost writes its own nearest surface, then draws its colour there, so it hides its own inner surfaces and a nearer ghost blends over a farther one. Ghosts sort as whole objects by their centres, and a mover ghost draws just before any ghost standing in it (on a platform, on a lift, or in a cab). Ghosts that pass through each other, or one close beside a large mover ghost without standing in it, can still hide each other where they overlap. Billboard ghosts stay on scene depth. Owner decision after the playtest, 2026-10-22; per-ghost order from review round 4; contact-based sort from review round 6.
  - Culled at the mapped position against the viewer's visible cells, whatever the real position's visibility.
  - A projectile ghost gets the same treatment on its billboard or model body.
  - Transparency is mod-wide: `ModManifest.render.ghosts` is `"blend"` (alpha blend, the default) or `"dither"` (screen-door: a screen-space ordered pattern keeps or discards each pixel at the presented opacity). It follows the render profile: a malformed value warns and uses blend without touching `bloom` or `reflections`, a successful staged mod-init replaces it, and it stays local, off the wire and out of every digest. A projectile billboard ghost blends in both modes. Owner amendment, 2026-10-21.
  - The renderer owns the pipelines; game logic decides membership.
- **Ghosts pass the photosensitivity limiter.** E23 I2 counts a primitive that changes presented luminance without a clamp as a violation, and `setColorFilter` and arm/disarm are such primitives.
  - A ghost's presented opacity is its link's opacity times its membership. Membership ramps linearly at the limiter's rate cap on arm, disarm, a category change, entry and exit, reversing from the presented value. As with fluid tint entry, ordinary reveals are never limited and a strobe is.
  - Each ghost is judged by what it draws over every background and texture: its drawn value over any albedo and background is sampled along the edges of that square, on one flash window per ghost that also counts its tint's redness under the fluid view tint's red rule, behind the player-only toggle. Like the screen channels, it judges each change as if it covered the screen (`rendering_pipeline.md` §7.8). A projectile sprite ghost's own opacity is part of what it draws, so it is judged too. Over budget the ghost holds a look no further counted change can leave, and a fade-out always presents.
  - Provisional owner amendment, 2026-10-21 (tint brightness limited too), revisited at the manual legibility check. Owner decisions on 2026-10-22 and 2026-10-23 extend it to drawn brightness, then every background, then the edge-sampled model (limiter v2, plan.md Delegated answers). Keeping a tint's hue keeps its hue angle: a tint that scaling can't bring to a held brightness mixes toward white. Saturation is not limited: it changes drawn luminance only through a coloured tint on a coloured texture.
  - A new ghost's membership starts at 0. Turning the limiter on starts every ghost fresh against its current value, as the screen channels start, so enabling it never dips a ghost. The membership ramp runs on presented-frame time, so it continues while script time is frozen (the dev `freeze_time`), while `setColorFilter` ramps follow script time.
  - The clamp applies on the presenting machine to the presented value, and nothing it holds reaches a script or the wire (I9).
  - The keyed clamp container (`KeyClamp`) and the per-value clamp extracted from the screen-channel clamp (`ValueClamp`) are reusable, so `drafts/E23--photosensitivity-source-floor` can reuse them for every other source. The per-ghost look clamp built on them is ghost-specific (limiter v2; reworded with the owner, 2026-10-23).
- **Look resolves per link, per field:** floor style, then the named style, then `setColorFilter`. `setColorFilter` ramps each field it names independently. An omitted field keeps its value and any ramp in flight. Presentation flags come from the style alone and never change at runtime.
- **Ghost audio mirrors voices this machine plays.**
  - When a link's style sets `audio`, a sound this machine plays anchored at a member inside the volume also plays a second voice. The voice sits at the mapped fire-time point on the Ghost bus. Admission reads the fire-time point and armed state, not the presented ramp.
  - Impact sounds join under the membership rule when the style sets `impactAudio`, and contacts outside the volume drop.
  - A ghost voice plays out when its member leaves or despawns or its link disarms. The real voice is untouched.
  - The Ghost bus sits under SFX, so SFX volume and world muffle apply. It carries volume, high-pass, low-pass and distortion drive, with mod-wide parameters, because the bus is one shared mix.
  - Coverage follows what each peer already plays: a ghost voice starts only beside a real voice this machine started. Mover sounds play only on the host, and a connected client hears only the partner's fire and impacts, so it ghosts only those. A client's barrier opens silently, real and ghost alike. Teleport arrival sounds are point-anchored and never ghost.
- **Ghost voices have their own cap of 16**, so the echo never crowds out sounds a player reacts to. The figure is half the SFX cap and fits the audio manager's capacity without raising it.
- **The ghost bus profile resolves:** floor profile, then manifest `audio.ghost`, then `setGhostAudio`. A malformed `audio.ghost` field applies the whole floor profile with a warning naming the field, matching `audio.attenuation`. Level unload and demotion return the bus to the resolved manifest profile on every peer. A resolved profile whose high-pass reaches its low-pass, after a reload or against another peer's profile, presents the floor cutoffs and warns once.
- **Link state and ghost audio replicate as always-on host state, the teleporter precedent.**
  - `arm`, `disarm`, `setMembers`, `setColorFilter` and `setGhostAudio` apply on the host only, at its next tick, in call order, and do nothing on a connected client.
  - Each link replicates armed state, its member categories, and the fields `setColorFilter` has set, each with its ramp. One level-scoped record replicates the fields `setGhostAudio` has set. Fields no script has set stay local.
  - A ramp's from-value is the unclamped value at the applying host tick. A field's first set therefore carries the host's style or manifest value as its from-value, and a client whose own declaration differs steps once to that ramp, the style divergence co-op already accepts (owner decision, 2026-10-22).
  - Each ghost reads its link's armed state, categories and look at the host time its member presents on that machine: the remote-pawn delay for pawns, on the host seat and on clients alike, and the mover clock for movers. An arm and the motion it reveals therefore present on one frame. A ghost voice reads armed state the same way, for its member, at the frame the voice starts.
  - Until a link's first record lands, a joiner presents its local spawn state and style. Demotion and level unload drop received state, and the client returns to local spawn state through the membership ramp. A record from before a participation reset never binds.
  - This diverges from the bound-presentation lane, which replicates only after a host-only write and lets every-machine origins present locally. Link state needs one authority and a late-joiner baseline, and a client-local early apply would double-apply against the replicated record.
- **`postretro/audio` is a new SDK namespace, and `setGhostAudio` is its first member.** It is named for the audio subsystem, as the manifest `audio` block, the audio crate and `audio.md` are. Upcoming music work, such as reactions adding and removing music layers, lands there too. It ships as a virtual module in TypeScript and Luau, beside `postretro/ui`. `setGhostAudio` is not also exported from the root `postretro` module.
- **The look setter is a color filter.** `setColorFilter` names the tint, opacity and saturation stack the way image-editor filter layers do, and stays a single call with per-field ramps.
- **Off the level content digest.** Links and their keys are presentation, not a prediction input (`networking.md` §Level identity and level content digest).
- **First consumer ships with it.** A dev map holds two room copies, a two-way link pair and a two-way seamless pair, with both set-pieces: the barrier puzzle, with the barrier a tagged mover, and the battle swap. The dev mod's manifest declares a sound-carrying style and a ghost bus profile. TypeScript and Luau scripts follow `teleporters.map` / `teleporters-luau.map`. An author section joins `docs/teleporters.md` or a new `docs/ghost-links.md` (executor's call).
- **Non-goals.**
  - Phase or timeline tags and interaction filtering.
  - Per-instance ghost tags on descriptor placements: on a connected client they are snapshot display copies with no tags or map identity. Their descriptor opts them in instead, and replicating per-instance identity is its own wire change.
  - Flipping `ghost` on individual entities at runtime: it needs a per-entity ghost record and setter on the presentation-override lane that lights, fog and emitters use. `setMembers` covers per-scenario rules; add it when a set-piece needs one entity to differ from its archetype.
  - Widening a link past its opt-ins, such as ghosting a descriptor that did not set `ghost`: the game declares what is ghostable, and links only narrow.
  - Ghost impact sparks: they would mark a hidden target's position on screen. A game that wants impact hints opts into impact audio.
  - Hitscan tracers and muzzle flashes: no peer sees them today, even unghosted.
  - Remote movement sounds, and host-only mover and reload sounds on clients: networked peer audio is roadmap Epic 12 step 4.
  - Suppressing the real voice: the author doc tells mappers to place copies farther apart than the attenuation `maxDistance`.
  - Per-link or per-style filter chains: one shared bus keeps the mix and the voice budget simple.
  - Switching a link's style at runtime: `setColorFilter` covers look changes during play.
  - Moving `playSound` and the attenuation types into `postretro/audio`: it breaks existing scripts, so it is its own follow-up ahead of the music-layer work.
  - World geometry ghosts: the other copy's architecture is the copy itself.
  - Lit ghosts: unlit keeps the ghost legible and avoids sampling light at the mapped position.

### Scripting surface
```typescript
// start-script.ts: mod-wide presentation
import { defineEntity, defineGhostStyle, defineMod } from "postretro";

export default defineMod({
  name: "Dreadgate", id: "dreadgate", version: "0.1.0",
  // Every field but name optional; omitted fields take the floor style's value. Colors follow fluids: [r, g, b].
  ghostStyles: [
    defineGhostStyle({ name: "echo", tint: [0.6, 0.8, 1.0], opacity: 0.45, saturation: 0.2,
      audio: true, impactAudio: false }),
  ],
  // Descriptors say what can ghost. `ghost` defaults to false everywhere, players included.
  entities: [
    defineEntity({ canonicalName: "player", ghost: true, components: { /* movement, health, ... */ } }),
    defineEntity({ canonicalName: "rocket_launcher",
      components: { weapon: { projectile: { speed: 30, ghost: true /* ... */ } /* ... */ } } }),
    defineEntity({ canonicalName: "phase_wraith", ghost: true, components: { /* health, behavior, ... */ } }),
  ],
  // Every field optional; a malformed field applies the whole floor profile.
  audio: { ghost: { volume: 0.7, highPassHz: 250, lowPassHz: 3000, drive: 0.3 } },
  // Mod-wide ghost transparency, beside bloom and reflections. "blend" is the default.
  render: { ghosts: "dither" },
});
```
```typescript
// Level script: changes during play
import { defineReaction, getMapEntities } from "postretro";
import { setGhostAudio } from "postretro/audio";

export function setupLevel() {
  const echoes = getMapEntities("ghostLink", { tag: "vault_echo" }); // map order
  const reveal = defineReaction("echo.reveal", {
    sequence: [
      ...echoes.flatMap((l) => l.arm()), // also: l.disarm()
      // Each named field ramps on its own; omitted fields keep their value and ramp. periodMs defaults to 0.
      ...echoes.flatMap((l) => l.setColorFilter({ tint: [1.0, 0.4, 0.3], opacity: 0.6, periodMs: 1500 })),
    ],
  });
  // Narrow what a link shows for one scenario; omitted categories keep their value.
  const fight = defineReaction("echo.fight", { sequence: echoes.flatMap((l) => l.setMembers({ projectiles: false })) });
  // A mod-wide primitive with no target is a whole reaction body, like setSentiment.
  const swell = defineReaction("echo.swell", setGhostAudio({ volume: 0.9, lowPassHz: 5000, periodMs: 1500 }));
  return {
    reactions: [reveal, swell, fight],
    triggerEvents: getMapEntities("trigger", { tag: "echo_plate" }).map((t) => t.on("enter", [reveal, swell])),
  };
}
```
- `defineGhostStyle` is a pure identity helper, like `defineFluid`. `setColorFilter`, `setMembers`, `arm` and `disarm` return `SequenceStep[]`. `setGhostAudio` returns a primitive reaction body.
- Each member carries `id`, `position` and `tags`.
- Ranges: `tint` components, `opacity`, `saturation`, `volume` and `drive` lie in 0..=1, and filter cutoffs in 20..=20000 Hz. `volume` maps on the existing perceptual taper.
- An out-of-range or non-finite script argument, a non-boolean `setMembers` field, or a call naming no field, warns and drops that step or reaction. So does a `setGhostAudio` whose targets would leave `highPassHz` at or above `lowPassHz`.
- The Luau mirror ships: `defineGhostStyle({...})`, `l:arm()`, `l:setColorFilter({...})`, `l:setMembers({...})`, and `setGhostAudio({...})` from `require("postretro/audio")`.

## Acceptance
Pins are in `research.md`.
### Automated
**Compile and load**
- [ ] A map with a `ghost_link` round-trips its records, tags, ghost tags, member categories and style name through the PRL section. A link without `ghost_members` round-trips all four categories.
- [ ] A missing or unknown `landmark` or `target`, a `landmark` equal to `target`, a link with no brushes, a bad `enabled_on_spawn` and an unknown `ghost_members` category each fail with the author-facing error.
- [ ] A `ghost_tags` entry matching no mover warns once at level install on host and client alike, and the level loads.
- [ ] Decode rejects each case the Wire format names, for the PRL section and for snapshot link and audio records alike.
- [ ] Adding a link as the map's last entity leaves the level content digest unchanged, while adding a teleporter still changes it.
**Styles**
- [ ] A link naming a declared style presents that style's values and flags. A link naming no style presents the floor style with no warning. A link naming an undeclared style presents the floor style and warns once at install.
- [ ] A style with one malformed field drops whole with a warning naming `ghostStyles[i].<field>`, and links naming it get the floor style. A later duplicate name drops with a warning, and the first declaration stands. Both runtimes judge the same entries the same way.
- [ ] A committed staged reload that changes a style re-resolves every link naming it, armed or not, mid-ramp or not. Fields set by `setColorFilter` keep their values and ramps (P16).
- [ ] The floor style carries no sound, and the floor bus profile applies no distortion.
- [ ] With `render.ghosts` absent or `"blend"`, ghosts alpha-blend; with `"dither"` they draw screen-door. A malformed `render.ghosts` warns and uses blend, leaving `bloom` and `reflections` as authored, and both runtimes judge it alike. A projectile billboard ghost blends under both.
- [ ] Two peers whose manifests declare one style name with different values each present their own, and the session admits both. After an opacity-only `setColorFilter`, both present the host's opacity and still their own tint and saturation (P15).
**Mapping and membership**
- [ ] For rotated and unrotated pairs, with vertical offset, a ghost pose equals the pose a seamless teleport through the same points produces.
- [ ] Membership edges, each on host and on a connected client:
  - presented point just inside vs just outside the volume, for a pawn's body point, a projectile's origin and a mover's bounds center
  - partner pawn ghosted while an untagged mover and a monster whose descriptor does not set `ghost` beside it are not
  - a mover carrying a listed tag ghosted, and a monster whose descriptor sets `ghost` ghosted
  - a player descriptor that omits `ghost` puts no pawn in any ghost set, and a level with links and no ghost-opted descriptor warns once at install
  - a spawner releasing a ghost-opted descriptor inside an armed link ghosts the spawn, ramping in from 0
  - the same tagged barrier inside two links ghosts through the link that lists its tag and not the other
  - a projectile whose weapon block sets `ghost` ghosted and one without not, whoever fired it; a ghost-opted projectile fired by an unghosted monster ghosts
  - the viewer's own pawn, projectiles and sounds absent from its own ghost set and ghost voices, while the partner sees and hears them
  - a mover inside two armed links that both list its tag draws two ghosts at two transforms (P27), and a mover matching two tags of one link draws one (P39)
  - a ghost pawn's pose matches the mapped presented pose, aim pitch and third-person weapon included
- [ ] `arm` on tick N and `disarm` on tick N+1 ramp presented membership up and back down at the rate cap. `arm` and `disarm` in one tick start no ramp and present no ghost on any peer (P2, P3). On a connected client the one-tick span presents at most one rise and fall, each at the rate cap (P35). An arm during a ramp-out, or a re-entry mid-fade, ramps from the presented value (P4).
- [ ] A ghost draws when its mapped cell is visible and not when it isn't, both with the real cell visible and without.
- [ ] Two pawns swapped on one tick by arming both seamless teleporters: each old ghost fades at its last in-volume mapped pose, each new ghost starts on the cut frame and fades in, and no frame draws a ghost between the copies (P1).
- [ ] A link arm and a remote pawn's seamless swap on one host tick present on one frame, on a connected client and on the host seat (P18). A link arm and a member mover's motion on one host tick present on one client frame (P33).
- [ ] An entity slot freed and re-tenanted on one frame starts the new tenant's ghost at membership 0 (P19). A member's projectile ghost ramps in from spawn and fades out at its last pose on despawn. A ghost-opted projectile keeps ghosting while its origin is inside after its owner leaves the volume, and that projectile's impact is not ghosted (P20, P22).
**Member categories**
- [ ] With one category off, that category's eligible members fade out at the rate cap and the other categories' members stay, for each of the four categories. A category on never ghosts an entity whose descriptor or tag did not opt in (P43).
- [ ] A link's `ghost_members` map default holds from the first frame. `setMembers` off then on in one tick ends in the last call's state with no ramp. `setMembers` and `arm` on one tick present together. A category flipped mid-ramp reverses from the presented value (P44, P45).
- [ ] A committed reload that changes a descriptor's `ghost` re-resolves membership through the membership ramp on every peer (P46). A host reload flipping only a weapon's `projectile.ghost` changes ghosting on a client without rebuilding its pawn.
- [ ] A non-boolean `ghost` or `projectile.ghost` rejects mod init with the same error in both runtimes.
- [ ] A ghost is never a hit, overlap, trigger or AI target: a hitscan through it records no hit on the ghost, a splash centered on it damages nothing through it, a trigger it overlaps does not fire, and an enemy facing it acquires nothing.
**Photosensitivity**
- [ ] With the limiter on and a style at opacity 1, each of these presents at most 3 flashes in any 1 s window and rests at its pre-flash level over budget:
  - a script toggling `arm`/`disarm` at 10 Hz
  - `setColorFilter` opacity alternating 0 and 1 at `periodMs` 0
  - `setMembers` toggling a category at 10 Hz
  - a member oscillating across the volume boundary at 10 Hz
- [ ] With the limiter on, a red `setColorFilter` tint strobe desaturates over budget, keeping luminance, while a single red tint change keeps its hue.
- [ ] With the limiter on and opacity steady, a `setColorFilter` tint alternating white and black at `periodMs` 0 presents at most 3 flashes in any 1 s window on any background and rests at a held state (provisional amendment; owner restatement, 2026-10-23).
- [ ] With the limiter on, a single `arm` reaches full style opacity within 250 ms, and the example's 1500 ms `setColorFilter` fade presents exactly as with the limiter off. A new ghost's first presented frame starts from membership 0 (P21). Turning the limiter on while ghosts present dips none (P31).
- [ ] With the limiter turned off by the player, the 10 Hz `arm`/`disarm` strobe presents unclamped, the membership ramp still applies, and a hitch frame steps at most the hitch ceiling (P30). When script time is frozen (the dev `freeze_time`), `setColorFilter` ramps hold while the membership ramp continues (P42). Owner restatement, 2026-10-22: the shipped pause menu keeps script time running.
**Look and audio values**
- [ ] `setColorFilter` overrides the style field by field. An opacity-only `setColorFilter` mid tint ramp leaves the tint ramp running to its target (P5). Two `setColorFilter` calls in one tick apply in order, the later winning per field (P7). A `setColorFilter` or `setGhostAudio` naming no field warns and changes no ramp (P41).
- [ ] A malformed `audio.ghost` field applies the whole floor profile and warns naming the field. A `highPassHz` at or above `lowPassHz` counts as malformed.
- [ ] A `periodMs` ramp presents the midpoint value halfway through, and a second set mid-ramp starts from the unclamped value at that moment. `periodMs` 0 lands in one frame. A `setGhostAudio` whose targets cross the cutoffs warns and drops (P29). A resolved profile whose cutoffs cross after a reload, or a client whose own `audio.ghost` crosses against host-set fields, presents the floor cutoffs and warns once (P36). A non-finite or out-of-range `setColorFilter` or `setGhostAudio` argument warns and drops.
- [ ] A fire sound anchored at a ghosted pawn admits a second voice at the mapped fire-time point under a style with `audio` true, and none under one with `audio` false. A sound fired just inside the volume during a ramp-out still admits; one fired just outside does not (P24). A mover's open sound ghosts on the host under a sound-carrying style. A member's sound refused at the SFX cap admits no ghost voice (P40). On a client, a fire cue arriving just after an arm admits by armed state as the shooter's ghost presents it at that frame (P34).
- [ ] The viewer's own fire admits no ghost voice on its machine (P25).
- [ ] An impact sound is ghosted only under a style with `audio` and `impactAudio` both true, from a shot whose owner is a member. The host's own impact is never attributed to another entity. An impact without a shot id is never ghosted. Contacts outside the volume drop.
- [ ] A ghost voice plays out at its mapped point after its member despawns or its link disarms (P23). Ghost voices follow SFX volume and world muffle.
- [ ] The seventeenth live ghost voice is refused while a real SFX sound requested in the same frame still plays. With the Ghost track present, SFX keeps headroom for two full caps of voices, and the ghost pool keeps its own.
- [ ] On the mock backend, a tone through the Ghost bus under the dev profile measures attenuated below the high-pass and above the low-pass cutoffs, and changes after `setGhostAudio`. Ghost voices release their slot on finish, stop and level unload.
- [ ] After `setGhostAudio`, a level unload or demotion returns the Ghost bus to the manifest profile on host and client (P13). A staged reload changing `audio.ghost` re-resolves the bus, and `setGhostAudio` values stand (P17).
**Replication**
- [ ] Host `arm`, `setMembers`, `setColorFilter` and `setGhostAudio` reach a connected client, and the client's ramp ends within one snapshot interval of the host's. Repeated identical records do not move the presented value as RTT changes (P8).
- [ ] A late joiner, including one joining mid-ramp, gets current armed state, member categories, `setColorFilter` fields and `setGhostAudio` fields, not its local spawn state (P9, P12). A record landing before the presentation clock applies armed state and defers the ramp (P10). A record naming an uninstalled link binds nothing, and a stale old-level record never binds a new link, the host restarting the same map included (P11, P14, P37). A client demoted mid-level returns to local spawn state through the membership ramp (P38).
- [ ] A mid-ramp `setColorFilter` carries the unclamped host value: a host with the limiter on and a client with it off present the same ramp, each through its own limiter (P6).
- [ ] The same steps run on a connected client change nothing.
- [ ] Demotion and level unload drop received link and audio state.
- [ ] A handshake across the old and new wire versions is refused.
**Surface fixture**
- [ ] The dev mod's manifest and the dev map's TypeScript and Luau scripts, which contain the Scripting surface examples, load and run with zero warnings, `setGhostAudio` resolving from `postretro/audio` in both runtimes and the generated declarations. The `ghostLink` query returns both dev-map links, and their set-pieces drive the arm, look and audio changes the rows above observe (P28).
- [ ] A headless frame capture of the dev map's tagged barrier on a link armed on spawn, at rest, shows tinted, blended pixels with the geometry behind still visible and no ghost shadow (P26, P32). The ghost's pixels are identical under two different dynamic-light setups, and the part behind a nearer wall does not draw. Under `render.ghosts: "dither"`, the same capture shows unblended ghost and scene pixels interleaved, with the ghost's pixel coverage over its footprint within tolerance of the presented opacity.
### Manual
- [ ] Two-peer loopback co-op on the dev map, run once as listen host and once as client, at the harness lag profile:
  - Barrier puzzle: one player presses the button in copy B, sees the ghost barrier open, and the player in copy A passes.
  - Battle swap: each player sees only their own monsters plus the partner's ghost, aim pitch and weapon. After the swap each faces the other set.
- [ ] The floor style reads as a ghost against both copies' lighting and under the high-contrast theme. Record the chosen floor values.
- [ ] The dev mod's sound-carrying style and bus profile make the partner's fire read punchy and muffled, and the scripted swell is audible.

## Path
- Thin slice first: a link section, local membership, one floor-style translucent ghost of the partner pawn, and a two-peer loopback swap. It proves the presented pose, the teleport cut, mapped culling and the client arm timing compose before any surface fans out.
- Precedents and seams are in `research.md`. The ones that change a plan:
  - Teleporters, for compile, section, install and replication.
  - `defineFluid`, for styles.
  - `setSentiment`, for a target-less reaction body.
  - `resolve_authored_attenuation`, for the bus profile.
- Rendering: translucent variants of the skinned-mesh and kinematic-brush passes, with game logic handing the renderer extra instances and look values. Mover and skinned instance seams: `research.md` §Rendering.
- Limiter: extract the private per-channel tracked value from `flash_clamp.rs` (`research.md` §Client-side facts).
- Audio seams (bus-aware voice release, the SFX child-track slot, `Point` anchors, kira's built-in effects and the dB drive): `research.md` §Audio, §Audio capacity.
- Descriptor attribute: the host must record a weapon class on every gameplay projectile it spawns, a monster's included, before it can read `projectile.ghost` (`research.md` §Client-side facts). Remote entities and pawns materialize from local descriptors by `entity_class`, and projectiles by their weapon's `projectile` block (`research.md` §Client-side facts). Read `ghost` there, and keep it out of the networked-by-default component classification.
- Impact and projectile owners on each role: `research.md` §Client-side facts. Normalize before reading membership.
- Replication: links register at level install like teleporters. Look ramps rebase with the host-time rule bound presentation state uses, then present at each member's own clock (`research.md` §Presentation clocks).
- Rivals weighed:
  - Ghosts as registry display copies: they enter hit, trigger, overlap and AI paths, and would need the filters room copies avoid.
  - Per-link look keys and flags: every link would repeat a game's taste.
  - Script-set membership at arm time: map data keeps membership off the wire.
  - A per-entity `ghost` KVP: it cannot vary by link, and clients cannot read it on descriptor placements.
  - Each link listing the descriptor and weapon classes it ghosts, as `ghost_tags` lists movers: clients could resolve it, and it tells rockets from plasma per link. Rejected because whether an archetype can ghost is a fact about the archetype, true through every link, and class lists would repeat across links. Category narrowing covers per-scenario rules.
  - Projectile ghosting as a style flag: it puts an archetype fact in the presentation layer, and it cannot tell rockets from plasma.
  - Two briefs, visuals now and ghost audio later: rejected because the owner treats ghost audio, `setGhostAudio` included, as core to designing the experience.
  - Screen-door (dithered) transparency instead of alpha blending: kept as a mod-wide render-profile option beside blend (Decisions, Ghost draw). It suits the retro look; the limiter judges the coverage the screen door draws.
- Namespace: `postretro/ui` is the precedent for a virtual module: `luau_require.rs`, `luau_prelude.rs`, `staged_manifest.rs`, `typedef/luau.rs` and the `ui_sdk_module.d.ts` template. Dev `tsconfig.json` files map module ids to the generated declarations. The script compiler's specifier allowlist and build-side Luau `require` also need the new id (`research.md` §Client-side facts).
- Split before extending: `crates/net/src/wire.rs`, `crates/netcode/src/replication.rs`, `crates/level-compiler/src/parse.rs`, `crates/renderer/src/render/mesh_pass.rs`.

## Open questions
- The floor style and floor bus profile values — **delegated**: chosen against the legibility row and recorded in the plan of record.

## Wire format
**PRL `GhostLinks` (id 56, next free).** Mirrors Teleporters (id 55): little-endian, `u32` counts and indices, `f32` scalars, convex leaves and length-prefixed strings encoded as id 55 encodes them. Optional: present when the map has at least one `ghost_link`; absent means no links. Presentation only: the level content digest does not hash it. Link `_tags` ride the id-29 companion record, as teleporter tags do.
- Header: `u16` internal version (1), `u32` link count, capped at 65,535 with `0xFFFF` reserved as the no-link index.
- Per link, in map entity order:
  - source entity
  - `u8` enabled_on_spawn
  - `u8` starting member categories: bit 0 players, bit 1 entities, bit 2 projectiles, bit 3 movers; other bits 0
  - style name, length-prefixed; empty means the floor style
  - convex leaves
  - landmark origin and yaw, then target origin and yaw
  - `u32` ghost-tag count, then each tag length-prefixed, in authored order, deduplicated. Count 0 means no movers.
- Decode rejects:
  - another version, or a link count of 0 or past the cap
  - an enabled_on_spawn other than 0 or 1
  - member-category bits past bit 3
  - a non-finite pose
  - an empty or duplicated ghost tag
  - a payload of the wrong length

**Snapshot payloads (`WIRE_VERSION` 32 → 33, `SNAPSHOT_VERSION` 21 → 22).** Per link: map-order index (`u16`), source entity, armed, and the `u8` member categories in the PRL bit order. Then a `u8` field mask (bit 0 tint, bit 1 opacity, bit 2 saturation; other bits 0), and for each set field its ramp: from-value, to-value, host start time and duration ms. Tint values are three `f32`. Level-scoped: a `u8` field mask (bit 0 volume, bit 1 highPassHz, bit 2 lowPassHz, bit 3 drive) with a ramp per set field. Bound on the client by map-order index and source entity, like `WireTeleporterState`. The level-scoped record's identity scheme is the executor's. Style values and limiter state never ride the wire.

## Boundary inventory
| Name | Rust | Wire / PRL | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Ghost link entity | new component kind | PRL id 56 record; link-state payload | `getMapEntities("ghostLink")` | `getMapEntities("ghostLink")` | `ghost_link`, with `_tags` |
| Landmark / target | — | poses | — | — | `landmark`, `target` |
| Spawn armed | — | `enabled_on_spawn` byte | — | — | `enabled_on_spawn` (0/1, default 1) |
| Ghosted tags | — | per-link tag list | — | — | `ghost_tags` (whitespace-delimited, like `_tags`; default empty) |
| Style reference | resolved style | style name string | — | — | `ghost_style` (default empty: floor style) |
| Member categories | link field | `u8` mask in PRL and link-state payload | `setMembers({ players, entities, projectiles, movers })` | `l:setMembers({...})` | `ghost_members` (whitespace-delimited from `players entities projectiles movers`; default all four) |
| Descriptor opt-in | descriptor attribute | — | `defineEntity({ canonicalName, ghost, components })`; default `false` for every descriptor | same | — |
| Projectile opt-in | weapon projectile field | — | `components.weapon.projectile.ghost` (default `false`) | same | — |
| Style declaration | style table | — | `ghostStyles: [defineGhostStyle({ name, tint: [r, g, b], opacity, saturation, audio, impactAudio })]` | same | — |
| Look change | host-only command | per-field look ramps in link-state payload | `setColorFilter({ tint, opacity, saturation, periodMs })` | `l:setColorFilter({...})` | — |
| Arm / disarm | host-only commands | armed in link-state payload | `arm()`, `disarm()` | `l:arm()`, `l:disarm()` | — |
| Ghost transparency | render-profile mode | — | `render.ghosts: "blend" \| "dither"` (default `"blend"`) | same | — |
| Ghost bus profile | resolved profile | — | `audio.ghost: { volume, highPassHz, lowPassHz, drive }` | same | — |
| Ghost bus change | host-only reaction body | per-field level audio payload | `setGhostAudio({ volume, highPassHz, lowPassHz, drive, periodMs })` from `postretro/audio` | `require("postretro/audio").setGhostAudio({...})` | — |
