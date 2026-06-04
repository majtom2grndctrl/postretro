# Audio

> **Read this when:** working on the audio subsystem, adding sound events, or integrating reverb zones.
> **Key invariant:** audio subsystem never touches wgpu or renderer types. It receives listener state and sound event requests; it produces audio output internally via kira.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) · [Build Pipeline](./build_pipeline.md)

**Status: not yet implemented.** `kira` is declared in the workspace `Cargo.toml` but no audio module exists in engine source. This doc describes intended design.

---

## 1. Subsystem Boundary

Audio will be a self-contained subsystem. It will not depend on the renderer or wgpu.

| Direction | Data |
|-----------|------|
| **Receives** | Listener position and orientation, sound event requests, current BSP leaf index (for reverb lookup — later goal, not yet consumed) |
| **Produces** | Audio output (mixed and delivered to the OS by kira internally) |

The boundary carries primitive types only. `ListenerState` (position + forward/up as `[f32; 3]`; world up is `[0, 1, 0]`) and `SoundRequest` (target bus, sound key, looping flag) cross the public API — no glam, no wgpu. Conversion from the glam-typed `Camera` happens at the frame-loop call site, not inside the module.

Init is fault-tolerant: if the device or kira backend fails to start, the subsystem disables itself and the game runs silent — never a crash, never a panic. Asset load and decode failures degrade the same way (warn, skip, no sound).

Sound assets will load at level load time from `content/<mod>/_sounds/`. The sound registry follows level lifetime — populated at level load, released at unload. No streaming from disk during gameplay beyond kira's own music streaming.

### Mixer bus tree

kira's main track serves as Master. SFX, Music, and UI hang off it as sub-tracks, each with a runtime volume control. In-world sound categories route to one of these buses. A per-bus active-voice cap bounds concurrency; the sum of per-bus caps stays within kira's global sound capacity so play commands never silently drop at the kira layer.

---

## 2. Playback Crate

kira 0.12 will handle playback and mixing. Engine code will configure tracks, spatial parameters, and reverb effects through kira's API. kira pulls glam 0.32 transitively; its math types will not cross into engine code. The audio subsystem boundary will use primitive types (f32 arrays, tuples) — no glam types in the public API.

---

## 3. Frame Integration

Audio will run third in frame order: Input → Game logic → **Audio** → Render → Present.

Each frame, audio will:
1. Update listener position and orientation from camera/player state.
2. Process sound event requests emitted by game logic.
3. Update spatial parameters for active sounds.

Audio must never block the frame. kira will manage its own audio thread; per-frame work is parameter updates and playback triggers.

---

## 4. Sound Triggering

Game logic will emit sound event requests. Audio will process them.

| Event | Example trigger |
|-------|-----------------|
| Footstep | Player or entity movement tick |
| Gunshot | Weapon fire |
| Explosion | Projectile impact |
| Pickup | Item collection |
| Door | Door open/close |
| Impact | Projectile hitting a surface |

Each request will carry: sound category, world position, and (where relevant) surface material type.

### Surface Material Sounds

Texture name prefix will map to a material enum. Footstep and impact sounds will vary by material. The material lookup will be shared with the renderer's decal system — one prefix table, one enum. Which prefixes exist is a game content concern; see `resource_management.md` §3.

---

## 5. Spatial Positioning

All in-world sounds will be positioned in 3D relative to the listener.

| Parameter | Behavior |
|-----------|----------|
| Distance attenuation | Sounds fall off with distance from listener |
| Stereo panning | Left/right balance derived from sound direction relative to listener facing |
| Position tracking | Active sounds will update position each frame if their source moves |

---

## 6. Reverb Zones

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
