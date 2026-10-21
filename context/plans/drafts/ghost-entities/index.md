# ghost-entities

Brief · resumable · reads: `context/lib/movement.md` §6 (Teleport), `context/lib/networking.md` §Current contract, §Version gates, `context/lib/rendering_pipeline.md` §4, §7.4, `context/lib/audio.md` §1, §4, §5, `context/lib/scripting.md` §12, `context/lib/build_pipeline.md` §PRL section IDs · read at d6f3471

## Problem
The game owner wants a new capability for The Dreadgate Trials, the first game on the engine: co-op players in the same room at different moments in time. Each player sees the partner in the other time as a ghost. A barrier one player cannot pass is watched opening from the other time when the partner hits a button there. In battles, each player sees only their own monsters plus the partner's ghost and aim, and a script swaps them so each faces what they could not see. The swap already exists: room copies joined by a seamless teleporter pair (`docs/teleporters.md` "A closet loop and a time shift"). The missing piece is perceiving the other copy. When done, a mapper places a `ghost_link` volume over one room copy, naming a landmark in it and a target in the other copy. While the link is armed, eligible entities inside it are drawn as tinted, translucent ghosts at the mapped pose in the other copy, and their sounds can play there through a muffled ghost bus. Scripts arm links and control the ghost look and ghost audio, with ramps.

