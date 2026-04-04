# Audio

> **Read this when:** working on the audio subsystem, adding sound events, or integrating reverb zones.
> **Key invariant:** audio subsystem never touches wgpu or renderer types. It receives listener state and sound event requests; it produces audio output internally via kira.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) · [Build Pipeline](./build_pipeline.md)

---

## 1. Subsystem Boundary

Audio is a self-contained subsystem. It does not depend on the renderer or wgpu.

| Direction | Data |
|-----------|------|
| **Receives** | Listener position and orientation, sound event requests, current BSP leaf index (for reverb lookup) |
| **Produces** | Audio output (mixed and delivered to the OS by kira internally) |

Sound assets load at level load time. No streaming from disk during gameplay.

---

## 2. Playback Crate

kira 0.12 handles playback and mixing. Engine code configures tracks, spatial parameters, and reverb effects through kira's API. kira pulls glam 0.32 transitively; its math types do not cross into engine code. Audio subsystem boundary uses primitive types (f32 arrays, tuples) — no glam types in the public API.

---

## 3. Frame Integration

Audio runs third in frame order: Input → Game logic → **Audio** → Render → Present.

Each frame, audio:
1. Updates listener position and orientation from camera/player state.
2. Processes sound event requests emitted by game logic.
3. Updates spatial parameters for active sounds.

Audio never blocks the frame. kira manages its own audio thread; the per-frame work is parameter updates and playback triggers.

---

## 4. Sound Triggering

Game logic emits sound event requests. Audio processes them.

| Event | Example trigger |
|-------|-----------------|
| Footstep | Player or entity movement tick |
| Gunshot | Weapon fire |
| Explosion | Projectile impact |
| Pickup | Item collection |
| Door | Door open/close |
| Impact | Projectile hitting a surface |

Each request carries: sound category, world position, and (where relevant) surface material type.

### Surface Material Sounds

Texture name prefix maps to a material enum. Footstep and impact sounds vary by material. The material lookup is shared with the renderer's decal system — one prefix table, one enum. Which prefixes exist is a game content concern; see `resource_management.md` §3.

---

## 5. Spatial Positioning

All in-world sounds are positioned in 3D relative to the listener.

| Parameter | Behavior |
|-----------|----------|
| Distance attenuation | Sounds fall off with distance from listener |
| Stereo panning | Left/right balance derived from sound direction relative to listener facing |
| Position tracking | Active sounds update position each frame if their source moves |

---

## 6. Reverb Zones

Reverb varies spatially through mapper-placed brush entities.

### Entity: `env_reverb_zone`

Placed in TrenchBroom via custom FGD. Mappers paint acoustic regions as brush volumes.

| Property | Purpose |
|----------|---------|
| `reverb_type` | Preset category (hall, tunnel, room, outdoor, etc.) |
| `decay_time` | How long reverb tail persists |
| `occlusion_factor` | How much geometry between source and listener dampens sound |

### BSP Leaf Resolution

At level load time, each `env_reverb_zone` brush is resolved to the set of BSP leaves it contains. This is the same resolution strategy used for fog volumes.

At runtime, audio looks up the listener's current BSP leaf and checks which (if any) reverb zone contains that leaf. Reverb parameters apply per leaf — a listener crossing from one zone to another gets the new zone's parameters immediately.

**Overlap rule:** when a BSP leaf belongs to multiple reverb zones, the smallest zone (fewest leaves) wins. This means a small tunnel zone inside a large outdoor zone produces tunnel reverb, not outdoor. No blending between zones — transitions are immediate on leaf crossing.

Leaves outside any reverb zone get no reverb effect (dry signal only).

---

## 7. Non-Goals

- HRTF (head-related transfer function) processing
- Real-time acoustic simulation or ray-traced audio
- Ambisonics
- Dynamic music system (adaptive soundtrack layers)
- Audio recording or capture
- Multiplayer voice chat
