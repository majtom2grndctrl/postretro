# ghost-entities — research

Read at d6f3471. These findings inform the brief but do not decide anything; the brief's Decisions carry the conclusions. Cited by symbol.

## Why room copies
- No interaction filtering exists. Entity-entity collision is plain overlap. Hitscan, radial overlap and splash check only alive, damageable and caller exclusions (`crates/sim/src/sim/splash.rs` honors only owner `self_damage`). Faction (`FactionRegistry`, `FactionSentimentState`) gates only AI target acquisition. A shared-space timeline would add a filter to each of those paths, plus triggers, AI perception, render and audio.
- The teleporter docs already describe the time shift as room copies (`docs/teleporters.md` "Building room copies for a seamless shift").
- Arming an occupied seamless teleporter fires every occupant on the next tick (`teleport_two_pawn_entry_and_arm_moves_both_and_fires_the_event_once_each`). Scripts can teleport players only through `arm`/`disarm`.

## Replication
- `host_replicate` (crates/netcode/src/host.rs) encodes one snapshot per participating client from the full `ReplicableSet`. The only per-client exclusion is `exclude_entity_for_client`, used solely to hide the owner's predicted projectile (`projectile_presentation.rs`). Client apply and interpolation apply every record without a distance check. Cell culling happens only at render collection.
- Bound presentation state: host capture in `presentation_capture.rs` (`REPLICATED_PRESENTATION_PRIMITIVES`). It rides snapshot records as `ComponentPayload::{LightState, FogState, EmitterState}` with `WirePresentationBinding { kind, index, origin }`. `PresentationBindingKind` has `MapLight`, `FogVolume` and `MapEntity`, and the decode-side `kind > 2` checks are hardcoded. It replicates only after a host-only write or a write on a networked entity.
- Ramps on that lane are animation curves (`FogAnimation`, `LightAnimation`, `play_count: Some(1)`) with `anim_start` stamped from `presentation_script_time`. The client rebases in `apply_presentation_payload` against `presentation_host_time` (snapshot server script time plus half RTT).
- Teleporter state: `ComponentPayload::TeleporterState(WireTeleporterState { index, source_entity, armed })`, produced in `collect_payloads`, bound in `crates/netcode/src/client/teleporters.rs`. It is always replicated, with a joiner gate.
- Host-only no-op: `enqueue_teleporter_mutations` and `apply_pending_teleporter_mutations` gate on `group_commands_apply_here` → `owner_slot_writes_enabled`. This is a role gate, not an origin gate.
- Current `WIRE_VERSION` 32 (`handshake.rs`) and `SNAPSHOT_VERSION` 21 (`wire.rs`).

## Rendering
- `MeshPass::new` and `KinematicBrushPass::new` use `blend: None`, write depth and back-face cull. Blending exists only for billboards (additive), fluid, splash and UI.
- Skinned `Instance` holds `model` plus `base_and_pad` (palette base, shadow bias). `KinematicMoverInstance` holds `{ mover_id, transform }`. Neither shader has tint, alpha or discard.
- Skinned culling: `MeshRenderCollector` → `mesh_visible` (`locate_cell` vs `VisibleCells`, cell only, no frustum). It draws at `interpolated_transform`. Movers: `mover_visible_against_cell_bounds`.
- Movers use their own pass with world materials and direct SH, not the world BVH and not lightmaps.
- Presentation-vs-sim precedents: `interpolated_transform`, `presentation_body_rotation`, pose modifiers, and `collect_viewmodel` (`is_viewmodel`).
- Remote aim: `PresentedPose.aim_pitch` (crates/netcode/src/interpolation.rs) drives the `aimSpine` pose modifier, and `active_weapon_archetype` picks the third-person weapon. The socket-mount draw under an extra transform was not traced.

## Observer fire presentation
- Hitscan has no engine muzzle flash or tracer for anyone. Observers get fire and impact sounds plus sparks from reliable observer cues (`freeze_observer_weapon_cue`, `materialize_observer_weapon_cue`, `spawn_observer_impact_bursts`).
- Projectiles are replicated entities (`COMPONENT_KIND_PROJECTILE`, `WireProjectilePresentation`).

