# ghost-entities

Brief · resumable · reads: `context/lib/movement.md` §6 (Teleport), `context/lib/networking.md` §Current contract, §What gates, and what replicates instead, §Version gates, `context/lib/rendering_pipeline.md` §4, §7.4, `context/lib/audio.md` §1, §4, §5, `context/lib/scripting.md` §1, §12 · `context/lib/build_pipeline.md` §PRL section IDs · read at d6f3471

## Problem
The game owner wants a new capability for The Dreadgate Trials, the first game on the engine: co-op players in the same room at different moments in time. Each player sees the partner in the other time as a ghost. A barrier one player cannot pass is watched opening from the other time when the partner hits a button there. In battles, each player sees only their own monsters plus the partner's ghost and aim, and a script swaps them so each faces what they could not see. The swap already exists: room copies joined by a seamless teleporter pair (`docs/teleporters.md` "A closet loop and a time shift"). The missing piece is perceiving the other copy. When done, a mapper places a `ghost_link` volume over one room copy, naming a landmark in it, a target in the other copy, the tags it ghosts and a ghost style. A mod declares its ghost styles in its manifest: look, which extras ghost, and whether sound carries. While the link is armed, members inside it draw as translucent ghosts at the mapped pose in the other copy. When the style carries sound, their sounds also play there through a muffled ghost bus. Scripts arm links and change look and ghost audio during play, with ramps.

## Decisions
- **Room copies, not phase tags.** Timelines are separate physical copies, so collision, hits, splash, AI perception and triggers stay isolated with no filter (none exists today). Copies may differ in geometry and baked light. Ghosts are presentation only: they never collide, block, take hits or touch triggers. Rival rejected: tagging entities in one shared space would need a filter in every interaction path and would force both timelines to share geometry and lighting.
- **No network work for the ghosts themselves.** Every client already receives and interpolates every networked entity wherever its pawn is (`host_replicate` has no relevance filter). Ghosts derive locally from that state on every peer, the listen host included. Only link state and ghost audio state cross the wire (below).
- **Three layers, one question each** (`scripting.md` §1, "Engine owns the floor; scripts own the taste"):
  - The mod manifest declares how ghosts present, mod-wide: named ghost styles and the ghost bus profile.
  - Map data says where and what: the volume, the two points, the ghosted tags, the spawn state and which style.
  - Level scripts change things during play: arming, look and ghost audio.
  - No layer carries another's answer. The link holds no raw look values or presentation flags, and the manifest names no map entity.
