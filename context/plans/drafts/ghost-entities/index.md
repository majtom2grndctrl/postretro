# ghost-entities

Brief · resumable · reads: `context/lib/movement.md` §6 (Teleport), `context/lib/networking.md` §Current contract, §What gates, and what replicates instead, §Version gates, `context/lib/rendering_pipeline.md` §4, §7.4, §7.8 (Photosensitivity limiter, Fluid view tint), `context/plans/ready/E23--accessibility/index.md` (I2, I9, D4), `context/lib/audio.md` §1, §4, §5, `context/lib/scripting.md` §1, §12, `context/lib/build_pipeline.md` §PRL section IDs · read at d6f3471

## Problem
The game owner wants a new capability for The Dreadgate Trials, the first game on the engine: co-op players in the same room at different moments in time. Each player sees the partner in the other time as a ghost. A barrier one player cannot pass is watched opening from the other time when the partner hits a button there. In battles, each player sees only their own monsters plus the partner's ghost and aim, and a script swaps them so each faces what they could not see. The swap already exists: room copies joined by a seamless teleporter pair (`docs/teleporters.md` "A closet loop and a time shift"). The missing piece is perceiving the other copy. When done, a mapper places a `ghost_link` volume over one room copy, naming a landmark in it, a target in the other copy, the tagged movers it ghosts and a ghost style. A mod declares its ghost styles in its manifest: look, which extras ghost, and whether sound carries. While the link is armed, its members draw as translucent ghosts at the mapped pose in the other copy. When the style carries sound, their sounds also play there through a muffled ghost bus. Scripts arm links and change look and ghost audio during play, with ramps.

## Decisions
- **Room copies, not phase tags.** Timelines are separate physical copies, so collision, hits, splash, AI perception and triggers stay isolated with no filter (none exists). Copies may differ in geometry and baked light. Ghosts are presentation only: they never collide, block, take hits or touch triggers. Rival rejected: tags in one shared space need a filter in every interaction path, and both timelines would have to share geometry and lighting.
- **No network work for the ghosted entities.** Members are player pawns and movers. Every peer has both: pawns replicate to every client wherever its pawn is, with no relevance filter, and clients spawn movers, tags included, from the level file. Ghosts derive locally on every peer, the listen host included. Only link state and ghost audio state cross the wire.
- **Each layer answers one question** (`scripting.md` §1, "Engine owns the floor; scripts own the taste"). The mod manifest says how ghosts present: named styles and the ghost bus profile. Map data says where and what: volume, points, tags, spawn state and style name. Level scripts say what changes during play. The link holds no raw look values or presentation flags, and the manifest names no map entity.
- **`ghost_link` is a brush entity that reuses `teleport_destination` points.** `landmark` names a point inside the source copy, `target` a point in the other copy. They must exist and differ, under the seamless `trigger_teleport` rules and error wording. Links carry `_tags` for script queries. A link is independent of teleporters. A two-way link is two volumes.
- **One mapping function, in physics beside the teleport resolve.** The seamless landmark → target transform becomes one shared pure function that teleports and ghosts both call, so a ghost and a seamless arrival from the same pose always agree. This follows the one-chokepoint rule for pawn relocation (`movement.md` §6).
- **Membership: players and tagged movers.**
  - Player pawns, with their third-person weapon, are members of every link. A link's `ghost_tags` adds the movers carrying those `_tags`, and membership is per link.
  - A listed tag that matches no mover warns once at install.
  - When the link's style sets `projectiles`, a player's projectile is a member while its owner is one. An impact belongs to a link when its shot's owner does, resolved from the shot id normalized the way observer cues normalize it. An impact without a shot id belongs to no link.
  - The viewing player's own pawn, its projectiles and its sounds never ghost on that viewer's machine.
- **Ghost styles are named manifest declarations, following fluids.**
  - `ModManifest.ghostStyles` lists `defineGhostStyle` entries: `name`, `tint`, `opacity`, `desaturate`, `projectiles`, `audio`, `impactAudio`. `impactAudio` takes effect only with `audio`.
  - A link's `ghost_style` names a style. Each peer resolves it locally, by name, at level install and on each committed staged reload, as fluid volumes resolve.
  - Validation follows fluids: a malformed entry or a later duplicate name drops with a warning naming `ghostStyles[i].<field>`, and the first declaration stands.
  - A link naming no style uses the engine floor style. A name no style declares also uses it, warning once at install.
  - Styles stay out of the mod compatibility digest and off the wire. Peers whose declarations differ see different ghosts, the visibility difference co-op accepts for fluid look (`networking.md` §What gates, and what replicates instead).
