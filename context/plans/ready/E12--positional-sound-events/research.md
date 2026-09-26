# E12--positional-sound-events — research

Read at b3548b603. Findings that inform but do not decide. Symbols only; re-verify before relying.

## Basis: no typed sound events exist

- `TickEvents` (sim/mod.rs) holds `movement: Vec<&'static str>`, `ai: Vec<Cow<str>>`, `weapon: Vec<&'static str>`, `mover: Vec<(MoverEventKind, u32)>` and `death: Vec<String>`.
- All of them drain through `drain_named_events_with_sequences` (main.rs) with no dispatch context. The source string is `"named:<event>"` and it carries no values.
- `WeaponFireEvents::event_names()` flattens `activate`, `impacts`, `spawned` and `dry_fire` into names.
- `ENEMY_ATTACK_EVENT` and activity `on_enter` names are pushed by `ai::apply::apply_outcomes`.
- Movement's `append_named_events` emits `landed`, `jumped`, `dash_started`, `crouch_started` and `slide_started`.
- Mover edges resolve through `MoverEventKind::dispatch_address` to the authored `*_event` strings.
- `Audio::play` has only two callers: the `SystemReactionCommand::PlaySound` arm of `App::dispatch_system_commands`, and `DiagnosticAction::PlayTestSfx`.
- The only content example is `lowHealthAlert` in `content/dev/scripts/arena-lights.ts`.
- `mover_sound_event_drain_maps_authored_name_and_executes_play_sound` proves the path up to the queue, not up to `Audio::play`.

## Anchor sources

| Event | Identity in scope at push | Notes |
|---|---|---|
| weapon `activate` / `dry_fire` (local) | `pawn`, `weapon_id`, `WeaponActivation { origin, direction }` | `run_local_weapon_command_with_content` |
| weapon `activate` / `dry_fire` (remote, host) | `remote.pawn`, `weapon` | `run_remote_weapon_commands` |
| weapon impact | `WeaponImpact { point, normal, target }` per impact | several per tick for pellets |
| projectile contact | `ProjectileContactEvent { projectile, point }` | projectile despawned in-tick; no script `impact` today, only `local_projectile_contacts` |
| reload | `ReloadDelivery { pawn, weapon, outcome }` | fields `pub(crate)`; `ReloadOutcome::event_name`: `reload_started`, `reload_completed`, `reload_shell_loaded`, `reload_cancelled`, `reload_blocked_*` |
| AI attack / `on_enter` | `outcome.id` (enemy), `target.entity` | `apply_outcomes` |
| movement `landed` / `jumped` | local pawn only | `run_movement_tick` keeps `local_player_movement_pawn()`; client: `client_predict_movement_tick` |
| mover edges | `mover_id: u32` → `EntityId` via `registry.iter_with_kind(KinematicMover)` | `mover_event_dispatch_addresses` discards the id |

The brief decides that a projectile contact plays `sounds.impact` at its contact point. Today the contact produces no script event, and the owner decided it now fires `impact` reactions too (pin P14).

## Positions and lifetime

- `EntityRegistry::interpolated_transform(id, alpha)` gives the pose render shows. `frame_result.alpha` is in scope before `audio.update`.
- On clients, remote poses are snapshot-interpolated before audio (`client_sample_interpolation` → `set_presentation_transform`). On the host, remote client pawns are presented the same way (`host_present_client_pawns`).
- `EntityId` is a 16-bit index plus a 16-bit generation, and stale ids are detected.
- Enemies are removed in `run_end_of_frame_removal_pass`, which runs after the named-event drain and before `audio.update`.
- Mover transforms are origin-relative. `RuntimeMovers` keeps local bounds per `mover_id`, and the world bounds come from bounds × interpolated transform.
- The listener comes from `self.camera.position` plus `aim_ray()` (raw). Render uses `render_eye_position`, built from `frame_timing.interpolated_state()` plus the view-feel offsets. It is computed after audio today.

## Scripting scope mechanics