## Decisions
- **Room copies, not phase tags.** Timelines are separate physical copies, so collision, hits, splash, AI perception and triggers stay isolated with no filter (none exists today). Copies may differ in geometry and baked light. Ghosts are presentation only: they never collide, block, take hits or touch triggers. Rival rejected: tagging entities in one shared space would need a filter in every interaction path and would force both timelines to share geometry and lighting.
- **No network work for the ghosts themselves.** Every client already receives and interpolates every networked entity wherever its pawn is (`host_replicate` has no relevance filter). Ghosts derive locally from that state on every peer, the listen host included. Only link state and ghost audio state cross the wire (below).
- **`ghost_link` is a brush entity that reuses `teleport_destination` points.** `landmark` names a point inside the source copy, `target` a point in the other copy. They must exist and differ, under the same rules and error wording as a seamless `trigger_teleport`. The link is independent of teleporters: ghosts show whether or not any teleporter is armed. A two-way link is two volumes, like a two-way teleporter pair.
- **One mapping function.** The landmark → target rigid transform is extracted from the seamless branch of the teleport resolve into one shared pure function used by both teleports and ghosts. A ghost and a seamless arrival from the same pose then always agree.
- **Each link names what it ghosts.** Player pawns, with their third-person weapon, are always eligible through every link. A link's `ghost_tags` lists the `_tags` it also ghosts, and it applies to movers and map-placed descriptor entities. Membership is per link: one barrier can ghost through one link and not another. Tags are map data every peer loads, so each peer resolves the same members at level install. A listed tag that matches no entity warns once at install. A projectile, and an impact sound, belong to a link exactly when their owner does. Projectiles also need the link's `ghost_projectiles`, and impacts need its `ghost_impact_audio`. Monsters ghost only when tagged. The viewing player's own pawn is never ghosted for that viewer.
- **An armed link is a dynamic coupling in the cell-visibility relation.** It makes the cells under its source volume perceivable from the cells under its target, stored as if open and masked at runtime by armed state. This is the dynamic-portal shape the relation's generalizability contract reserves for runtime couplings (`context/research/cell-visibility-substrate.md`). Future relevance, cue-recipient and audible-set consumers then inherit it through the relation's one query, and none of them knows about links. Nothing is built now: no consumer queries the relation at runtime yet. At promotion, the coupling is recorded in `build_pipeline.md` beside id 46, so that ghosts and ghost audio do not vanish silently when relevance lands.
- **Membership is evaluated per frame from presented state.** An entity ghosts through a link when the link is armed and the entity's presented position lies inside the volume: the body point for pawns, the origin for other entities, current bounds center for movers. The ghost draws at the mapped presented pose, so aim pitch, upper-body yaw, pose modifiers and teleport cuts carry over unchanged. A teleport swap cuts the ghost on the same frame with no interpolated smear. An entity inside two armed links ghosts through each.
- **Ghost look.** Unlit: the base color is desaturated by `desaturate`, multiplied by `tint`, and blended at `opacity`. Ghosts are depth-tested without depth write, cast no shadows, and are culled at the mapped position against the viewer's visible cells, independent of the real position's visibility. This needs translucent variants of the skinned-mesh and kinematic-brush passes and a second draw transform per mover. A projectile ghost gets the same treatment on its billboard or model body. Placement: renderer owns the pipelines; game logic decides membership and hands the renderer extra instances with look parameters.
- **Look values resolve in layers.** The engine default is overridden by the manifest `ghostLook` block, then by the link's own keys, then by script `setLook`. A malformed manifest block warns, naming the field, and the whole engine default applies, matching the `audio.attenuation` rule. Look is per link.
- **Ghost audio.** One Ghost bus under SFX carries volume, high-pass, low-pass and distortion drive. Ghost voices also pass through SFX, so SFX volume and world muffle still apply. When a link sets `ghost_audio`, a sound anchored at an eligible entity inside it plays a second voice at the mapped fire-time point on the Ghost bus. This covers fire, dry fire, reload, overheat and an opted mover's sounds. A contact-set impact sound joins under the membership rule above. Contacts outside the volume drop. The real voice is untouched. Ghost voices count against a Ghost bus voice cap.
- **Ghost audio values resolve in layers.** The engine default is overridden by the manifest `audio.ghost` block (same whole-default-on-malformed rule), then by `setGhostAudio`. Bus parameters are level-wide, not per link.
- **Link state and ghost audio replicate as always-on host state, the teleporter precedent.** `arm`, `disarm`, `setLook` and `setGhostAudio` apply on the host only, at its next tick, in call order, and do nothing on a connected client. Each link registers at level install and replicates armed state plus its current look ramp. One level-scoped record replicates the ghost audio ramp. A ramp carries from-value, to-value, host start time and duration, and the client rebases the start with the host-time rule bound presentation state already uses (`networking.md` §Current contract). A new set mid-ramp starts from the value presented at that moment. Before a joiner's first record lands, it presents the installed defaults. Demotion and level unload drop received state. This diverges from the bound-presentation lane, which replicates only after a host-only write and lets every-machine origins present locally. Link state needs one authority and a late-joiner baseline either way, and a client-local early apply would double-apply against the replicated record. Bumps `WIRE_VERSION` 32 → 33 and `SNAPSHOT_VERSION` 21 → 22.
- **Off the level content digest.** Links and their keys are presentation, not a prediction input (`networking.md` §Level identity and level content digest). Adding a link leaves the digest and `FINGERPRINT_EPOCH` unchanged.
- **First consumer ships with it.** A dev map with two room copies, a two-way link pair and a two-way seamless pair holds both set-pieces, with the barrier ghosted by tag: the barrier puzzle and the battle swap. It has a TypeScript script and a Luau twin, following the `teleporters.map` / `teleporters-luau.map` precedent. An author section joins `docs/teleporters.md` or a new `docs/ghost-links.md` (executor's call).
- **Non-goals.**
  - Phase or timeline tags and interaction filtering of any kind.
  - Ghost impact sparks: they would reveal targets the player must not see.
  - Hitscan tracers and muzzle flashes: no peer sees them today, even unghosted. They are separate weapon-VFX work.
  - Remote footsteps: no machine plays another player's movement sounds today (roadmap Epic 12 step 4).
  - Suppressing the real voice: the author doc tells mappers to place copies farther apart than the attenuation `maxDistance`.
  - Per-link filter chains.
  - Ghosting runtime-spawned entities and world geometry.
  - Lit ghosts.

### Scripting surface
```typescript
import { defineReaction, getMapEntities, setGhostAudio } from "postretro";

export function setupLevel() {
  const echoes = getMapEntities("ghostLink", { tag: "vault_echo" }); // map order
  const reveal = defineReaction("echo.reveal", {
    sequence: [
      ...echoes.flatMap((l) => l.arm()), // also: l.disarm()
      // Every field optional; an omitted field keeps its current value.
      // periodMs defaults to 0 (immediate).
      ...echoes.flatMap((l) =>
        l.setLook({ tint: { x: 1.0, y: 0.4, z: 0.3 }, opacity: 0.5, desaturate: 0.6, periodMs: 1500 }),
      ),
      setGhostAudio({ volume: 0.8, highPassHz: 300, lowPassHz: 2500, drive: 0.4, periodMs: 1500 }),
    ],
  });
  return {
    reactions: [reveal],
    triggerEvents: getMapEntities("trigger", { tag: "echo_plate" }).map((t) => t.on("enter", [reveal])),
  };
}
```
`setLook` and `arm`/`disarm` return `SequenceStep[]`, and `setGhostAudio` returns one step. Each member also carries `id`, `position` and `tags`. Manifest: `defineMod({ audio: { attenuation, ghost: { volume, highPassHz, lowPassHz, drive } }, ghostLook: { tint, opacity, desaturate } })`, every field optional. Ranges: `tint` components, `opacity`, `desaturate`, `volume` and `drive` lie in 0..=1. Filter cutoffs lie in 20..=20000 Hz, with `highPassHz` below `lowPassHz`. `volume` maps on the existing perceptual taper. An out-of-range or non-finite script argument warns and drops that step. The Luau mirror ships too: `l:arm()`, `l:setLook({...})`.

## Acceptance
### Automated
**Compile and load**
- [ ] A map with a `ghost_link` round-trips its records, ghost tags included, through the PRL section.
- [ ] A missing or unknown `landmark` or `target`, a `landmark` equal to `target`, a link with no brushes and a bad `enabled_on_spawn` each fail with the author-facing error.
- [ ] A `ghost_tags` entry matching no mover or placement warns once at level install, and the level loads.
- [ ] Decode rejects each case the Wire format names.
- [ ] Adding a link to a map leaves the level content digest unchanged, while adding a teleporter still changes it.
**Mapping and membership**
- [ ] For rotated and unrotated pairs, with vertical offset, a ghost pose equals the pose a seamless teleport through the same points produces.
- [ ] Membership edges:
  - presented point just inside vs just outside the volume
  - partner pawn ghosted while an untagged monster beside it is not
  - a mover and a placement carrying a listed tag ghosted
  - the same tagged barrier inside two links ghosts through the link that lists its tag and not the one that doesn't
  - projectile ghosted with `ghost_projectiles` 1 and not with 0
  - with `ghost_projectiles` 1, a tagged enemy's projectile ghosts and an untagged enemy's does not
  - viewer's own pawn absent from its own ghost set while the partner sees it
  - an entity inside two armed links yields two ghosts
- [ ] Disarmed link: no ghosts. Arm then disarm on consecutive ticks shows ghosts for exactly the armed span. Arm and disarm in one tick ends in the last call's state.
- [ ] A ghost draws when its mapped cell is visible and not when it isn't, both with the real cell visible and without.
- [ ] Two pawns swapped on one tick by arming both seamless teleporters: on the cut frame each ghost appears in the other copy, and no frame draws a ghost between the two poses.
**Look and audio values**
- [ ] Look resolution: engine default, then manifest, then link key, then `setLook`. Each layer overrides the one before, and an omitted `setLook` field keeps its value.
- [ ] Manifest resolution: a malformed `ghostLook` or `audio.ghost` field applies the whole engine default for that block and warns naming the field. A `highPassHz` at or above `lowPassHz` counts as malformed.
- [ ] A `periodMs` ramp presents the midpoint value halfway through. A second set mid-ramp starts from the presented value. `periodMs` 0 lands in one frame.
- [ ] A fire sound anchored at a ghosted pawn in a `ghost_audio` link admits a second voice at the mapped point on the Ghost bus. The same link with `ghost_audio` 0 admits none.
- [ ] An impact sound is ghosted only with `ghost_impact_audio` 1 and an owner that is a member of the link. Contacts outside the volume are dropped.
- [ ] Ghost voices are refused at the Ghost bus cap and release their slot on finish, stop and level unload.
**Replication**
- [ ] Host `arm`, `setLook` and `setGhostAudio` reach a connected client, and the client's ramp ends when the host's does.
- [ ] A late joiner gets current armed state and look, not `enabled_on_spawn` and defaults.
- [ ] The same steps run on a connected client change nothing.
- [ ] Demotion and level unload drop received link and audio state.
- [ ] A handshake across the old and new wire versions is refused.
**Surface fixture**
- [ ] The dev map's TypeScript and Luau scripts, which contain the Scripting surface example, load and run in a test with zero warnings. Their set-pieces drive the arm, look and audio changes the rows above observe.
- [ ] A headless frame capture of a ghost in front of opaque geometry shows tinted, blended pixels with the geometry behind still visible.
### Manual
- [ ] Two-peer loopback co-op on the dev map, run once as listen host and once as client, at the harness lag profile. Barrier puzzle: one player presses the button in copy B, sees the ghost barrier open, and the player in copy A passes. Battle swap: each player sees only their own monsters plus the partner's ghost, aim pitch and weapon. After the swap each faces the other set.
- [ ] Ghost audio feel: the partner's fire through the default bus reads punchy and muffled, and a scripted ramp is audible.
- [ ] Ghost tint stays readable against both copies' lighting and under the high-contrast theme. Tune the engine defaults here and record them.

## Path
- The thin slice falsifies the riskiest seam. It covers a link section, a local membership pass, one translucent skinned-mesh ghost of the partner pawn, and a two-peer loopback swap. That proves the presented pose, the teleport cut and mapped culling compose before any surface fans out.
- Precedents to follow:
  - Teleporter compile, section and install: `TeleporterParser`, `TeleportersSection`, `decode_teleporters`, `spawn_loaded_teleporters`, `wrapTeleporterEntity`.
  - Brush volumes: `environment_volumes::resolve_leaf`.
  - Teleporter replication: `WireTeleporterState`, `collect_payloads`, `crates/netcode/src/client/teleporters.rs`.
  - Host-only no-op: `group_commands_apply_here`.
  - Ramp rebasing: `apply_presentation_payload`, `presentation_host_time`.
  - Manifest: `resolve_authored_attenuation`.
  - Global step: `setSentiment`.
- Rendering: ghost skinned instances ride the existing batch as extra `MeshInstanceInput`s, the way `attachments::emit_for_holder` adds them, and need a ghost `MeshPaletteCacheKey` variant. Mover upload keys by `mover_id`, and `active_draw_lookup` keeps one instance per id, so a mover ghost needs its own instance path. The instance structs carry no tint or alpha today.
- Audio: anchored voices are parented to SFX in `spatial.rs::start_voice`. `AudioCore::play` drops anchored non-SFX requests, and voice release assumes `BusId::Sfx`. All three need the bus a voice took. Ghost requests derive at the drain, where `AnchorScene::sound_anchor` resolves emitters. A ghost voice uses a `Point` anchor, because an `Entity` anchor would follow the real pose. The Ghost bus chain uses kira 0.12.5's built-in effects: `FilterBuilder` (`FilterMode::HighPass`, `LowPass`), `DistortionBuilder` and `VolumeControlBuilder`, added with `TrackBuilder::add_effect` so their handles are kept (`with_effect` discards them). Distortion drive is in decibels, so the 0..=1 `drive` maps onto a dB range the executor picks.
- Rival weighed: membership set by script at arm time (`l.arm({ tags, ... })`). Rejected for v1: map-data membership lets every peer resolve the same members locally, which keeps membership off the wire. A script override can be added later.
- Rival weighed: a per-entity `ghost` KVP. It is simpler to author, but it cannot vary by link, and it needs a mixed mover-id / placement-ordinal table, because movers carry no id-29 ordinal. Tags already address both.
- Resolve `ghost_tags` against the same tag index `getMapEntities` queries, once per level install.
- Files past ~800 lines this extends are split first, in their own commits. Likely candidates: `crates/net/src/wire.rs`, `crates/netcode/src/replication.rs`, `crates/level-compiler/src/parse.rs`, `crates/renderer/src/render/mesh_pass.rs`.

## Open questions
- Whether a contact-set emission carries its shot owner at the drain, so impact ghosting can check membership — **delegated**: if it does not, the executor carries the owner through and reports it in the plan of record.
- The shared mapping function's crate home: physics, or somewhere the renderer-feeding presentation code can reach — **delegated**.
- Engine default look and Ghost bus values, and the Ghost bus voice cap — **delegated**: set in the manual pass and recorded in the plan of record.

## Wire format
**PRL `GhostLinks` (id 56, next free).** Mirrors Teleporters (id 55): little-endian, `u32` counts and indices, `f32` scalars, convex leaves encoded as id 55 encodes them. Optional: present when the map has at least one `ghost_link`; absent means no links. Presentation only: the level content digest does not hash it.
- Header: `u16` internal version (1), `u32` link count, capped at 65,535 with `0xFFFF` reserved as the no-link index.
- Per link, in map entity order:
  - source entity
  - `u8` enabled_on_spawn
  - `u8` flags: bit 0 `ghost_projectiles`, bit 1 `ghost_audio`, bit 2 `ghost_impact_audio`; other bits must be 0
  - `u8` look-override mask: bit 0 tint, bit 1 opacity, bit 2 desaturate; then each present value in that order (tint as three `f32`)
  - convex leaves
  - landmark origin and yaw, then target origin and yaw
  - `u32` ghost-tag count, then each tag as a length-prefixed string encoded as id 55 encodes strings, in authored order, deduplicated. Count 0 means players only.
- Decode rejects:
  - another version, or a link count of 0 or past the cap
  - unknown flag or mask bits
  - a look value outside 0..=1 or non-finite
  - a non-finite pose
  - an empty or duplicated ghost tag
  - a payload of the wrong length

**Snapshot payloads (`WIRE_VERSION` 33, `SNAPSHOT_VERSION` 22).** Per link: map-order index (`u16`), source entity, armed, and the look ramp (from-look, to-look, host start time, duration ms). Level-scoped: the ghost audio ramp (from-params, to-params, host start time, duration ms). Bound on the client by map-order index, like `WireTeleporterState`. The level-scoped record's identity scheme is the executor's (one registered level entity, or a dedicated binding).

## Boundary inventory
| Name | Rust | Wire / PRL | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Ghost link entity | new component kind | PRL id 56 record; link-state payload | `getMapEntities("ghostLink")` | `getMapEntities("ghostLink")` | `ghost_link` |
| Landmark / target | — | poses | — | — | `landmark`, `target` |
| Spawn armed | — | `enabled_on_spawn` byte | — | — | `enabled_on_spawn` (0/1, default 1) |
| Projectile ghosts | — | flag bit 0 | — | — | `ghost_projectiles` (0/1, default 0) |
| Ghost audio | — | flag bit 1 | — | — | `ghost_audio` (0/1, default 0) |
| Impact ghost audio | — | flag bit 2 | — | — | `ghost_impact_audio` (0/1, default 0) |
| Look override | — | mask + values | `setLook({ tint, opacity, desaturate, periodMs })` | `l:setLook({...})` | `ghost_tint` (`"r g b"` 0..1), `ghost_opacity`, `ghost_desaturate` |
| Ghosted tags | — | per-link tag list | — | — | `ghost_tags` (whitespace-delimited, like `_tags`; default empty) |
| Arm / disarm | host-only commands | armed in link-state payload | `arm()`, `disarm()` | `l:arm()`, `l:disarm()` | — |
| Ghost audio bus | new bus | level audio payload | `setGhostAudio({ volume, highPassHz, lowPassHz, drive, periodMs })` | `setGhostAudio({...})` | — |
| Manifest look | resolved defaults | — | `ghostLook` | `ghostLook` | — |
| Manifest audio | resolved defaults | — | `audio.ghost` | `audio.ghost` | — |