- **The floor style is legible and neutral, not a game's taste.** It is fully desaturated, neutrally tinted and translucent, yet readable against both copies' lighting and under the high-contrast theme. It ghosts no projectiles and carries no sound. The floor ghost bus profile is band-limited, quieter than SFX and undistorted. A game's look, Dreadgate's "punchy but muffled" included, is a style and a bus profile in its own manifest.
- **Future relevance must treat an armed link as a one-way coupling.** Cells under the source volume are perceivable from cells under the target while the link is armed. Any relevance, cue-recipient or audible-set filter that lands later gets it through the cell-visibility relation, never by knowing about links (`context/research/cell-visibility-substrate.md`, generalizability contract). The shipped relation stores connected-component ids, and folding a link into a component would couple both ways and permanently. So the coupling waits for the relation's runtime-masked couplings, and nothing is built now. At promotion it is recorded in `build_pipeline.md` beside id 46 as an obligation on that extension.
- **Membership is evaluated per frame from presented state.**
  - An entity is a target member while the link is armed and its presented point lies inside the volume: the body point for pawns, the origin for projectiles, the current bounds center for movers.
  - The ghost draws at the mapped presented pose, so aim pitch, upper-body yaw, pose modifiers and teleport cuts carry over.
  - A ghost whose membership ends fades out at its last in-volume mapped pose. It never draws at the mapping of a pose outside the volume, so a teleport swap draws no ghost between copies.
  - An entity inside two armed links ghosts through each, one transform per link.
  - At level install, members of links armed on spawn present at full membership on the first frame.
- **Ghost draw.**
  - Unlit: the base color is desaturated by `desaturate`, multiplied by `tint`, and blended at the presented opacity.
  - Depth-tested without depth write; no shadows.
  - Culled at the mapped position against the viewer's visible cells, whatever the real position's visibility.
  - Translucent variants of the skinned-mesh and kinematic-brush passes, plus per-link mover transforms. A projectile ghost gets the same treatment on its billboard or model body.
  - The renderer owns the pipelines. Game logic decides membership and hands the renderer extra instances with look values.
- **Ghosts pass the photosensitivity limiter.** E23 I2 counts a primitive that changes presented luminance without a clamp as a violation, and `setLook` and arm/disarm are such primitives.
  - A ghost's presented opacity is its link's opacity times its membership. Membership ramps linearly at the limiter's rate cap on arm, disarm, entry and exit, reversing from the presented value. As with fluid tint entry, ordinary reveals are never limited and a strobe is.
  - Each ghost is one limiter channel: presented opacity is its strength and its link's tint is its redness, under the fluid view tint's rules and player-only toggle (`rendering_pipeline.md` §7.8). It assumes worst-case coverage (E23 D4).
  - A ghost's limiter state starts from opacity 0 and keys by generation-checked entity and link. It applies on the presenting machine to the presented value, and nothing it holds reaches a script or the wire (I9).
- **Look resolves per link, per field:** floor style, then the named style, then `setLook`. `setLook` ramps each field it names independently. An omitted field keeps its value and any ramp in flight. Presentation flags come from the style alone and never change at runtime.
- **Ghost audio mirrors voices this machine plays.**
  - When a link's style sets `audio`, a sound this machine plays anchored at a member inside the volume also plays a second voice. The voice sits at the mapped fire-time point on the Ghost bus. Admission reads the fire-time point and armed state, not the presented ramp.
  - Impact sounds join under the membership rule only when the style also sets `impactAudio`, and contacts outside the volume drop.
  - A ghost voice plays out when its member leaves or despawns or its link disarms. The real voice is untouched.
  - The Ghost bus sits under SFX, so SFX volume and world muffle apply. It carries volume, high-pass, low-pass and distortion drive, with mod-wide parameters, because the bus is one shared mix.
  - Coverage follows what each peer already plays. Movers and a pawn's own reload, dry fire and overheat sound only on the host, so a connected client ghosts only remote fire and impacts. A client's barrier opens silently, real and ghost alike.