- **`ghost_link` is a brush entity that reuses `teleport_destination` points.** `landmark` names a point inside the source copy, `target` a point in the other copy. They must exist and differ, under the same rules and error wording as a seamless `trigger_teleport`. The link is independent of teleporters: ghosts show whether or not any teleporter is armed. A two-way link is two volumes, like a two-way teleporter pair.
- **One mapping function, in physics beside the teleport resolve.** The landmark → target rigid transform is extracted from the seamless branch of the teleport resolve into one shared pure function. Teleports and ghosts both call it, so a ghost and a seamless arrival from the same pose always agree. This follows the one-chokepoint rule for pawn relocation (`movement.md` §6). The app crate already depends on physics.
- **Each link names what it ghosts.** Player pawns, with their third-person weapon, are members of every link. A link's `ghost_tags` lists the `_tags` it also ghosts, and it applies to movers and map-placed descriptor entities. Membership is per link: one barrier can ghost through one link and not another. Tags are map data every peer loads, so each peer resolves the same members at level install. A listed tag that matches no entity warns once at install. A projectile, and an impact sound, belong to a link exactly when their owner does. An impact's owner is the pawn its shot id names, and an impact without a shot id belongs to no link. Monsters ghost only when tagged. The viewing player's own pawn is never ghosted for that viewer.
- **Ghost styles are named manifest declarations, following fluids.** `ModManifest.ghostStyles` lists `defineGhostStyle` entries: `name`, `tint`, `opacity`, `desaturate`, `projectiles`, `audio`, `impactAudio`. `projectiles` ghosts members' projectiles. `audio` carries members' anchored sounds. `impactAudio` carries members' impact sounds when `audio` is also set. A link's `ghost_style` names a style, and each peer resolves it locally, by name, at level install and on each committed staged reload, as fluid volumes resolve. Validation also follows fluids: a malformed entry or a later duplicate name drops with a warning naming `ghostStyles[i].<field>`, and the first declaration of a name stands. A link naming no style, or a name no style declares, uses the engine floor style; an unknown name warns once at install. Styles stay out of the mod compatibility digest and off the wire. Peers whose declarations differ see different ghosts, the visibility difference co-op already accepts for fluid look (`networking.md` §What gates, and what replicates instead).
- **The engine floor style is legible and neutral, not a game's taste.** It is fully desaturated, neutrally tinted, translucent but readable against both copies' lighting and under the high-contrast theme, and it ghosts no projectiles and carries no sound. The floor ghost bus profile is band-limited and quieter than SFX, with no distortion. A game's look, Dreadgate's "punchy but muffled" included, is a style and a bus profile in that game's manifest.
- **An armed link is a dynamic coupling in the cell-visibility relation.** It makes the cells under its source volume perceivable from the cells under its target, stored as if open and masked at runtime by armed state. This is the dynamic-portal shape the relation's generalizability contract reserves for runtime couplings (`context/research/cell-visibility-substrate.md`). Future relevance, cue-recipient and audible-set consumers then inherit it through the relation's one query, and none of them knows about links. Nothing is built now: no consumer queries the relation at runtime yet. At promotion, the coupling is recorded in `build_pipeline.md` beside id 46, so that ghosts and ghost audio do not vanish silently when relevance lands.
- **Membership is evaluated per frame from presented state.** An entity ghosts through a link when the link is armed and the entity's presented position lies inside the volume: the body point for pawns, the origin for other entities, current bounds center for movers. The ghost draws at the mapped presented pose, so aim pitch, upper-body yaw, pose modifiers and teleport cuts carry over unchanged. A teleport swap cuts the ghost on the same frame with no interpolated smear. An entity inside two armed links ghosts through each.
- **Ghost draw.** Unlit: the base color is desaturated by `desaturate`, multiplied by `tint`, and blended at `opacity`. Ghosts are depth-tested without depth write, cast no shadows, and are culled at the mapped position against the viewer's visible cells, independent of the real position's visibility. This needs translucent variants of the skinned-mesh and kinematic-brush passes and a second draw transform per mover. A projectile ghost gets the same treatment on its billboard or model body. Placement: renderer owns the pipelines; game logic decides membership and hands the renderer extra instances with look values.
- **Look resolves per link, per field:** floor style, then the link's named style, then `setLook`. An omitted `setLook` field keeps its current value. Presentation flags come from the style alone and do not change at runtime.
- **Ghost audio.** One Ghost bus under SFX carries volume, high-pass, low-pass and distortion drive. Ghost voices also pass through SFX, so SFX volume and world muffle still apply. When a link's style sets `audio`, a sound anchored at a member inside the volume plays a second voice at the mapped fire-time point on the Ghost bus. This covers fire, dry fire, reload, overheat and a tagged mover's sounds. An impact sound joins under the membership rule only when the style also sets `impactAudio`, and contacts outside the volume drop. The real voice is untouched.
- **Ghost voices have their own cap of 16.** The echo then never crowds out the sounds a player reacts to. The figure is half the SFX cap and fits the audio manager's existing capacity without raising it.
- **The ghost bus profile resolves:** floor profile, then the manifest `audio.ghost` block, then `setGhostAudio`. A malformed `audio.ghost` field applies the whole floor profile with a warning naming the field, matching `audio.attenuation`. Bus parameters are mod-wide, not per link or per style.
- **Link state and ghost audio replicate as always-on host state, the teleporter precedent.**
  - `arm`, `disarm`, `setLook` and `setGhostAudio` apply on the host only, at its next tick, in call order, and do nothing on a connected client.
  - Each link registers at level install and replicates armed state plus its current look ramp; one level-scoped record replicates the ghost audio ramp.
  - A ramp carries from-value, to-value, host start time and duration. The client rebases the start with the host-time rule bound presentation state already uses (`networking.md` §Current contract). A new set mid-ramp starts from the value presented at that moment.
  - Until a link's first record lands, a joiner presents its locally resolved style. Once `setLook` has run, the host's values replace the style's. Demotion and level unload drop received state.
  - This diverges from the bound-presentation lane, which replicates only after a host-only write and lets every-machine origins present locally. Link state needs one authority and a late-joiner baseline, and a client-local early apply would double-apply against the replicated record.
  - Bumps `WIRE_VERSION` 32 → 33 and `SNAPSHOT_VERSION` 21 → 22.
