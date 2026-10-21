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

## Layer placement and the fluid precedent
- `scripting.md` §1: "Engine owns the floor; scripts own the taste". The engine seeds a default; a game picks its point through data and script.
- Fluids are the precedent for named, mod-wide presentation that a map volume references (`scripting.md` §Fluid declarations):
  - `defineFluid` is a pure identity helper, and declaration order assigns the index.
  - Warn-and-drop: a malformed entry, or a later duplicate name, drops with a warning naming `fluids[i].<field>`, and the first declaration stands.
  - Staged reload re-resolves the level's volumes.
  - Fluids stay out of the mod compatibility digest. Presentation fields resolve locally by name, and peers may see different opacity and murk (`networking.md` §What gates, and what replicates instead).
- Mod-wide profiles with whole-default fallback: `audio.attenuation`, `render` (`scripting.md` §Mod render profile).

## Impact owner
- `WeaponEmission { shot_id, emitter, … }` (crates/entities/src/reactions/emitter.rs). `Emitter::Contacts` carries only point, normal and hit. The owner is `shot_id.pawn`, stamped from the owner pawn's `NetworkId` (`shot_id.pawn = allocator.stamp(owner_pawn).0`). An emission without a shot id has no owner.

## Audio capacity
- Bus voice caps (`BusId::voice_cap`, crates/audio/src/buses.rs): SFX 32, Music 4, UI 8. The manager budget has room for a 16-voice ghost cap without raising its capacity (`caps_within_capacity`).

## Client-side facts that bound membership and audio
- A connected client drops every host-replicated map placement before dispatch (`filter_out_client_host_replicated_placements`, crates/postretro/src/startup/lifecycle_world_cpu.rs). It rebuilds them from snapshots as class-only display copies with no `_tags` or map ordinal (crates/netcode/src/remote_materialize.rs, crates/sim/src/scripting/builtins/net_descriptor.rs). `ComponentPayload` carries no map identity (crates/net/src/wire/components.rs). Movers spawn locally from PRL with tags on every peer (crates/postretro/src/runtime_movers.rs).
- The host's own shots carry `shot_id.pawn = 0` in the raw emission (crates/sim/src/sim/mod.rs, crates/sim/src/sim/weapon_stage/machine.rs). Only `publish_observer_weapon_cues` normalizes it to a `NetworkId` (crates/postretro/src/weapon_observers.rs). `NetworkId(0)` is a real allocation (crates/netcode/src/lib.rs). Client-predicted shots and observed cues already use `NetworkId`s.
- Mover sounds and a pawn's own reload, dry fire and overheat play on the host only. Clients receive only fire and impact observer cues (`audio.md` §4).
- A sequence step must name a target. The JS step parser requires an `id`, so a mod-wide primitive is a whole reaction body (`setSentiment`, content/dev/scripts/faction-sentiment.ts).
- The cell-visibility relation (id 46) stores per-cell reachability component ids plus graded pair records, with no per-pair open state to mask. The research doc's dynamic portal is a sightline blocker mask for destructibles, not built.
- `ChannelClamp` (crates/render-cpu/src/flash_clamp.rs) is hard-wired to `screen.flash`, `screen.vignette` and the fluid tint, and its per-channel tracked value is private. A state turned on "starts fresh against its own values".
- Nesting the Ghost track under SFX takes one of SFX's child-track slots, which are sized to the SFX voice headroom. `caps_within_capacity` with a 16 ghost cap totals 60 of 64.
- Fluid colors are `[r, g, b]` arrays (`viewTint`, `fogColor` in content/dev/start-script.ts).