- **Ghost voices have their own cap of 16**, so the echo never crowds out sounds a player reacts to. The figure is half the SFX cap and fits the audio manager's capacity without raising it.
- **The ghost bus profile resolves:** floor profile, then manifest `audio.ghost`, then `setGhostAudio`. A malformed `audio.ghost` field applies the whole floor profile with a warning naming the field, matching `audio.attenuation`. Level unload and demotion return the bus to the resolved manifest profile on every peer.
- **Link state and ghost audio replicate as always-on host state, the teleporter precedent.**
  - `arm`, `disarm`, `setLook` and `setGhostAudio` apply on the host only, at its next tick, in call order, and do nothing on a connected client.
  - Each link registers at level install and replicates armed state plus the fields `setLook` has set, each with its ramp. One level-scoped record replicates the fields `setGhostAudio` has set. Fields no script has set stay local.
  - A ramp's from-value is the unclamped value at the applying host tick. The client rebases the start with the host-time rule bound presentation state uses (`networking.md` §Current contract).
  - A client presents an armed edge at the remote-presentation time of the snapshot that carried it, so it lines up with the remote poses it gates.
  - Until a link's first record lands, a joiner presents its local spawn state and style. Demotion and level unload drop received state.
  - This diverges from the bound-presentation lane, which replicates only after a host-only write and lets every-machine origins present locally. Link state needs one authority and a late-joiner baseline, and a client-local early apply would double-apply against the replicated record.
- **Off the level content digest.** Links and their keys are presentation, not a prediction input (`networking.md` §Level identity and level content digest).
- **First consumer ships with it.** A dev map holds two room copies, a two-way link pair and a two-way seamless pair, with both set-pieces: the barrier puzzle, with the barrier a tagged mover, and the battle swap. The dev mod's manifest declares a sound-carrying style and a ghost bus profile. TypeScript and Luau scripts follow `teleporters.map` / `teleporters-luau.map`. An author section joins `docs/teleporters.md` or a new `docs/ghost-links.md` (executor's call).
- **Non-goals.**
  - Phase or timeline tags and interaction filtering.
  - Tagged descriptor placements, monsters and runtime spawns as members: on a connected client they are snapshot display copies with no tags or map identity. Replicating that identity is its own wire change, and the Problem's barrier is a mover.
  - Ghost impact sparks: they would mark a hidden target's position on screen. A game that wants impact hints opts into impact audio.
  - Hitscan tracers and muzzle flashes: no peer sees them today, even unghosted.
  - Remote movement sounds, and host-only mover and reload sounds on clients: networked peer audio is roadmap Epic 12 step 4.
  - Suppressing the real voice: the author doc tells mappers to place copies farther apart than the attenuation `maxDistance`.
  - Per-link or per-style filter chains: one shared bus keeps the mix and the voice budget simple.
  - Links that exclude players: an additive key can opt players out later without breaking maps.
  - Switching a link's style at runtime: `setLook` covers look changes during play.
  - World geometry ghosts: the other copy's architecture is the copy itself.
  - Lit ghosts: unlit keeps the ghost legible and avoids sampling light at the mapped position.

### Scripting surface
```typescript
// start-script.ts: mod-wide presentation
import { defineGhostStyle, defineMod } from "postretro";

export default defineMod({
  name: "Dreadgate", id: "dreadgate", version: "0.1.0",
  // Every field but name optional; omitted fields take the floor style's value. Colors follow fluids: [r, g, b].
  ghostStyles: [
    defineGhostStyle({ name: "echo", tint: [0.6, 0.8, 1.0], opacity: 0.45, desaturate: 0.8,
      projectiles: false, audio: true, impactAudio: false }),
  ],
  // Every field optional; a malformed field applies the whole floor profile.
  audio: { ghost: { volume: 0.7, highPassHz: 250, lowPassHz: 3000, drive: 0.3 } },
});
```
```typescript
// Level script: changes during play
import { defineReaction, getMapEntities, setGhostAudio } from "postretro";

export function setupLevel() {
  const echoes = getMapEntities("ghostLink", { tag: "vault_echo" }); // map order
  const reveal = defineReaction("echo.reveal", {
    sequence: [
      ...echoes.flatMap((l) => l.arm()), // also: l.disarm()
      // Each named field ramps on its own; omitted fields keep their value and ramp. periodMs defaults to 0.
      ...echoes.flatMap((l) => l.setLook({ tint: [1.0, 0.4, 0.3], opacity: 0.6, periodMs: 1500 })),
    ],
  });
  // A mod-wide primitive with no target is a whole reaction body, like setSentiment.
  const swell = defineReaction("echo.swell", setGhostAudio({ volume: 0.9, lowPassHz: 5000, periodMs: 1500 }));
  return {
    reactions: [reveal, swell],
    triggerEvents: getMapEntities("trigger", { tag: "echo_plate" }).map((t) => t.on("enter", [reveal, swell])),
  };
}
```
- `defineGhostStyle` is a pure identity helper, like `defineFluid`. `setLook`, `arm` and `disarm` return `SequenceStep[]`. `setGhostAudio` returns a primitive reaction body.
- Each member carries `id`, `position` and `tags`.
- Ranges: `tint` components, `opacity`, `desaturate`, `volume` and `drive` lie in 0..=1, and filter cutoffs in 20..=20000 Hz. `volume` maps on the existing perceptual taper.
- An out-of-range or non-finite script argument warns and drops that step or reaction. So does a `setGhostAudio` whose targets would leave `highPassHz` at or above `lowPassHz`.
- The Luau mirror ships: `defineGhostStyle({...})`, `l:arm()`, `l:setLook({...})`, `setGhostAudio({...})`.