- **Off the level content digest.** Links and their keys are presentation, not a prediction input (`networking.md` §Level identity and level content digest). Adding a link leaves the digest and `FINGERPRINT_EPOCH` unchanged.
- **First consumer ships with it.** A dev map with two room copies, a two-way link pair and a two-way seamless pair holds both set-pieces: the barrier puzzle and the battle swap. The barrier is ghosted by tag, and the links name a sound-carrying style. The dev mod's manifest declares that style and a ghost bus profile. The map has a TypeScript script and a Luau twin, following the `teleporters.map` / `teleporters-luau.map` precedent. An author section joins `docs/teleporters.md` or a new `docs/ghost-links.md` (executor's call).
- **Non-goals.**
  - Phase or timeline tags and interaction filtering of any kind.
  - Ghost impact sparks: they would reveal targets the player must not see.
  - Hitscan tracers and muzzle flashes: no peer sees them today, even unghosted. They are separate weapon-VFX work.
  - Remote footsteps: no machine plays another player's movement sounds today (roadmap Epic 12 step 4).
  - Suppressing the real voice: the author doc tells mappers to place copies farther apart than the attenuation `maxDistance`.
  - Per-link or per-style filter chains.
  - Switching a link's style at runtime: `setLook` covers look changes during play. A runtime style switch can be added on demand.
  - Ghosting runtime-spawned entities and world geometry.
  - Lit ghosts.

### Scripting surface
```typescript
// start-script.ts — mod-wide presentation
import { defineGhostStyle, defineMod } from "postretro";

export default defineMod({
  name: "Dreadgate", id: "dreadgate", version: "0.1.0",
  // Every field but name optional; omitted fields take the floor style's value.
  ghostStyles: [
    defineGhostStyle({ name: "echo", tint: { x: 0.6, y: 0.8, z: 1.0 }, opacity: 0.45, desaturate: 0.8,
      projectiles: false, audio: true, impactAudio: false }),
  ],
  // Every field optional; a malformed field applies the whole floor profile.
  audio: { ghost: { volume: 0.7, highPassHz: 250, lowPassHz: 3000, drive: 0.3 } },
});
```
```typescript
// Level script — changes during play
import { defineReaction, getMapEntities, setGhostAudio } from "postretro";

export function setupLevel() {
  const echoes = getMapEntities("ghostLink", { tag: "vault_echo" }); // map order
  const reveal = defineReaction("echo.reveal", {
    sequence: [
      ...echoes.flatMap((l) => l.arm()), // also: l.disarm()
      // Every field optional; an omitted field keeps its current value. periodMs defaults to 0.
      ...echoes.flatMap((l) => l.setLook({ tint: { x: 1.0, y: 0.4, z: 0.3 }, opacity: 0.6, periodMs: 1500 })),
      setGhostAudio({ volume: 0.9, lowPassHz: 5000, periodMs: 1500 }),
    ],
  });
  return {
    reactions: [reveal],
    triggerEvents: getMapEntities("trigger", { tag: "echo_plate" }).map((t) => t.on("enter", [reveal])),
  };
}
```
- `defineGhostStyle` is a pure identity helper, like `defineFluid`.
- `setLook`, `arm` and `disarm` return `SequenceStep[]`; `setGhostAudio` returns one step.
- Each member also carries `id`, `position` and `tags`.
- Ranges: `tint` components, `opacity`, `desaturate`, `volume` and `drive` lie in 0..=1. Filter cutoffs lie in 20..=20000 Hz, with `highPassHz` below `lowPassHz`. `volume` maps on the existing perceptual taper.
- An out-of-range or non-finite script argument warns and drops that step. A `setGhostAudio` whose result leaves `highPassHz` at or above `lowPassHz` also warns and drops.
- The Luau mirror ships too: `defineGhostStyle({...})`, `l:arm()`, `l:setLook({...})`.

## Acceptance
### Automated
**Compile and load**
- [ ] A map with a `ghost_link` round-trips its records, ghost tags and style name included, through the PRL section.
- [ ] A missing or unknown `landmark` or `target`, a `landmark` equal to `target`, a link with no brushes and a bad `enabled_on_spawn` each fail with the author-facing error.
- [ ] A `ghost_tags` entry matching no mover or placement warns once at level install, and the level loads.
- [ ] Decode rejects each case the Wire format names.
- [ ] Adding a link to a map leaves the level content digest unchanged, while adding a teleporter still changes it.
**Styles**
- [ ] A link naming a declared style presents that style's values and flags. A link naming no style presents the floor style with no warning. A link naming an undeclared style presents the floor style and warns once at install.
- [ ] A style with one malformed field drops whole with a warning naming `ghostStyles[i].<field>`, and links naming it get the floor style. A later duplicate name drops with a warning, and the first declaration stands. Both runtimes judge the same entries the same way.
- [ ] A committed staged reload that changes a style re-resolves armed links to the new values, and a link carrying `setLook` values keeps them.
- [ ] Two peers whose manifests declare one style name with different values each present their own, and the session admits both.
**Mapping and membership**
- [ ] For rotated and unrotated pairs, with vertical offset, a ghost pose equals the pose a seamless teleport through the same points produces.
- [ ] Membership edges:
  - presented point just inside vs just outside the volume
  - partner pawn ghosted while an untagged monster beside it is not
  - a mover and a placement carrying a listed tag ghosted
  - the same tagged barrier inside two links ghosts through the link that lists its tag and not the one that doesn't
  - a player's projectile ghosted under a style with `projectiles` true and not under one with false
  - under `projectiles` true, a tagged enemy's projectile ghosts and an untagged enemy's does not
  - viewer's own pawn absent from its own ghost set while the partner sees it
  - an entity inside two armed links yields two ghosts
