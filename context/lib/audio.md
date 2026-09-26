# Audio

> **Read this when:** working on the audio subsystem, adding or positioning sound events, authoring sound fields, or integrating reverb zones.
> **Key invariant:** audio subsystem never touches wgpu or renderer types. It receives listener state and sound event requests; it produces audio output internally via kira.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) · [Build Pipeline](./build_pipeline.md)

---

## 1. Subsystem Boundary

Audio is a self-contained subsystem. It does not depend on the renderer or wgpu.

| Direction | Data |
|-----------|------|
| **Receives** | Listener position and orientation, sound event requests |
| **Produces** | Audio output (mixed and delivered to the OS by kira internally) |

The boundary carries primitive types only. `ListenerState` (position + forward/up as `[f32; 3]`; world up is `[0, 1, 0]`) and `SoundRequest` (target bus, sound key, looping flag) cross the public API — no glam, no wgpu. Conversion from the glam-typed `Camera` happens at the frame-loop call site, not inside the module. A runtime cell id is not yet part of the boundary; it will be added when reverb zone lookup is implemented. Decided, not yet built: a request carries an optional anchor, which is an entity, a world point, or an impact's contact set. The sim supplies emitter identity and contact data only and never names an audio type.

Init is fault-tolerant: if the device or kira backend fails to start, the subsystem holds `None` and the game runs silent — never a crash, never a panic. Asset load and decode failures degrade the same way (warn, skip, no sound).

Sound assets load at level install time from `content/<mod>/sounds/<collection>/<name>.{ogg,wav}`. The sound registry follows level lifetime — populated at level install, released at unload. Static clips (all non-`music/` collections) are decoded into memory; music streams from disk via kira's streaming path.

### Mixer bus tree

kira's main track serves as Master. SFX, Music, and UI hang off it as sub-tracks, each with a runtime volume control (`set_bus_volume`). In-world sound categories route to one of these buses. A per-bus active-voice cap bounds concurrency; the sum of per-bus caps stays within kira's provisioned budget so play commands accepted by the voice counter always find a kira slot. Decided, not yet built: kira removes finished sounds and tracks on its audio thread one to two blocks after the engine reclaims them. SFX admission therefore checks kira's live slot occupancy as well as the engine's count. A slot the engine has reclaimed but kira still holds counts as occupied, so the counter never disagrees with the mixer. Over the cap a request is refused, never queued.

---

## 2. Playback Crate

kira 0.12 handles playback and mixing. Engine code configures tracks and spatial parameters through kira's API. kira pulls glam 0.32 transitively; its math types do not cross into engine code. The audio subsystem boundary uses primitive types (f32 arrays) — no glam types in the public API.

---

## 3. Frame Integration

Audio runs third in frame order: Input → Game logic → **Audio** → Render → Present.

Each frame, the audio step:
1. Updates listener position and orientation from camera/player state.
2. Reclaims finished non-looping voices so bus capacity is not leaked.

Decided, not yet built: the listener is the rendered eye — the interpolated eye including the view-feel offset, evaluated once per frame and shared by audio and render. Moving that evaluation ahead of the audio step keeps Audio → Render order. The step also repositions live anchored voices, and resolves an impact's contact set to the contact nearest this frame's listener.

The step is control-plane only — it never decodes or touches disk. kira manages its own audio thread; per-frame work is listener updates and voice reclamation. The `dt` parameter (frame delta in seconds) is part of the per-frame contract but is currently unused — it will drive spatialization tweening once that is implemented.

---

## 4. Sound Triggering

Callers emit `SoundRequest` values targeting a named bus. `Audio::play` resolves the bus and sound key, routes to the bus's kira sub-track, and returns an opaque `SoundHandle`. `Audio::stop` stops the sound and releases its voice slot. Looping sounds repeat until stopped; one-shot sounds release their voice automatically once kira reports them finished.

| Event | Example trigger |
|-------|-----------------|
| Footstep | Player or entity movement tick |
| Gunshot | Weapon fire |
| Explosion | Projectile impact |
| Pickup | Item collection |
| Door | Door open/close |
| Impact | Projectile hitting a surface |

Surface-material-aware routing (varying footstep/impact sounds by texture prefix) and the shared material enum with the renderer's decal system are later goals. Impact events already carry each contact's normal and what it hit (an entity or world geometry), so surface routing needs no new plumbing.

### Sound sources (decided, not yet built)

Gameplay sounds are presentation. They resolve on the app drain after the tick loop and stay host-local: no sound key or sound request enters the wire or the replicated snapshot. There are two authoring paths, and both play when they name the same event.

