# Audio

> **Read this when:** working on sound playback, sound events and their placement, authored sound fields, reverb zones, or audio accessibility — volume and mono options, captions, subtitles, sound-direction cues.
> **Key invariant:** audio never touches wgpu or renderer types. It takes listener state and local sound requests; kira produces output inside the audio crate. Gameplay sound playback resolves after the tick loop; frozen observer weapon cues may carry keys across the wire.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) · [Build Pipeline](./build_pipeline.md) · [Scripting](./scripting.md) §12 · [Networking](./networking.md) §Combat authority · [Player Options](./player_options.md) §5

---

## 1. Subsystem Boundary

Audio is a self-contained subsystem in its own crate, `postretro-audio` (`crates/audio/`), the only crate that names kira. It depends on no other workspace crate, and only the binary depends on it (`layering_invariants_hold` pins that no sim-side crate does).

| Direction | Data |
|-----------|------|
| **Receives** | Listener pose and the pawn it is attached to, sound requests, a per-frame entity-position resolver, the mod's attenuation |
| **Produces** | Audio output (mixed and delivered to the OS by kira internally) |

The boundary carries primitive types only: no glam, no kira, no engine ids.

- `ListenerState`: position and forward/up as `[f32; 3]` (world up is `[0, 1, 0]`), plus the opaque key of the pawn the listener is attached to.
- `SoundRequest`: target bus, sound key, looping flag, and an optional `SoundAnchor`.
- `SoundAnchor`: an entity (opaque key plus fire-time point), a fixed world point, or an impact's contact points. Every variant carries a point, so an emitter gone before the audio step still has a position.

Entity keys are opaque to audio. The app encodes an entity id with its generation, so a key never resolves to a later entity reusing the slot. Conversion from glam and entity types happens app-side (`crates/postretro/src/sound_events/`), never inside the crate. The sim supplies emitter identity and contact data only and never names an audio type. A runtime cell id is not yet part of the boundary; reverb zone lookup will add it.

Init is fault-tolerant: if the device or kira backend fails to start, the subsystem holds `None` and the game runs silent — never a crash, never a panic. Asset load and decode failures degrade the same way (warn, skip, no sound).

Sound assets load at level install time from `content/<mod>/sounds/<collection>/<name>.{ogg,wav}`. The sound registry follows level lifetime — populated at level install, released at unload. Static clips (all non-`music/` collections) are decoded into memory; music streams from disk via kira's streaming path.

### Mixer bus tree

kira's main track serves as Master. SFX, Music, and UI hang off it as sub-tracks, each with a runtime volume control (`set_bus_volume`). In-world sound categories route to one of these buses. A per-bus active-voice cap bounds concurrency.

**The voice counter never disagrees with kira.** A request is admitted only when both the engine's voice counter and kira's live slot occupancy on that bus have room. kira frees a finished sound's slot on its own audio thread, after the engine reclaims the voice, so a slot kira still holds counts as occupied. Positional voices admitted but not yet started count too. Each bus's kira pools — sounds, and for SFX the spatial child tracks — are sized to twice its voice cap, so a full cap of new requests finds slots while kira releases the previous cap's. Over the cap a request is refused with a warning, never queued. A request the counter admits never fails for want of a kira slot.

Master, SFX, Music, and UI volumes are player options (`player_options.md` §5) — Master scales the main track, each other scales its own bus. The App applies them, with mono, at session build and again whenever the resolved preferences change. Volumes are stored linear `[0, 1]` and mapped here on the perceptual taper, 40·log₁₀ v dB (gain v²); 0 silences, 1.0 is unity gain. A mono option folds left and right in an effect on the main track, after spatialization, so a hard-panned source reaches both ears. Toggling it crossfades over 20 ms rather than stepping (the floor is 10 ms), and a toggle reversed mid-crossfade turns back from the current mix. Settled stereo passes through the effect untouched.

---

## 2. Playback Crate