## Acceptance
Pins are in `research.md`.
### Automated
**Compile and load**
- [ ] A map with a `ghost_link` round-trips its records, tags, ghost tags and style name through the PRL section.
- [ ] A missing or unknown `landmark` or `target`, a `landmark` equal to `target`, a link with no brushes and a bad `enabled_on_spawn` each fail with the author-facing error.
- [ ] A `ghost_tags` entry matching no mover warns once at level install on host and client alike, and the level loads.
- [ ] Decode rejects each case the Wire format names.
- [ ] Adding a link to a map leaves the level content digest unchanged, while adding a teleporter still changes it.
**Styles**
- [ ] A link naming a declared style presents that style's values and flags. A link naming no style presents the floor style with no warning. A link naming an undeclared style presents the floor style and warns once at install.
- [ ] A style with one malformed field drops whole with a warning naming `ghostStyles[i].<field>`, and links naming it get the floor style. A later duplicate name drops with a warning, and the first declaration stands. Both runtimes judge the same entries the same way.
- [ ] A committed staged reload that changes a style re-resolves every link naming it, armed or not, mid-ramp or not. Fields set by `setLook` keep their values and ramps (P16).
- [ ] Two peers whose manifests declare one style name with different values each present their own, and the session admits both. After an opacity-only `setLook`, both present the host's opacity and still their own tint and desaturation (P15).
**Mapping and membership**
- [ ] For rotated and unrotated pairs, with vertical offset, a ghost pose equals the pose a seamless teleport through the same points produces.
- [ ] Membership edges, each on host and on a connected client:
  - presented point just inside vs just outside the volume
  - partner pawn ghosted while an untagged mover and a monster beside it are not
  - a mover carrying a listed tag ghosted
  - the same tagged barrier inside two links ghosts through the link that lists its tag and not the other
  - a player's projectile ghosted under a style with `projectiles` true and not under one with false; a monster's projectile never
  - the viewer's own pawn, projectiles and sounds absent from its own ghost set and ghost voices, while the partner sees and hears them
  - a mover inside two armed links that both list its tag draws two ghosts at two transforms (P27)
- [ ] `arm` on tick N and `disarm` on tick N+1 ramp presented membership up and back down at the rate cap. `arm` and `disarm` in one tick start no ramp and present no ghost on any peer (P2, P3). An arm during a ramp-out, or a re-entry mid-fade, ramps from the presented value (P4).
- [ ] A ghost draws when its mapped cell is visible and not when it isn't, both with the real cell visible and without.
- [ ] Two pawns swapped on one tick by arming both seamless teleporters: each old ghost fades at its last in-volume mapped pose, each new ghost starts on the cut frame and fades in, and no frame draws a ghost between the copies (P1).
- [ ] On a connected client, a link arm and a remote pawn's seamless swap on one host tick present on one client frame (P18).
- [ ] An entity slot freed and re-tenanted on one frame starts the new tenant's ghost at membership 0 (P19). A member's projectile despawns with its ghost on the same frame. An owner leaving the volume mid-flight ramps out its projectile's ghost, and that projectile's impact is not ghosted (P20, P22).
- [ ] A ghost is never a hit, overlap, trigger or AI target: a hitscan through it, a splash on it and a trigger it overlaps record nothing.
**Photosensitivity**
- [ ] With the limiter on and a style at opacity 1, each of these presents at most 3 flashes in any 1 s window and rests at its pre-flash level over budget:
  - a script toggling `arm`/`disarm` at 10 Hz
  - `setLook` opacity alternating 0 and 1 at `periodMs` 0
  - a member oscillating across the volume boundary at 10 Hz
