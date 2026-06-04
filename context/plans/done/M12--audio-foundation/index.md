# Audio Foundation (M12, goal 1)

## Goal

Stand up the audio subsystem as a self-contained module that owns kira: an `AudioManager`, a mixer bus tree, sound-asset loading, playback handles, and a primitive-typed subsystem boundary. This is the foundation the three later M12 goals (spatial audio, reverb zones, sound-event playback) build behind. It establishes the contract and the wiring; it does not route entity sound events or spatialize anything yet.

## Scope

### In scope
- `src/audio/` module owning a kira `AudioManager` (`DefaultBackend` / cpal) with explicit `init` and `shutdown` lifecycle.
- Mixer bus tree: Master → SFX, Music, UI sub-tracks, each with a runtime volume control.
- Sound-asset loading: static (decode-in-memory) for SFX, streaming for music, from `content/<mod>/sounds/`. Ogg + wav via kira's Symphonia decoders. (streaming load path is built and unit-tested; audible streamed playback is verified only if the optional ambient-track smoke path is chosen — see Open questions)
- An in-memory sound registry keyed by content-relative name; populated at level load, released at level unload.
- A public play API returning handles for stoppable/looping sounds; per-bus active-voice cap (voice-budget stub) that respects kira's pre-allocated `Capacities`.
- Primitive-typed subsystem boundary: `ListenerState` and `SoundRequest` carry only primitive types (`[f32; 3]`, etc.) — no glam, no wgpu. One kira listener, created at init, updated each frame from the camera (glam → primitives converted at the boundary).
- The per-frame Audio step inserted in frame order (Input → Game logic → **Audio** → Render → Present).
- Headless tests via kira's `MockBackend`.

### Out of scope
- Spatialization: 3D position, distance attenuation, stereo panning, Doppler. (M12 goal 2.)
- Reverb zones, `env_reverb_zone`, BSP-leaf reverb lookup, occlusion. (M12 goal 3.)
- Routing entity-emitted sound events (`WeaponFireEvents`, movement events, footsteps) to playback. (M12 goal 4.)
- Texture-prefix → material → sound mapping. The `Material` enum already exists (`crates/postretro/src/material.rs`); wiring it to sounds is goal 4.
- A per-level sound manifest (which sounds a level preloads). Foundation provides the load/registry mechanism; content-driven population is goal 4.
- Voice stealing by priority/loudness. Foundation drops the newest request when a bus is at cap.
- Dynamic music / adaptive layers, dialogue/Voice bus, ducking. (See `audio.md` §7 non-goals.)

## Acceptance criteria
- [ ] Engine boots with audio initialized. If kira init fails, the engine logs `[Audio]` error and continues silently — no crash, no panic.
- [ ] The subsystem exposes a Master → SFX/Music/UI bus tree. Lowering a bus's volume measurably reduces the output amplitude of sounds routed to it (verified via `MockBackend` output inspection).
- [ ] A sound file (ogg or wav) under `content/<mod>/sounds/` loads and plays. A missing or undecodable file logs an `[Audio]` warning and produces no sound — no panic.
- [ ] Playing a sound returns a handle. The handle stops the sound. A sound played as looping repeats until stopped.
- [ ] When a bus reaches its configured active-voice cap, further play requests on that bus are dropped and logged; active voices never exceed the kira `Capacities` configured at init.
- [ ] The Audio step runs each frame between Game logic and Render, updates the kira listener from the camera, and never blocks the frame.
- [ ] On level unload the sound registry is released; a subsequent level load repopulates it.
- [ ] The `audio` module references no wgpu or renderer types, and no glam type appears in its public API — `ListenerState` and `SoundRequest` use primitives only.
- [ ] Headless `MockBackend` tests cover: init, bus creation, load from fixture, play, volume change, voice-cap drop, stop, shutdown — all without an audio device.

## Tasks

### Task 1: Subsystem skeleton, lifecycle, and boundary types
Create `src/audio/mod.rs` and register `mod audio;` in `main.rs` (alphabetically first in the module group, before `mod camera;` ~line 4). Define the `Audio` struct owning the kira `AudioManager`, an `AudioError` (thiserror) for init/load failures, and the boundary types `ListenerState` (position + forward/up as `[f32; 3]`) (up is the world up `[0.0, 1.0, 0.0]` — `Camera` has no up accessor; it uses `Vec3::Y` internally) and `SoundRequest` (bus + sound key + looping flag; primitives only). `Audio::new()` builds the manager with explicit `Capacities`; `shutdown` drops it cleanly. Add `audio: Option<Audio>` to `App` (`main.rs:227`); construct it in `resumed()` after the renderer is ready (`main.rs` ~line 447). On init failure, leave the field `None` and log — the game runs silent. Create one kira listener at init as the anchor for later spatial work.

### Task 2: Bus tree and voice budget
Build Master → SFX/Music/UI as kira sub-tracks under the main track via `TrackBuilder`/`add_sub_track`. kira's main track serves as Master directly — no separate Master sub-track; SFX/Music/UI hang off it. Store the sub-track handles on `Audio`. Expose per-bus volume control (`set_bus_volume(BusId, f32)`). `BusId` enumerates `Sfx`, `Music`, `UI`; Master volume is set on the main track directly (not a `BusId` variant). Implement a per-bus active-voice counter with a configured cap; `play` consults it and drops (logs) when a bus is full. Cap totals must stay within the `Capacities` from Task 1. Invariant: the sum of per-bus voice caps must not exceed kira's global sound capacity, so a play command never silently drops at the kira layer.