| Path | Surface | Use |
|------|---------|-----|
| Descriptor sounds | Sound fields on weapons, player movement, enemy attacks and behavior states; `*_sound` KVPs on movers | The common path. Declared on the thing that makes the sound, with no script. The only per-instance path where the engine owns the event name (weapon fire, impact, enemy attack), because a reaction addressed to such a name is global. |
| Positional reaction | `playSound(key, { at: on.emitter })` | Scripted cases. The emitter token and its scope rules are in `scripting.md` §12. |

- **Weapon sounds follow the weapon, whoever wields it.** An enemy attack that names a weapon plays that weapon's sounds. A projectile records at spawn the weapon it was fired from, so its contact resolves sounds from the projectile, not from the shooter.
- **An impact is one event per activation per tick**, carrying every contact of that tick. A multi-pellet hitscan shot yields one impact sound; each projectile contact yields its own. Projectile and hitscan contacts both fire `impact` reactions, whoever fired them.
- **Anchors:**
  - Fire, dry fire and reload anchor at the firing pawn.
  - Enemy attack and state entry anchor at the enemy.
  - Movement events anchor at the local pawn.
  - A mover anchors at the center of its current world bounds, because mover transforms are origin-relative.
  - Every anchor captures a point at fire time, so an emitter despawned the same tick still has a position.
- **Unknown sound keys** are checked after the registry loads, at install and at each hot-reload commit. An unknown key warns once, and the level still loads. The play-time drop remains as a backstop.
- **A connected client hears its own actions.** Fire, dry fire and impacts come from its fire prediction. Reload edges derive from its replicated reload and ammo state (`networking.md` §Combat authority). Landing and jumping come from its predicted movement. Remote peers' and world sounds are host-local until peer audio lands.

---

## 5. Spatial Positioning

The listener anchor and orientation are established and updated each frame (position + forward/up via `update`). Today no spatialization is applied, and all sounds play dry.

Decided, not yet built:

| Parameter | Behavior |
|-----------|----------|
| Placement | Each positional sound plays on its own kira spatial track under the SFX bus, so the SFX volume control governs it. |
| Distance attenuation | Falls off between a minimum and maximum distance along a curve. These are set mod-wide in the manifest's `audio.attenuation` block, with an engine-seeded default. A malformed value warns naming the field and falls back to the default, like other manifest profiles. Attenuation applies when a sound starts; live sounds keep theirs. |
| Stereo panning | Left/right balance from the sound's direction relative to listener facing. Distance gain plus two-ear panning; no Doppler, no HRTF. |
| Position tracking | An entity anchor follows the entity's render-interpolated pose each frame. Once the entity is gone, the sound freezes at its last position and plays out; a tail is never cut. |
| Own pawn | A sound anchored on the pawn the listener is attached to plays non-spatial on SFX. A sound keeps the treatment it started with. |

**One spatial pipeline (decided, not yet built).** Every positional play goes through one chokepoint in the audio module. It owns each voice's anchor, and the voice's direction and distance relative to the listener, computed engine-side each frame. It is the only code that touches kira's spatial tracks. Directional cues, such as front/back filtering and distance-based stereo spread, and occlusion extend this chokepoint in place. There is no second pipeline and no per-hardware renderer tier.

**Level lifetime.** Unload, restart and return-to-frontend stop every positional voice with a short fade, so no sound outlives its world or follows an entity from the next level.

---

## 6. Reverb Zones

> **Not yet implemented — future goal.** Nothing in this section is shipped. The design below captures the intended architecture for mapper-placed reverb volumes.

Reverb will vary spatially through mapper-placed brush entities.

### Entity: `env_reverb_zone`

Will be placed in TrenchBroom via custom FGD. Mappers paint acoustic regions as brush volumes.

| Property | Purpose |
|----------|---------|
| `reverb_type` | Preset category (hall, tunnel, room, outdoor, etc.) |
| `decay_time` | How long reverb tail persists |
| `occlusion_factor` | How much geometry between source and listener dampens sound |

### Runtime Cell Resolution

Each `env_reverb_zone` brush will resolve to runtime cell IDs. It must use the `Cells`/`CellLocator` id space, not BSP leaf sections.

At runtime, audio will look up the listener's current runtime cell and check which (if any) reverb zone contains that cell. Reverb parameters will apply per cell — a listener crossing from one zone to another gets the new zone's parameters immediately.

**Overlap rule:** when a runtime cell belongs to multiple reverb zones, the smallest zone (fewest cells) wins. A small tunnel zone inside a large outdoor zone produces tunnel reverb, not outdoor. No blending between zones — transitions are immediate on cell crossing.

Cells outside any reverb zone will get no reverb effect (dry signal only).

---

## 7. Non-Goals

- HRTF (head-related transfer function) processing
- Doppler shift
- Surround (5.1/7.1) output — kira mixes stereo only
- Real-time acoustic simulation or ray-traced audio
- Ambisonics
- Dynamic music system (adaptive soundtrack layers)
- Audio recording or capture
- Multiplayer voice chat