- [ ] With the limiter on, a red `setLook` tint strobe desaturates over budget, keeping luminance, while a single red tint change keeps its hue.
- [ ] With the limiter on, a single `arm` reaches full style opacity within 250 ms, and the example's 1500 ms `setLook` fade presents exactly as with the limiter off. A ghost's first presented frame starts from opacity 0 (P21).
- [ ] With the limiter turned off by the player, the 10 Hz `arm`/`disarm` strobe presents unclamped, and the membership ramp still applies (P30).
**Look and audio values**
- [ ] `setLook` overrides the style field by field. An opacity-only `setLook` mid tint ramp leaves the tint ramp running to its target (P5). Two `setLook` calls in one tick apply in order, the later winning per field (P7).
- [ ] A malformed `audio.ghost` field applies the whole floor profile and warns naming the field. A `highPassHz` at or above `lowPassHz` counts as malformed.
- [ ] A `periodMs` ramp presents the midpoint value halfway through, and a second set mid-ramp starts from the presented value. `periodMs` 0 lands in one frame. A `setGhostAudio` whose targets cross the cutoffs warns and drops (P29). A non-finite or out-of-range `setLook` or `setGhostAudio` argument warns and drops.
- [ ] A fire sound anchored at a ghosted pawn admits a second voice at the mapped fire-time point under a style with `audio` true, and none under one with `audio` false. A sound fired just inside the volume during a ramp-out still admits; one fired just outside does not (P24).
- [ ] An impact sound is ghosted only under a style with `audio` and `impactAudio` both true, from a shot whose owner is a member. The host's own impact is never attributed to another entity. An impact without a shot id is never ghosted. Contacts outside the volume drop.
- [ ] A ghost voice plays out at its mapped point after its member despawns or its link disarms (P23). Ghost voices follow SFX volume and world muffle.
- [ ] The seventeenth live ghost voice is refused while a real SFX sound requested in the same frame still plays. Ghost voices release their slot on finish, stop and level unload.
- [ ] After `setGhostAudio`, a level unload or demotion returns the Ghost bus to the manifest profile on host and client (P13). A staged reload changing `audio.ghost` re-resolves the bus, and `setGhostAudio` values stand (P17).
**Replication**
- [ ] Host `arm`, `setLook` and `setGhostAudio` reach a connected client, and the client's ramp ends within one snapshot interval of the host's. Repeated identical records do not move the presented value as RTT changes (P8).
- [ ] A late joiner, including one joining mid-ramp, gets current armed state, `setLook` fields and `setGhostAudio` fields, not its local spawn state (P9, P12). A record landing before the presentation clock applies armed state and defers the ramp (P10). A record naming an uninstalled link binds nothing, and a stale old-level record never binds a new link (P11, P14).
- [ ] A mid-ramp `setLook` carries the unclamped host value: a host with the limiter on and a client with it off present the same ramp, each through its own limiter (P6).
- [ ] The same steps run on a connected client change nothing.
- [ ] Demotion and level unload drop received link and audio state.
- [ ] A handshake across the old and new wire versions is refused.
**Surface fixture**
- [ ] The dev mod's manifest and the dev map's TypeScript and Luau scripts, which contain the Scripting surface examples, load and run with zero warnings. The `ghostLink` query returns both dev-map links, and their set-pieces drive the arm, look and audio changes the rows above observe (P28).
- [ ] A headless frame capture of the dev map's tagged barrier on a link armed on spawn, at rest, shows tinted, blended, unlit pixels with the geometry behind still visible, and no ghost shadow (P26).
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
- Rendering: a mover ghost needs its own instance path, because upload keys by `mover_id` and keeps one instance per id. Skinned ghosts ride the batch as extra instances with a ghost palette cache key.
- Limiter: the per-channel tracked value in `flash_clamp.rs` is private and hard-wired to the three screen channels. Expose it, or build a per-value clamp on the same rules.
- Audio:
  - Voices need the bus they took, through `start_voice`, `AudioCore::play` and voice release.
  - The Ghost track takes one of SFX's child-track slots and needs its own pool.
  - A ghost voice uses a `Point` anchor.
  - The chain is kira 0.12.5's built-in filter, distortion and volume effects, added with `add_effect` so their handles are kept. Drive is in decibels, so the 0..=1 value maps to a dB range.