- `IrType` / `IrValue` are {Number, Bool} only (§11 "two value types").
- Token precedent: SDK sentinels `activators` / `trigger` on the frozen `DISPATCH_PARAMS` object. The builders emit `target: "@activators"` or a sequence `id: "@trigger"`, which parse to `SequenceTarget`, then bind to `BoundTarget`, then are resolved from `TriggerFireContext`.
- The binder refuses a sentinel on a non-consequential primitive (`partition_direct_reaction`). The named dispatch path skips a primitive carrying `target` (`reaction_dispatch`).
- The app-drain side channel is `SystemCommandFireContext { source, values }` on `SystemCommandQueue`, stamped by `fire_named_event_with_sequences`. Only `setState` reads it, via `SystemReactionIrBindings` against `APP_DRAIN_DISPATCH_INPUTS` (`@rising`).
- Install checks in `startup/reaction_validation.rs`: V4a (sentinel after `wait`) and V4b (`scoped_fire_targets`).
- `playSound` today: `PlaySoundArgs { sound, bus }`; TS `playSound(sound, bus?)` in `sdk/lib/ui/reactions.ts`; Luau mirror in `sdk/types/postretro.d.luau`.

## kira

These notes are from the 0.12.0 source. 0.12.4 has not changed any of these APIs.

- **Spatial tracks.** `TrackHandle::add_spatial_sub_track(listener, position, SpatialTrackBuilder)`. Position belongs to the track, not the sound, so each positional sound gets its own track. Positions are mint vectors (plain arrays are fine).
- **Builder defaults.** `distances` defaults to (1, 100). `attenuation_function` defaults to `Some(Easing::Linear)`. `spatialization_strength` defaults to 0.75: mono fold plus two-ear dot-product gain, which is panning only. `sound_capacity` and `sub_track_capacity` both default to 128 per track, so set them small per positional track.
- **Movement.** `SpatialTrackHandle::set_position(pos, Tween)` with a short tween; kira interpolates within each audio chunk.
- **Lifetime.** Dropping the handle removes the track immediately and cuts the tail, unless the builder sets `persist_until_sounds_finish(true)`.
- **Capacity.** A spatial track created under the SFX track counts against SFX's `sub_track_capacity` (today 128, the `build_bus` default), not against the manager's `Capacities.sub_track_capacity` (16). The SFX `sound_capacity` of 32 no longer bounds sounds placed on child tracks.
- **Tail safety.** A spatial track whose handle is dropped is removed on the next block even while its sound plays (`track/sub.rs`). Build positional tracks with `persist_until_sounds_finish(true)`, or keep the handle until the sound reports `Stopped`.
- **Live occupancy.** `num_sounds()` / `num_sub_tracks()` count reserved arena slots from `try_reserve` until the audio thread removes the item, so they are the live occupancy the admission check reads.
- **For the reverb step.** `ReverbBuilder` and `FilterBuilder` handles expose tweenable parameters. Sends tap a child track's output before the parent's fader, so a reverb send must be coupled to the SFX volume explicitly.

## Dependency currency (step 0)

- **Locked:** kira 0.12.0, cpal 0.17.3, symphonia 0.5.5, glam 0.32.1 (a single copy).
- **Latest:** kira 0.12.4 (2026-08-27). It has no API breaks. It pulls cpal 0.18 (macOS and Windows backend fixes) and symphonia 0.6, and fixes a streaming seek-while-paused bug.
- **Cost:** kira 0.12.1 and later require glam ^0.33, which duplicates glam against the workspace's 0.32.
- **Change:** lockfile only (`cargo update -p kira`). Owner sequenced it as a separate chore.

## Networking seams (peer-audio step inputs)

- Sound is `PrimitiveClass::Presentation` (sim/trigger_bindings.rs), host-local on the app drain (roadmap E18).
- `ServerPresentationPayload` is append-only (net/wire.rs) and travels on the unreliable `Channel::Presentation`.
- `recipients_for_world_point` (netcode/presentation.rs) exists and currently broadcasts to every participant except the owner.
- `WireKinematicMoverState` has `direction`, `segment_index`, `completed` and `blocked`, and no terminus field. A client-derived door edge must infer terminus from those. There is no crush edge on the wire.
- E17 AC11: a connected client emits no mover sound. Step 1 keeps that.