kira 0.12 handles playback, mixing, and spatialization. Engine code configures tracks and spatial parameters through kira's API, only inside `postretro-audio`. kira pulls glam 0.33 transitively; its math types do not cross into engine code.

---

## 3. Frame Integration

Audio runs third in frame order: Input → Game logic → **Audio** → Render → Present.

After the tick loop, the app drain turns the frame's gameplay events into sound requests (§4). Before playing them it names this frame's listener pawn (`set_listener_attached`). Own-pawn treatment is decided when a sound is admitted, which precedes the audio step, so without this the first sounds after a level load or respawn would be judged against the previous pawn.

**The listener is the rendered eye.** The frame eye — the interpolated eye plus the view-feel offset — is evaluated once per frame, ahead of the audio step, and render and the listener both read that one result (`crates/postretro/src/frame_eye.rs`). View feel therefore advances once per frame. Audio → Render order is unchanged.

The audio step then runs, in order:
1. Re-anchors the listener to the rendered eye and records its pawn. A non-finite position or orientation keeps the last finite one.
2. Reclaims finished voices. A voice that finished frees its slot here, not earlier in the frame, so a request made earlier in the frame is still refused at the cap.
3. Moves each tracked positional voice to its entity's presented position (§5).
4. Starts the positional voices admitted since the last step, against this frame's listener.

The step is control-plane only — it never decodes or touches disk. kira manages its own audio thread. The frame delta paces each reposition tween, capped so a long hitch snaps a source rather than sliding it across the gap.

---

## 4. Sound Triggering

Callers emit `SoundRequest` values targeting a named bus. `Audio::play` resolves the bus and sound key, admits the request against the voice budget, and returns an opaque `SoundHandle`. `Audio::stop` stops the sound and releases its voice slot. Looping sounds repeat until stopped; one-shot sounds release their voice once kira reports them finished.

An unanchored request starts at once on its bus. An anchored request must be a one-shot on SFX, or it is dropped with a warning. It is admitted at `play` and starts at the next audio step (§5).

Surface-material-aware routing (varying impact sounds by surface) and a material enum shared with the renderer's decal system are later goals. Footsteps have no source event yet. Impact contacts already carry each contact's normal and what it hit (an entity or world geometry), so surface routing needs no new plumbing.

### Sound sources

Gameplay sounds are presentation. Playback resolves locally on the app drain after the tick loop (`crates/postretro/src/sound_events/`). Frozen observer weapon cues carry sound keys on the reliable Input channel; sound requests remain local, and movement tuning carries no sounds. The sim hands the drain **emissions**: each named event paired with its emitter and retained presentation provenance. An emitter is an entity plus its origin at the tick, or an impact's contact set. The emission types live in `postretro-entities`, so the sim/AI seam shares one definition.

There are two authoring paths. When both name the same event, both play; neither suppresses the other.

| Path | Surface | Use |
|------|---------|-----|
| Descriptor sounds | Sound fields on weapons, player movement, enemy attacks and behavior activities; `*_sound` KVPs on movers | Common path; no script. The only per-instance path for engine-owned event names (weapon fire, impact, enemy attack), where a reaction is global. |
| Positional reaction | `playSound(key, { at: on.emitter })` | Scripted cases. Emitter token and scope rules: `scripting.md` §12. |

| Kind | Descriptor field | Plays on |
|------|------------------|----------|
| Weapon | `sounds` { `fire`, `dryFire`, `impact`, `reloadStart`, `reloadShell`, `reloadComplete`, `overheat` } | Fire, dry fire, impact, reload start / shell loaded / completed, overheat |
| Weapon action | `primary` / `secondary` `sounds` { `fire`, `impact` } | Overrides that action's fire/impact defaults |
| Player movement | `movement.sounds` { `land`, `jump` } | Landing, jumping |
| Enemy attack | the attack's `sound` | That attack's `enemyAttack` |
| Behavior activity | the activity's `sound` | Entry, whether or not `onEnter` is authored |
| Mover | FGD `open_sound`, `close_sound`, `blocked_sound`, `crush_sound` (PRL KinematicGeometry v7, `build_pipeline.md`) | The matching mover edge; crush once per victim |