## Pin table
| id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Pawn teleported out of link A's volume | seamless cut frame, then A's ramp-out | A's ghost fades at its last in-volume mapped pose; never drawn at the mapping of the new pose |
| P2 | `arm` and `disarm` in one tick | both apply at one host tick, in call order | Last call wins; no frame on any peer presents a ghost; no ramp starts |
| P3 | `arm` tick N, `disarm` tick N+1 | target membership changes twice, the ramp follows | Presented membership rises for one tick at the rate cap, then falls to 0 at the rate cap |
| P4 | `arm` during a ramp-out; member re-enters mid-fade | reversal mid-ramp | Ramps from the presented value, never resets |
| P5 | `setColorFilter` naming only opacity, mid tint ramp | second set before the first ramp ends | Tint ramp continues to its target; opacity ramps on its own |
| P6 | `setColorFilter` mid-ramp | from-value captured at the applying host tick | Unclamped ramp value at that tick's host script time; host limiter state never on the wire |
| P7 | Two `setColorFilter` calls in one tick | applied in call order within one tick | Later call wins per field; its ramp starts from the value before that tick |
| P8 | Snapshot repeats an identical link record | always-on record every snapshot | No re-rebase; the presented value does not move as the RTT estimate changes |
| P9 | Joiner before its first link record | install, then the first record lands | Presents local spawn state and style; a disagreeing armed state moves membership at the rate cap, never cuts |
| P10 | Link record before the presentation clock is ready | record lands, then clock anchor | Armed state applies; look ramp deferred, then rebased |
| P11 | Record names a link not installed | record before or without a matching install | Binds nothing, warns, not acked; a later baseline binds |
| P12 | Joiner mid-`setColorFilter` or mid-`setGhostAudio` ramp | join after ramp start, before end | Presents the rebased current value; ends with the host |
| P13 | Level unload or demotion after `setGhostAudio` | record dropped, then next level | Ghost bus returns to the manifest profile on every peer, host included; ghost voices released |
| P14 | Host level change with links in both levels | old-level record arrives after new install | Matched by index and source entity; a stale record never binds a new link |
| P15 | Opacity-only `setColorFilter`; peers with differing styles; then staged reload | setColorFilter, replication, reload | Only opacity crosses the wire; tint and saturation stay local and follow the reload |
| P16 | Staged reload mid `setColorFilter` ramp; reload on a disarmed link | reload commit during a ramp or while disarmed | Script-set ramps continue to their targets; disarmed links re-resolve too |
| P17 | Staged reload changing `audio.ghost` | reload after or without `setGhostAudio` | Bus re-resolves; `setGhostAudio` fields stand |
| P18 | Link arm and a remote pawn's seamless swap on one host tick | each ghost reads link state at its member's presentation time, on client and host seat | Both present on one frame on every peer |
| P19 | Entity slot freed and re-tenanted on one frame | despawn, then spawn, same frame | Ramp and limiter state key by generation-checked id; the new tenant starts at 0 |
| P20 | Member fires a projectile; the projectile despawns | spawn frame, flight, despawn frame | Ghost ramps in from spawn at the rate cap; fades out at its last pose on despawn |
| P21 | A new ghost's first presented frame | membership ramp creation, then first pack | Membership starts at 0, so the limiter's fresh start sees 0 |
| P22 | Owner leaves the volume while its projectile flies | owner membership changes before impact | Projectile ghost ramps out with the owner; the impact is not ghosted |
| P23 | Ghost voice playing when the link disarms, the member leaves or despawns | voice admitted, then membership ends | Voice plays out at its fire-time mapped point |
| P24 | Sound at a member near the boundary or mid-ramp | drain-time admission | Admitted when armed and the fire-time point is inside; the presented ramp is ignored |
| P25 | Viewer's own pawn makes a sound | local drain | No ghost voice on the viewer's machine |
| P26 | Level first frame; headless capture | install, then first frame or capture at time zero | Members of links armed on spawn present at full membership |
| P27 | Tagged mover inside two armed links that both list its tag | two memberships, one mover id | Two ghost transforms for one mover |
| P28 | `getMapEntities("ghostLink", { tag })` with zero tagged links | query in `setupLevel` | Returns `[]` silently; the fixture asserts both dev-map links return |
| P29 | `setGhostAudio` lowering `lowPassHz` while a `highPassHz` ramp is in flight | validation at the applying host tick | Judged against targets after the step; high at or above low warns and drops |
| P30 | Limiter off; a hitch frame | the ramp is independent of the limiter | Membership ramp still applies unclamped; a hitch steps at most the hitch ceiling |
| P31 | Limiter turned on, or a new renderer's first frame, while ghosts present | enable frame, then pack | Each ghost starts fresh against its current value; no dip |
| P32 | Level first frame with the limiter on, link armed on spawn | install at full membership, then the limiter's first pack | Full membership on frame 1, presented through the limiter's fresh start; a headless capture, outside the limiter, shows full style opacity |
| P33 | Link arm and a member mover's phase change on one host tick, on a connected client | movers present at the estimated host tick | The mover's ghost and its motion start on one client frame |
| P34 | Observer fire cue on a client for a shot on the host tick after an arm or disarm | cue played on receipt | Admission reads armed state as the shooter's ghost presents it at that frame |
| P35 | `arm` on tick N, `disarm` on tick N+1, on a connected client | snapshots may skip tick N | At most one rise and fall, each at the rate cap; none when no snapshot carries tick N |
| P36 | Resolved cutoffs cross after a staged reload under a script-set cutoff, or against another peer's profile | reload or record apply, then resolve | Floor cutoffs present; one warning; never a high-pass at or above the low-pass |
| P37 | Host restarts the same map; an old-level link record arrives after the new install | participation reset, then re-promotion, then the stale record | Rejected by the participation reset; the new link presents spawn state until its first record |
| P38 | Client demoted mid-level while received state differs from local spawn state | received state dropped, then the next frame | Local spawn state and style; membership moves at the rate cap |
| P39 | Mover carrying two tags one link lists | one link, two tag matches | One membership, one ghost transform |
| P40 | SFX at its cap when a member's sound is requested | real voice refused, then ghost admission | No ghost voice |
| P41 | `setColorFilter` or `setGhostAudio` naming no field | applying host tick | Warns and drops; no ramp in flight changes |
| P42 | Game paused mid membership ramp and mid `setColorFilter` ramp | pause while frames keep presenting | `setColorFilter` ramps freeze with script time; the membership ramp runs on presented-frame time |