- [ ] Disarmed link: no ghosts. Arm then disarm on consecutive ticks shows ghosts for exactly the armed span. Arm and disarm in one tick ends in the last call's state.
- [ ] A ghost draws when its mapped cell is visible and not when it isn't, both with the real cell visible and without.
- [ ] Two pawns swapped on one tick by arming both seamless teleporters: on the cut frame each ghost appears in the other copy, and no frame draws a ghost between the two poses.
**Look and audio values**
- [ ] `setLook` overrides the style field by field, and an omitted field keeps its current value.
- [ ] A malformed `audio.ghost` field applies the whole floor profile and warns naming the field. A `highPassHz` at or above `lowPassHz` counts as malformed.
- [ ] A `periodMs` ramp presents the midpoint value halfway through. A second set mid-ramp starts from the presented value. `periodMs` 0 lands in one frame.
- [ ] A fire sound anchored at a ghosted pawn admits a second voice at the mapped point on the Ghost bus under a style with `audio` true, and none under one with `audio` false.
- [ ] An impact sound is ghosted only under a style with `audio` and `impactAudio` both true, from a shot whose owner is a member. An impact without a shot id is never ghosted. Contacts outside the volume drop.
- [ ] The seventeenth live ghost voice is refused while a real SFX sound requested in the same frame still plays. Ghost voices release their slot on finish, stop and level unload.
**Replication**
- [ ] Host `arm`, `setLook` and `setGhostAudio` reach a connected client, and the client's ramp ends when the host's does.
- [ ] A late joiner gets current armed state and `setLook` values, not `enabled_on_spawn` and its local style.
- [ ] The same steps run on a connected client change nothing.
- [ ] Demotion and level unload drop received link and audio state.
- [ ] A handshake across the old and new wire versions is refused.
**Surface fixture**
- [ ] The dev mod's manifest and the dev map's TypeScript and Luau scripts, which contain the Scripting surface examples, load and run in a test with zero warnings. Their set-pieces drive the arm, look and audio changes the rows above observe.
- [ ] A headless frame capture of a ghost in front of opaque geometry shows tinted, blended pixels with the geometry behind still visible.
### Manual
- [ ] Two-peer loopback co-op on the dev map, run once as listen host and once as client, at the harness lag profile. Barrier puzzle: one player presses the button in copy B, sees the ghost barrier open, and the player in copy A passes. Battle swap: each player sees only their own monsters plus the partner's ghost, aim pitch and weapon. After the swap each faces the other set.
- [ ] The floor style reads as a ghost against both copies' lighting and under the high-contrast theme. Record the chosen floor values.
- [ ] The dev mod's sound-carrying style and bus profile make the partner's fire read punchy and muffled, and the scripted ramp is audible.

## Path
- The thin slice falsifies the riskiest seam. It covers a link section, a local membership pass, one floor-style translucent skinned-mesh ghost of the partner pawn, and a two-peer loopback swap. That proves the presented pose, the teleport cut and mapped culling compose before any surface fans out.
- Precedents to follow:
  - Teleporter compile, section and install: `TeleporterParser`, `TeleportersSection`, `decode_teleporters`, `spawn_loaded_teleporters`, `wrapTeleporterEntity`.
  - Brush volumes: `environment_volumes::resolve_leaf`.
  - Style declaration, validation and resolution by name: `defineFluid` and the fluid table's install and re-resolve on staged reload.
  - Teleporter replication: `WireTeleporterState`, `collect_payloads`, `crates/netcode/src/client/teleporters.rs`.
  - Host-only no-op: `group_commands_apply_here`.
  - Ramp rebasing: `apply_presentation_payload`, `presentation_host_time`.
  - Bus profile: `resolve_authored_attenuation`.
  - Global step: `setSentiment`.