Every sound field is optional; an unknown key inside `sounds` is rejected. An event naming no sound plays nothing and warns nothing. Each shot freezes its effective fire/impact keys, including weapon-default fallbacks, and its action aliases. Delayed impact keeps those keys through switching, despawn, or hot reload. Shared reload/dry-fire/overheat sounds remain weapon-owned and resolve through the canonical-name table rebuilt at install and committed hot reload. Enemy sounds resolve from the behavior graph the brain held when the event fired. Mover keys ride the mover component, as its `*_event` addresses do.

- **Weapon sounds follow the weapon, whoever wields it.** An enemy attack that names a weapon plays its primary action's fire sound at the enemy, plus the attack's own `sound` when authored. Projectiles retain originating shot/action data, so contacts never look up the shooter's current execution. Action aliases dispatch alongside built-in `activate` / `impact` at the same emitter; descriptor sound plays once for the built-in event.
- **An impact is one event per shot per tick**, carrying every contact of that tick. A multi-pellet hitscan shot yields one impact sound; projectiles from the same shot contacting on one tick yield one; a shot with no contact yields none. Splash is not a contact. Projectile and hitscan contacts both fire `impact` reactions, whoever fired them, enemy projectiles included. On the host, a remote client's shot yields one impact carrying every validated contact (`networking.md` §Combat authority).
- **Anchors:**
  - Fire, dry fire, overheat and reload anchor at the firing pawn.
  - Enemy attack and activity entry anchor at the enemy.
  - Movement events anchor at the local pawn.
  - An impact anchors at its contact set.
  - A mover anchors at the center of its current world bounds, because mover transforms are origin-relative.
  - A player pawn places at its eye; any other entity at its transform origin.
  - An anchor's point is captured at fire time from the emitter's current pose, falling back to the origin stamped at the tick, so an emitter despawned the same tick still has a position.
- **Unknown sound keys** are checked after the registry loads, at level install and at each committed hot reload. The check covers descriptor fields, mover keys, and `playSound` reactions. An unknown key warns once, and the level still loads. The play-time drop remains as a backstop.
- **A connected client hears its own actions.**
  - Every scheduled shot resolves and declares through local execution prediction. Replicated resource/reload samples choose cosmetics only while naming the local active slot. Insufficient ammo/cell presents a dry-fire sound; a reload or overheat refusal presents nothing; otherwise frozen fire/impact keys and muzzle FX play. Hitscan keeps world contacts and normals. A cosmetically hidden predicted projectile still sweeps and declares contacts; authoritative denial ends future work (`networking.md` §Combat authority).
  - Reload edges derive from the replicated owner-private reload and ammo slots, attributed to the weapon the client holds in the host wieldable slot the reload flag names (every value read must name that slot; a frame mixing slots is held), plus that weapon's reload style and capacity, one round trip late. Start is the reload flag rising, unless the rise shows full progress — a replayed completion endpoint. A shell is ammo rising during a per-shell reload; shells that arrive in one snapshot sound once. Complete is the flag falling after the last sample held while reloading showed completion — magazine full, reserve empty, or a magazine reload at full progress — or a fall in which ammo rose by exactly what the reserve fell while the client wields that weapon. Any other fall — a cancel, a switch the host performs — plays nothing; a switch the host refuses keeps the reload tracked. Only a reload whose start the client saw on the projected weapon yields shells or a complete.
  - The overheat cue is the replicated overheat latch rising on the weapon the client holds in the named slot, while that slot is its active one. A frame whose heat values name different slots is held, and a latch first seen already raised plays nothing.
  - Landing and jumping come from its predicted movement. Movement sounds never ride the tuning payload, so the client resolves them from its local descriptor.
  - Remote players' and enemies' weapon fire/impact arrive as reliable ordered observer cues with frozen sound keys, aliases, shot identity, and captured anchors. The firing owner is excluded; it already predicts these cues. Receivers resolve local sound assets and deliver each built-in and alias at the captured emitter. Enemy attack sounds may accompany the weapon cue. Other remote movement, behavior-entry, and mover sounds remain host-only.