### Task 3: Asset loading and registry
Resolve sound paths under `content/<mod>/sounds/<collection>/<name>.{ogg,wav}` from `App::content_root` — mirror the path-resolution and graceful-degradation pattern of texture loading (`render/loaded_texture.rs`: missing/corrupt → `warn!`, skip, never panic) (the `[Audio]` log prefix is a new convention for this module — the texture path logs untagged; mirror the degradation behavior, not the tag). Load SFX as static decode-in-memory and music as streaming. Hold loaded sounds in a registry keyed by content-relative name. Add a load hook at level install (`App::install_level_payload`, after texture upload ~`main.rs:1526`) and a release hook at level unload so the registry follows level lifetime, like textures (`resource_management.md` §7.2).

### Task 4: Play API and per-frame Audio step
Implement `play(SoundRequest) -> Option<SoundHandle>` (routes to the named bus, applies looping, registers the handle for stop), `stop(handle)`, and an active-voice query. Insert the Audio step in the frame loop after the event drain and before render (`main.rs` ~line 847): convert camera state to `ListenerState` — `self.camera.position` (field), forward from `self.camera.aim_ray().1` (the direction half of the `(origin, direction)` tuple; includes pitch, unlike yaw-only `forward()`), right from `self.camera.right()` at the boundary and call `audio.update(listener, frame_dt)`, which updates the kira listener and per-frame bookkeeping. The step must be cheap and non-blocking.

### Task 5: Headless tests and smoke check
Add `MockBackend`-backed tests covering the AC behaviors (init, buses, load fixture, play, volume → output amplitude, voice-cap drop, stop, shutdown). Confirm the `MockBackend` sample-readout API (the method that pulls processed output frames) at implementation time — it is the load-bearing amplitude-verification mechanism. Add a small ogg/wav test fixture under `content/dev/sounds/`. (no `sounds/` directory exists yet — create it; the existing parallel is `content/dev/textures/`) Provide one real-device smoke path: a debug-UI trigger that plays a test SFX on the SFX bus, confirming output reaches the OS.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines `Audio`, `Capacities`, and boundary types; blocks everything.
**Phase 2 (concurrent):** Task 2, Task 3 — buses and asset loading are independent, both build on Task 1.
**Phase 3 (sequential):** Task 4 — play API and frame step consume the bus handles (Task 2) and registry (Task 3).
**Phase 4 (sequential):** Task 5 — tests and smoke check exercise the full surface.

## Rough sketch

**Module layout** (`development_guide.md` §2.4 — `mod.rs` barrel, internals `pub(crate)`):
- `src/audio/mod.rs` — `Audio`, `AudioError`, `BusId`, `ListenerState`, `SoundRequest`, public API.
- `src/audio/buses.rs` — bus-tree construction, volume, voice budget.
- `src/audio/assets.rs` — path resolution, static/streaming load, registry.

**kira surface** (0.12; confirm exact names against docs at implementation time): `AudioManager::new(AudioManagerSettings { capacities: Capacities { .. }, .. })` with `DefaultBackend`; `manager.main_track().add_sub_track(TrackBuilder::new())` for buses; `StaticSoundData::from_file` / `StreamingSoundData::from_file` for assets; `track.play(sound)` → handle; `handle.stop(..)`; `manager.add_listener(..)` for the listener anchor. Tests swap `DefaultBackend` for `MockBackend` and step `process()` to inspect output.

**kira owns the hard parts.** kira runs its own real-time audio thread and lock-free command queues internally. Do not build an audio thread, ring buffer, or mixer. The per-frame Audio step is control-plane only: convert listener state, issue play/stop/volume commands, update bookkeeping. The audio-thread real-time rules (no alloc/lock/block) are kira's concern; our job is to respect the pre-allocated `Capacities` so commands never silently drop and handles are not leaked.

**Subsystem boundary contract** (durable; align the existing `audio.md` boundary sections at promotion — the doc already documents this contract; its BSP-leaf/reverb row stays future-tense until goal 3): audio receives `ListenerState` and `SoundRequest` in primitive types and produces audio via kira internally. No glam crosses the public API; conversion from `Camera` happens at the call site in the frame loop. No wgpu/renderer types anywhere in the module. This mirrors "Renderer owns GPU" (`index.md` §2). The boundary is shaped so goal 2 adds spatial track properties + listener-relative positioning and goal 4 adds `SoundRequest` draining from the entity event stream — neither changes the boundary's primitive-only shape.

**Content layout:** `content/<mod>/sounds/<collection>/<name>.{ogg,wav}` — parallels the texture convention (`resource_management.md` §1.1), `_`-prefixed shared collection per `boot_sequence.md`. Sounds are not embedded in PRL and are released on level unload (no gameplay streaming-from-disk beyond kira's own music streaming).

## Open questions
- **kira feature flags.** `Cargo.toml:36` declares `kira = "0.12"` with default features. Confirm defaults include the cpal backend and Symphonia ogg/wav decoders; enable the `mock` backend feature (likely dev-only) for headless tests. Resolve during Task 1. Add the non-default `mock` backend feature on the consuming crate (`crates/postretro/Cargo.toml`) as a dev-only feature, not on the workspace root declaration.
- **Decode location.** Foundation decodes sounds on the main thread at `install_level_payload` (level load is already a loading/boot state). If this causes a hitch, move decode to the existing level worker thread (`startup/worker.rs`) — `StaticSoundData` is `Send`. Deferred unless measured.
- **Audible smoke proof.** Task 5 uses a debug-UI trigger. Alternative: start an optional per-level ambient track on the Music bus when present (absent = silence, not error), which also exercises the streaming path. Pick one during Task 5; debug trigger is the lower-scope default.
- **Capacities sizing.** Pick conservative initial sound/track/command capacities (e.g., sounds 128, tracks 16) and the per-bus voice cap; tune when goal 4 lands real content. Not load-bearing for the foundation.