- Rendering: ghost skinned instances ride the existing batch as extra `MeshInstanceInput`s, the way `attachments::emit_for_holder` adds them, and need a ghost `MeshPaletteCacheKey` variant. Mover upload keys by `mover_id`, and `active_draw_lookup` keeps one instance per id, so a mover ghost needs its own instance path. The instance structs carry no tint or alpha today.
- Audio:
  - Anchored voices are parented to SFX in `spatial.rs::start_voice`. `AudioCore::play` drops anchored non-SFX requests, and voice release assumes `BusId::Sfx`. All three need the bus a voice took.
  - Ghost requests derive at the drain, where `AnchorScene::sound_anchor` resolves emitters. A ghost voice uses a `Point` anchor, because an `Entity` anchor would follow the real pose.
  - An impact's `WeaponEmission.shot_id.pawn` is its owner pawn's stamped `NetworkId`.
  - The bus chain uses kira 0.12.5's built-in `FilterBuilder` (`FilterMode::HighPass`, `LowPass`), `DistortionBuilder` and `VolumeControlBuilder`, added with `TrackBuilder::add_effect` so their handles are kept (`with_effect` discards them). Distortion drive is in decibels, so the 0..=1 `drive` maps onto a dB range the executor picks.
- Rivals weighed:
  - Look values and flags as per-link map keys: every link would repeat a game's taste, and restyling a game would mean editing every map.
  - Membership set by script at arm time (`l.arm({ tags, ... })`): map-data membership lets every peer resolve the same members locally, which keeps membership off the wire. A script override can be added later.
  - A per-entity `ghost` KVP: it cannot vary by link, and it needs a mixed mover-id / placement-ordinal table, because movers carry no id-29 ordinal.
- Resolve `ghost_tags` against the same tag index `getMapEntities` queries, once per level install.
- Files past ~800 lines this extends are split first, in their own commits. Likely candidates: `crates/net/src/wire.rs`, `crates/netcode/src/replication.rs`, `crates/level-compiler/src/parse.rs`, `crates/renderer/src/render/mesh_pass.rs`.

## Open questions
- The floor style and floor bus profile values — **delegated**: chosen against the legibility row and recorded in the plan of record.

## Wire format
**PRL `GhostLinks` (id 56, next free).** Mirrors Teleporters (id 55): little-endian, `u32` counts and indices, `f32` scalars, convex leaves and length-prefixed strings encoded as id 55 encodes them. Optional: present when the map has at least one `ghost_link`; absent means no links. Presentation only: the level content digest does not hash it.
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

**Snapshot payloads (`WIRE_VERSION` 33, `SNAPSHOT_VERSION` 22).** Per link: map-order index (`u16`), source entity, armed, and the look ramp once `setLook` has run (from-look, to-look, host start time, duration ms), else none. Level-scoped: the ghost audio ramp once `setGhostAudio` has run, else none. Bound on the client by map-order index, like `WireTeleporterState`. The level-scoped record's identity scheme is the executor's (one registered level entity, or a dedicated binding). Style values never ride the wire.

## Boundary inventory
| Name | Rust | Wire / PRL | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Ghost link entity | new component kind | PRL id 56 record; link-state payload | `getMapEntities("ghostLink")` | `getMapEntities("ghostLink")` | `ghost_link` |
| Landmark / target | — | poses | — | — | `landmark`, `target` |
| Spawn armed | — | `enabled_on_spawn` byte | — | — | `enabled_on_spawn` (0/1, default 1) |
| Ghosted tags | — | per-link tag list | — | — | `ghost_tags` (whitespace-delimited, like `_tags`; default empty) |
| Style reference | resolved style | style name string | — | — | `ghost_style` (default empty: floor style) |
| Style declaration | style table | — | `ghostStyles: [defineGhostStyle({ name, tint, opacity, desaturate, projectiles, audio, impactAudio })]` | same | — |
| Look change | host-only command | look ramp in link-state payload | `setLook({ tint, opacity, desaturate, periodMs })` | `l:setLook({...})` | — |
| Arm / disarm | host-only commands | armed in link-state payload | `arm()`, `disarm()` | `l:arm()`, `l:disarm()` | — |
| Ghost bus profile | resolved profile | — | `audio.ghost: { volume, highPassHz, lowPassHz, drive }` | same | — |
| Ghost bus change | host-only command | level audio payload | `setGhostAudio({ volume, highPassHz, lowPassHz, drive, periodMs })` | `setGhostAudio({...})` | — |