## Ordering pins

Owner-resolved rows are marked (owner).

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Every SFX voice finishes; a full cap of new positional requests arrives | reclaim at audio step N → requests in frame N+1 → mixer has not yet removed finished sounds/tracks (removal happens on the audio thread one to two chunks later) | Every request the voice counter accepts plays; none is refused by the mixer |
| P2 | Level unload, restart or return-to-frontend with positional voices live | voice started → unload clears registry (generation kept, anchor goes stale) → next level or frontend listener | Every positional voice fades out at unload; none plays under the next level (owner: stop with fade) |
| P3 | Staged manifest commit (hot reload) changes a descriptor `sounds` key, or adds an unknown key | install → sound table and key check built → commit rebuilds reactions and reruns V4a/V4b | Commit rebuilds the sound table and reruns the key check (owner) |
| P4 | Multi-pellet shot picks its nearest contact | named-event drain (game logic) → listener pose built at audio step → eye built after audio today | Request carries every contact; nearest is resolved at the audio step against that frame's listener (owner) |
| P5 | Connected client fires dry, reloads, or lands pellets | client emits only predicted `activate` and movement events | Client predicts dry fire (ammo-gated) and world impacts; reload edges derive from replicated reload/ammo slots (owner) |
| P6 | Enemy attacks and is removed on the same frame | drain → play at fire-time point → end-of-frame removal → first audio reposition finds no entity | Sound plays out at its fire-time point |
| P7 | Reaction reading `on.emitter` fired by a death event, a trigger, or another reaction's completion | death events drain one frame after removal; follow-ups dispatch contextless | Skipped with one warning; nothing plays, no dry fallback |
| P8 | A voice finishes early in a frame at the cap; a new request arrives that frame | voice stops → request dispatched → reclaim at audio step | Request refused; slot frees at that frame's audio step |
| P9 | Manifest attenuation changes by staged commit while sounds play | voice started with old distances → commit | New voices use the new attenuation; live voices keep theirs |
| P10 | Eye computation moves ahead of audio | view-feel evaluate (advances integrator by frame dt) → listener → render | View feel advances once per frame; listener and render read one eye |
| P11 | Crush mover pins two actors on one tick | one crushed edge per victim per interval | Two crush sounds |
| P12 | Listener changes pawn (respawn, spectate) while a sound plays | sound started spatial or non-spatial → listener pawn changes | Sound keeps the treatment it started with |
| P13 | Client's predicted shot is rejected by host | fire sound plays at prediction → reject arrives → muzzle FX rolled back | Fire sound is neither stopped nor replayed |
| P14 | Projectile contact on a weapon with an `impact` reaction | contact in tick → projectile despawned in tick | Projectile contact fires `impact` reactions and the descriptor sound once per activation per tick (owner) |

## Client prediction and replication (step 1 client decision)

- Client fire prediction has no magazine gate today (`weapon/mod.rs` predicted fire; main.rs client fire path), so an empty gun predicts a fire.
- Hitscan prediction keeps only entity hits, as `LocalHitRecord { target, point, zone }` with no normal. World hits are dropped.
- Reload is not replicated as wieldable state (`wire.rs`). The owner-private `player.reloadActive` and `player.ammo` slots are. Complete and cancel look alike on the flag alone, so they are told apart by whether ammo rose.
- Enemy weapon attacks are projectile-only (`brain_programs.rs` disables hitscan weapons). They spawn via `host.spawn_projectile` with `owner_weapon` set to the enemy, and the weapon name is known only at spawn (`descriptor_class`).
- `unload_level` (`startup/lifecycle_net.rs`) is the single unload / restart / frontend hook. The staged-manifest commit block (`staged_manifest_lifecycle.rs`) is where the V4a/V4b rerun happens.