## Audio
- `BusTree::build` / `build_bus` (crates/audio/src/buses.rs). SFX carries `WorldMuffleBuilder`, a custom Simper SVF low-pass in `muffle.rs`, driven through an atomic target with a per-sample ramp. Bus volume uses `TrackHandle::set_volume`.
- A sub-track can hang under SFX, and its output then passes SFX volume and muffle. SFX `sub_track_capacity` is 64 (2 × 32). The manager's sub-track capacity is 16, with 3 used.
- `spatial.rs::start_voice` parents every voice to the `&mut TrackHandle` it is passed, always SFX today (`audio_core.rs`). `has_room` checks the parent's sub-track count. `try_acquire_voice` / `release_voice` key by `BusId`. Reclaim, `stop_all` and failed-start release `BusId::Sfx` unconditionally. `AudioCore::play` drops anchored requests whose bus is not SFX.
- kira 0.12.5 is pinned. Checked in its source (`src/effect/`):
  - `FilterBuilder` with `FilterMode::{LowPass, BandPass, HighPass, Notch}`; `FilterHandle::set_mode`, `set_cutoff`, `set_resonance` and `set_mix`.
  - `DistortionBuilder` with `DistortionKind::{HardClip, SoftClip}`; `DistortionHandle::set_kind`, `set_drive` (`Decibels`) and `set_mix`.
  - `VolumeControlBuilder` / `VolumeControlHandle`.
  - Every `set_*` takes a value and a `Tween` (`handle_param_setters!`).
  - Compressor, delay, EQ filter, panning control and reverb also exist.
- The engine uses none of kira's built-in effects yet. World muffle and mono are custom `Effect`s (`muffle.rs`, `mono.rs`). The behind-the-listener low-pass is planned, not built: roadmap Epic 12 step 2 puts kira's `FilterBuilder` on each voice's spatial track. The Ghost bus would be the first user of kira's built-in filter.
- Manifest attenuation: `resolve_authored_attenuation` (crates/scripting-core/src/data_descriptors/audio_profile.rs), drained by `drain_audio_profile_lua` and `drain_audio_profile_js`. Any malformed field falls back to the whole default.
- Drain anchors: `AnchorScene::sound_anchor` maps `Emitter::Entity` → `SoundAnchor::Entity { key, point }` and `Emitter::Contacts` → `SoundAnchor::Contacts`. `SoundAnchor::Point` exists. Requests are built in `frozen_sound`, the mover and teleporter sound modules, and the `PlaySound` drain.
- No machine plays a remote pawn's movement sounds (`audio.md` §4).

## PRL and map identity
- Teleporters: `crates/level-format/src/teleporters.rs` (`TeleportersSection`, `MAX_TELEPORTERS`, `TELEPORTER_SOURCE_ENTITY_KEY`). Loader: `crates/level-loader/src/prl_teleporters.rs`. Compiler: `crates/level-compiler/src/parse/teleporters.rs` (`TeleporterParser::extract` / `finish`, errors "landmark must differ from target", "unknown target", "unknown landmark"). Registry: `section_registry.rs`. Id 56 is free, and id 14 is a retired hole.
- `MapOrdinal` is the index in the id-29 MapEntity list. Descriptor placements carry it (`DescriptorSpawnPath::MapPlacement`). Movers do not: they carry `mover_id`, the index among movers, resolved by `runtime_movers::mover_entity`.
- Brush volumes: `environment_volumes::resolve_leaf` is generic and `pub(super)`.
- Tags: `_tags` is a whitespace-delimited KVP on movers and placements (`sdk/TrenchBroom/postretro.fgd`), and `getMapEntities` queries it through `worldQuery`. Teleporter and environment-volume `source_entity` is the raw `.map` index, not the id-29 ordinal.
- FGD: `sdk/TrenchBroom/postretro.fgd` (`trigger_teleport`, `teleport_destination`, `kinematic_mover`).
- Script kinds: `sdk/lib/map_entities.ts` `MAP_KIND_COMPONENTS` → `worldQuery`. Rust side: `entity_world_primitives.rs` `QueryFilter::Teleporter`, `collect_identity_snapshots_json`, `crates/entities/src/ffi.rs`, and the typedef templates `sdk_lib.*`.