- Impact owner: the host's own shots carry a raw local pawn id until `publish_observer_weapon_cues` normalizes it, and `NetworkId(0)` is a real id. Normalize before reading membership.
- Rivals weighed:
  - Ghosts as registry display copies: they enter hit, trigger, overlap and AI paths, and would need the filters room copies avoid.
  - Per-link look keys and flags: every link would repeat a game's taste.
  - Script-set membership at arm time: map data keeps membership off the wire.
  - A per-entity `ghost` KVP: it cannot vary by link.
- Split before extending: `crates/net/src/wire.rs`, `crates/netcode/src/replication.rs`, `crates/level-compiler/src/parse.rs`, `crates/renderer/src/render/mesh_pass.rs`.

## Open questions
- The floor style and floor bus profile values — **delegated**: chosen against the legibility row and recorded in the plan of record.

## Wire format
**PRL `GhostLinks` (id 56, next free).** Mirrors Teleporters (id 55): little-endian, `u32` counts and indices, `f32` scalars, convex leaves and length-prefixed strings encoded as id 55 encodes them. Optional: present when the map has at least one `ghost_link`; absent means no links. Presentation only: the level content digest does not hash it. Link `_tags` ride the id-29 companion record, as teleporter tags do.
- Header: `u16` internal version (1), `u32` link count, capped at 65,535 with `0xFFFF` reserved as the no-link index.
- Per link, in map entity order:
  - source entity
  - `u8` enabled_on_spawn
  - style name, length-prefixed; empty means the floor style
  - convex leaves
  - landmark origin and yaw, then target origin and yaw
  - `u32` ghost-tag count, then each tag length-prefixed, in authored order, deduplicated. Count 0 means players only.
- Decode rejects:
  - another version, or a link count of 0 or past the cap
  - an enabled_on_spawn other than 0 or 1
  - a non-finite pose
  - an empty or duplicated ghost tag
  - a payload of the wrong length

**Snapshot payloads (`WIRE_VERSION` 32 → 33, `SNAPSHOT_VERSION` 21 → 22).** Per link: map-order index (`u16`), source entity and armed. Then a `u8` field mask (bit 0 tint, bit 1 opacity, bit 2 desaturate; other bits 0), and for each set field its ramp: from-value, to-value, host start time and duration ms. Tint values are three `f32`. Level-scoped: a `u8` field mask (bit 0 volume, bit 1 highPassHz, bit 2 lowPassHz, bit 3 drive) with a ramp per set field. Bound on the client by map-order index and source entity, like `WireTeleporterState`. The level-scoped record's identity scheme is the executor's. Style values and limiter state never ride the wire.

## Boundary inventory
| Name | Rust | Wire / PRL | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Ghost link entity | new component kind | PRL id 56 record; link-state payload | `getMapEntities("ghostLink")` | `getMapEntities("ghostLink")` | `ghost_link`, with `_tags` |
| Landmark / target | — | poses | — | — | `landmark`, `target` |
| Spawn armed | — | `enabled_on_spawn` byte | — | — | `enabled_on_spawn` (0/1, default 1) |
| Ghosted tags | — | per-link tag list | — | — | `ghost_tags` (whitespace-delimited, like `_tags`; default empty) |
| Style reference | resolved style | style name string | — | — | `ghost_style` (default empty: floor style) |
| Style declaration | style table | — | `ghostStyles: [defineGhostStyle({ name, tint: [r, g, b], opacity, desaturate, projectiles, audio, impactAudio })]` | same | — |
| Look change | host-only command | per-field look ramps in link-state payload | `setLook({ tint, opacity, desaturate, periodMs })` | `l:setLook({...})` | — |
| Arm / disarm | host-only commands | armed in link-state payload | `arm()`, `disarm()` | `l:arm()`, `l:disarm()` | — |
| Ghost bus profile | resolved profile | — | `audio.ghost: { volume, highPassHz, lowPassHz, drive }` | same | — |
| Ghost bus change | host-only reaction body | per-field level audio payload | `setGhostAudio({ volume, highPassHz, lowPassHz, drive, periodMs })` | `setGhostAudio({...})` | — |
