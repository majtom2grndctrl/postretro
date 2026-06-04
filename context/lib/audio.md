# Audio

> **Read this when:** working on the audio subsystem, adding sound events, or integrating reverb zones.
> **Key invariant:** audio subsystem never touches wgpu or renderer types. It receives listener state and sound event requests; it produces audio output internally via kira.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) · [Build Pipeline](./build_pipeline.md)

---

## 1. Subsystem Boundary

Audio is a self-contained subsystem. It does not depend on the renderer or wgpu.

| Direction | Data |
|-----------|------|
| **Receives** | Listener position and orientation, sound event requests |
| **Produces** | Audio output (mixed and delivered to the OS by kira internally) |

The boundary carries primitive types only. `ListenerState` (position + forward/up as `[f32; 3]`; world up is `[0, 1, 0]`) and `SoundRequest` (target bus, sound key, looping flag) cross the public API — no glam, no wgpu. Conversion from the glam-typed `Camera` happens at the frame-loop call site, not inside the module. A BSP leaf index is not yet part of the boundary; it will be added when reverb zone lookup is implemented.

Init is fault-tolerant: if the device or kira backend fails to start, the subsystem holds `None` and the game runs silent — never a crash, never a panic. Asset load and decode failures degrade the same way (warn, skip, no sound).

Sound assets load at level install time from `content/<mod>/_sounds/<collection>/<name>.{ogg,wav}`. The sound registry follows level lifetime — populated at level install, released at unload. Static clips (all non-`music/` collections) are decoded into memory; music streams from disk via kira's streaming path.

### Mixer bus tree

kira's main track serves as Master. SFX, Music, and UI hang off it as sub-tracks, each with a runtime volume control (`set_bus_volume`). In-world sound categories route to one of these buses. A per-bus active-voice cap bounds concurrency; the sum of per-bus caps stays within kira's provisioned budget so play commands accepted by the voice counter always find a kira slot.

---

## 2. Playback Crate

kira 0.12 handles playback and mixing. Engine code configures tracks and spatial parameters through kira's API. kira pulls glam 0.32 transitively; its math types do not cross into engine code. The audio subsystem boundary uses primitive types (f32 arrays) — no glam types in the public API.

---

## 3. Frame Integration

Audio runs third in frame order: Input → Game logic → **Audio** → Render → Present.

Each frame, the audio step:
1. Updates listener position and orientation from camera/player state.
2. Reclaims finished non-looping voices so bus capacity is not leaked.

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

Surface-material-aware routing (varying footstep/impact sounds by texture prefix) and the shared material enum with the renderer's decal system are later goals.

---

## 5. Spatial Positioning (future goal)

The listener anchor and orientation are established and updated each frame (position + forward/up via `update`); no spatialization is applied yet — all sounds play dry. The parameters below describe the intended behavior once spatialization is implemented.

| Parameter | Behavior |
|-----------|----------|
| Distance attenuation | Sounds will fall off with distance from listener |
| Stereo panning | Left/right balance derived from sound direction relative to listener facing |
| Position tracking | Active sounds will update position each frame if their source moves |

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

### BSP Leaf Resolution

At level load time, each `env_reverb_zone` brush will be resolved to the set of BSP leaves it contains. This is the same resolution strategy intended for fog volumes.

At runtime, audio will look up the listener's current BSP leaf and check which (if any) reverb zone contains that leaf. Reverb parameters will apply per leaf — a listener crossing from one zone to another gets the new zone's parameters immediately.

**Overlap rule:** when a BSP leaf belongs to multiple reverb zones, the smallest zone (fewest leaves) wins. A small tunnel zone inside a large outdoor zone produces tunnel reverb, not outdoor. No blending between zones — transitions are immediate on leaf crossing.

Leaves outside any reverb zone will get no reverb effect (dry signal only).

---

## 7. Non-Goals

- HRTF (head-related transfer function) processing
- Real-time acoustic simulation or ray-traced audio
- Ambisonics
- Dynamic music system (adaptive soundtrack layers)
- Audio recording or capture
- Multiplayer voice chat