---

## 5. Spatial Positioning

**One spatial pipeline.** Every positional play goes through one chokepoint, `crates/audio/src/spatial.rs`. It owns every positional voice — its spatial track, its sound, its anchor and its last position — and is the only code that creates kira spatial tracks or updates their parameters. Later features extend it in place: front/back filtering, distance-based stereo spread, occlusion. No second pipeline, no per-hardware renderer tier.

| Parameter | Behavior |
|-----------|----------|
| Placement | Each positional sound plays alone on its own kira spatial track under the SFX bus, so the SFX volume control governs it. The track persists until its sound finishes, so reclaiming a voice never cuts a tail. |
| Start | A positional voice starts at the audio step after admission, against that frame's listener. An entity anchor starts at its presented position, or frozen at its fire-time point if the entity is already gone. A contact set resolves to the finite contact nearest that listener; an empty set is an ordinary miss and plays nothing. |
| Distance attenuation | Falls off between a minimum and maximum distance along a curve, set mod-wide in the manifest's `audio.attenuation` block (`minDistance`, `maxDistance`, `curve`). Engine-seeded default: 2 m, 60 m, `linear`. kira interpolates level in decibels: `linear` is a straight ramp; `quadratic` holds level near the minimum, then falls faster toward the maximum. A malformed field — non-numeric, negative or non-finite distance, unknown curve — or a minimum not below the maximum warns naming the field, and the whole default applies, never a mix of authored and seeded halves. The mod still loads. The profile applies at mod init and on each committed manifest reload. A voice captures attenuation at admission; live voices keep theirs. |
| Stereo panning | Left/right balance from the sound's direction relative to listener facing; the far ear keeps a fraction of the signal. Distance gain plus two-ear panning; no Doppler, no HRTF. |
| Position tracking | An entity anchor follows the entity's render-interpolated pose each frame. Once the entity is gone, or its pose goes non-finite, the sound freezes at its last finite position and plays out; a tail is never cut. |
| Own pawn | A sound anchored on the pawn the listener is attached to plays unpositioned on SFX, at full level, avoiding panning at near-zero distance. The treatment is fixed at admission for the voice's life: a listener pawn change never re-treats a live sound. Every other pawn's sounds spatialize, including a remote co-op player's fire heard on the host. |
| Non-finite guard | No non-finite position reaches kira. The listener keeps its last finite pose, a contact set skips non-finite contacts, and an anchor with no finite point is dropped with a warning and its slot released. |

**Level lifetime.** Unload, restart and return-to-frontend all pass through level unload, which fades every world-anchored sound — positional and own-pawn alike — over 150 ms and releases its slot, before the level's sounds are released. No sound outlives its world or follows an entity key into the next level. Unanchored sounds (music, UI) are untouched.

### Captions and direction cues (decided, not yet built)

Audio information is made visible for players who cannot hear it.

- **Authoring.** Captions are keyed per sound asset, so every play path — `playSound` and descriptor sounds — captions without reshaping. A scripted subtitle primitive carries a speaker.
- **Display.** Captions and cues draw in an engine-owned UI layer above the mod HUD, resolving theme tokens and the selected variant (`ui.md` §2), sized by text scale. A caption holds at least a minimum time after its sound starts; repeat plays within the hold refresh one entry; enabling captions mid-sound captions the rest of that sound. Caption background opacity is a player option.
- **Direction.** A positional sound's caption carries a direction arrow computed at the spatial chokepoint and updated as the listener turns. Sound-direction cues mark off-screen positional sounds on their side and hold at least as long as a caption. 2D, UI, and music sounds get neither.
- **Client-local.** Captions derive client-side from sounds the client plays locally. Caption data stays off the wire; observer weapon cues already carry sound keys.

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
