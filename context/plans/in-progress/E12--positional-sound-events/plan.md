# E12--positional-sound-events — plan of record

mode: resumable
status: approved
read at: b99101534

## Owner resolutions
- **AC 15 block → B, extend the wire (owner, 2026-09-26).** "What sets PostRetro up to thrive long term," not parity. `wire::HitRecord` gains `normal: [f32; 3]`. Hitscan declarations also carry world contacts under the `u32::MAX` presentation-contact sentinel that projectile declarations already use (`PROJECTILE_PRESENTATION_CONTACT_TARGET`). The host validates a presentation-only hitscan contact with the same range and eye line-of-sight checks as an entity hit, allowing for the contact surface itself; it applies no damage. A non-finite or non-unit normal invalidates that record's contact data only, never damage validation. `WIRE_VERSION` 22 → 23. `apply_valid_hit_record` and the projectile-contact path build their `WeaponImpact` from the declared normal instead of `Vec3::ZERO`. The host emits one impact per remote activation carrying every validated contact. Row 15 stands as written. The brief's client Decision records the extension.

## Corrections
- Path: "`audio/mod.rs` (930 lines) splits before the anchor and voice work lands" → extract the audio subsystem into its own crate, `crates/audio` (package `postretro-audio`), and split `mod.rs` along the way (owner, 2026-09-26). The crate depends on no workspace crate; kira lives only there. `layering_invariants_hold` pins that no sim-side crate depends on it.
- Read-at commit `b3548b603` is not an ancestor of `main` → every cited symbol was re-read at `b99101534` (four read-only verification passes). Planning proceeds from those reads.
- `RuntimeMovers` keeps local bounds → no such type. The per-mover cache is `KinematicMoverRenderCollector.mover_bounds: HashMap<u32, Aabb>` (`runtime_movers.rs:143`), filled from `mover_local_bounds` (`:463`). The mover anchor computes `local_bounds.transformed(interpolated transform)` the same way `collect` does (`:171-230`).
- `WeaponFireEvents::event_names()` flattens `impacts` → it emits `dry_fire`, `activate`, `impact`, `spawned` (`crates/sim/src/weapon/mod.rs:395-411`). The address is `impact`, as the Decisions use.
- `WeaponActivation { origin, direction }` is in scope at push → it never leaves the local weapon stage: `LocalWeaponCommandResult` exports names only (`weapon_stage/commands.rs:40-47, 605`). The remote host runner builds no activation and pushes bare `activate` / `dry_fire` (`commands.rs:167-169`). Emissions are built inside both runners, where `pawn`, `weapon_id` and the impacts are in scope.
- The projectile records its weapon at spawn → today `ProjectileComponent` holds `owner_weapon: EntityId`. For enemies that id is the enemy itself; the weapon name exists only as the AI's `descriptor_class` at spawn (`ai/src/lib.rs:196-201`). Spawn will stamp the weapon's canonical name and an activation key onto the projectile, sourced from `DescriptorProvenance` for pawns and from `descriptor_class` for enemies.
- `AttackParams` "names a weapon" → it already carries `weapon: Option<String>`, mutually exclusive with the inline contact stats (`behavior/recursive.rs:795-860`), under `deny_unknown_fields`. `sound` is added beside `weapon`.
- `WeaponDescriptor` rejects unknown keys → it does not (no `deny_unknown_fields`). The new `sounds` sub-struct carries `deny_unknown_fields`, so an unknown key *inside* `sounds` is rejected, which is exactly what row 33 requires. Unknown top-level weapon keys stay ignored, as today.
- Client hitscan prediction is the only client impact source → predicted projectiles already resolve a full `WeaponImpact` with its normal in `advance_client_predicted_projectiles` (`main.rs:7275`). The client's own projectile impacts come from there.
- The binder's presentation-sentinel rejection in `partition_direct_reaction`, and its twin in `reaction_dispatch`, must admit `@emitter` for `playSound` → unnecessary. The token rides `playSound`'s args (`at: "@emitter"`, per the Boundary inventory), not a primitive or step `target`, so neither sentinel check sees it. A missing emitter is detected where `playSound` resolves against the fire context. That check warns once per (reaction, source) and plays nothing. The existing warn-once set (`SystemReactionIrBindings.warned_missing_inputs`) is `setState`-only, so `playSound` gets its own keyed set, cleared on rebuild like that one.
- The trigger residual holds its trigger and activator → it holds `(TriggerResidualHandle, EntityId trigger, PlayerId)` and calls `fire_prepartitioned_reactions_with_sequences`, which never stamps a fire context. No behavior change: triggers publish no emitter.
- Listener change "respawn, spectate" (P12) → no spectate mode exists, and a dead player pawn is not despawned. The listener's pawn is `followed_player_pawn` (`registry.local_player_movement_pawn()`). P12 is proved by changing the listener pawn between frames.
- Splash contacts → splash impacts (`normal: Vec3::Y`) never enter `WeaponFireEvents.impacts`; only hitscan pushes there (`weapon/mod.rs:654`). An impact's contact set is direct contacts only.
- V4a/V4b install caller → `startup/lifecycle_world_cpu.rs:184-193` (not `lifecycle.rs`). The rerun on staged commit is at `staged_manifest_lifecycle.rs:230-257`.
- `unload_level` is the single unload hook → confirmed, but it is reached only through the level-request queue (`lifecycle.rs:335, 342`). The positional fade-out goes in `unload_level` beside `audio.release_level_sounds()`.

## Delegated answers
- Pawn anchor height: a player pawn anchors at its eye (interpolated transform plus the eye offset camera follow uses), and every other entity at its interpolated transform origin. Fire, reload and movement come from the head, so a remote co-op player's shot is heard where their view is.
- Fade length at unload: 150 ms. That is long enough to avoid a click and short enough to finish before the next level's first frame.
- Seeded attenuation: `minDistance: 2`, `maxDistance: 60`, `curve: "linear"`, in engine units (metres, matching `speed_mps`). Two metres keeps close fights at full level. Sixty metres covers the long sightlines of the dev arenas without hearing through the whole map. `quadratic` maps to kira `Easing::InPowi(2)`. kira interpolates attenuation in decibels.

## Design notes (non-binding, for resumption)
- **Anchor vocabulary.** The anchor type lives in `postretro-entities` beside `SystemCommandFireContext`, since it names `EntityId`. It is emitter identity, not an audio type: `Emitter::Entity { id, point }`, `Emitter::Point(point)`, `Emitter::Contacts(Vec<ImpactContact { point, normal: Option<Vec3>, hit: Entity(EntityId) | World }>)`. The app converts it to the audio boundary's primitive `SoundAnchor` (a u64 entity key plus `[f32; 3]` points).
- **Audio chokepoint.** `audio/spatial.rs` owns every positional voice: its spatial track, its sound handle, its anchor, its last position and its spatial or non-spatial treatment. It is the only file that names `add_spatial_sub_track`, `SpatialTrackBuilder` or `set_position`. Positional requests are admitted and reserved at `play`, then started at `update` against that frame's listener. That is where a contact set resolves to its nearest contact (P4) and where own-pawn treatment is decided (P12). Tracks are built with `persist_until_sounds_finish(true)`, `sound_capacity(1)` and `sub_track_capacity(0)`. Admission requires both the engine SFX count under its cap and SFX `num_sub_tracks()` under `sub_track_capacity()`; direct SFX sounds check `num_sounds()` under `sound_capacity()`. Both SFX capacities are sized to 2× the voice cap, so a slot reclaimed by the engine but still held by kira never refuses a full new cap (P1).
- **Listener.** The frame eye (view-feel evaluate, then `RenderCamera`) moves ahead of `audio.update`. Render and audio read the one evaluated eye; view feel is not evaluated twice. `ListenerState` gains the attached pawn's entity key.
- **Emissions.** `TickEvents` name vectors become emission vectors: name, emitter, and descriptor identity (weapon canonical name, enemy plus attack name or entered-activity path, mover id). The app-side drain lives in a new `crates/postretro/src/sound_events/` module, not inline in `main.rs` (14,191 lines). It fires each named event with `NamedEventDispatchContext { emitter }` and resolves descriptor sounds against a `DescriptorSoundTable`. That table is built at install and rebuilt on staged commit.
- **Client edges.** A small app-side tracker derives reload start, shell and complete from the owner-private `player.reloadActive` and `player.ammo` slots. The fire gate reads `player.ammo`.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 positioned request on spatial track under SFX; muting SFX silences it | `audio::spatial` capturing-backend test: play positioned, render, mute SFX, render ≈ silence | achievable as stated |
| 2 cap N plays, N+1 refused; finished frees; track outlives its sound | mock-backend test on positional voices; assert kira `num_sub_tracks` stays until the sound reports `Stopped` | achievable as stated |
| 3 P1 full cap after reclaim, before mixer runs, all play | mock-backend test: fill, advance, reclaim, then N requests with no mixer step, all `Some` | achievable as stated |
| 4 P8 finish frees at next audio step; earlier request refused | mock-backend test ordering finish → play (refused) → `update` → play (admitted) | achievable as stated |
| 5 right-louder, turning swaps, twice as far quieter | capturing-backend per-channel RMS test | achievable as stated |
| 6 moving entity tracks pose; despawn holds last position and completes | spatial test with an anchor resolver returning moving positions, then `None` | achievable as stated |
| 7 P6 removed before first reposition plays at fire-time point | spatial test: resolver `None` on the first `update` | achievable as stated |
| 8 P12 treatment kept across listener pawn change | spatial test: start with pawn A attached, switch to B | achievable as stated |
| 9 own-pawn non-spatial, other pawn spatial | spatial test asserting treatment per anchor | achievable as stated |
| 10 listener equals render eye incl. view-feel offset | unit test on the extracted frame-eye function feeding both `ListenerState` and `RenderCamera` | achievable as stated |
| 11 P10 view feel advances once per frame; one eye | test: frame-eye evaluation advances `ViewFeelState` exactly once; render and listener read the same value | achievable as stated |
| 12 each descriptor field plays once at its anchor (all kinds) | `sound_events` tests: emissions → requests audio receives, asserting key and anchor per field | achievable as stated |
| 13 P4 multi-pellet one impact at nearest; none on miss; two shots two | sim emission test plus spatial nearest-contact test | achievable as stated |
| 14 P14 projectile despawned on hit still sounds once; per-tick grouping | sim projectile-stage emission test | achievable as stated |
| 15 every impact carries point, normal, entity/world | sim emission tests (local hitscan, projectile) plus netcode ingest test: a remote hitscan declaration with entity and world contacts yields one impact whose contacts carry the declared normals and hit kinds | achievable as stated |
| 16 enemy projectile contact fires `impact` once; weapon sound from projectile | sim test with an enemy-spawned projectile | achievable as stated |
| 17 enemy attack naming weapon: fire at enemy, impact at contact, plus `AttackParams.sound` | ai/sim emission plus `sound_events` resolution test | achievable as stated |
| 18 descriptor sound and reaction both play | `sound_events` test with a `playSound` reaction on `activate` | achievable as stated |
| 19 P11 crush two actors, two sounds | physics blocking test counting emissions with anchor | achievable as stated |
| 20 activity `sound` without `onEnter` plays, fires no reaction | ai apply test plus `sound_events` test | achievable as stated |
| 21 no descriptor sound, nothing and no warn | `sound_events` test with log capture (negative) | achievable as stated |
| 22 P5 client own events once; remote fire and mover edges none | client-path tests over the prediction and reload-edge tracker | achievable as stated |
| 23 client empty mag predicts dry fire, not fire or flash | client fire-gate test | achievable as stated |
| 24 client predicted hitscan into wall plays impact at wall | `resolve_client_hitscan` world-hit test plus emission | achievable as stated |
| 25 client cancelled reload plays start only; completed plays start and complete | reload-edge tracker test | achievable as stated |
| 26 P13 rejected predicted shot plays fire once, nothing more | client reconcile test counting requests | achievable as stated |
| 27 Scripting-surface fixture TS and Luau installs and runs; `door.open` hands one anchored request | `content/dev` fixtures plus an install test observing requests | achievable as stated |
| 28 `on.emitter` from `levelLoad` or crossing skipped with one warning, no dry play | dispatch test with log capture | achievable as stated |
| 29 P7 trigger, death and completion skipped with one warning | dispatch tests with log capture | achievable as stated |
| 30 `on.emitter` plays at anchor on fire, reload, attack, entry, landing, mover | `sound_events` tests per source | achievable as stated |
| 31 install rejects token after `wait` and as a `fire` target; token-free versions install | `reaction_validation` V4a/V4b tests | achievable as stated |
| 32 install rejects `at` with non-SFX bus; accepts `at` alone or with SFX | reaction install test | achievable as stated |
| 33 sound fields parse JS and Luau, round-trip; unknown key in `sounds` rejected; optional | scripting-core descriptor tests, both runtimes | achievable as stated |
| 34 v7 round-trips; v6 loads with none; blank KVP absent | level-format codec tests plus compiler parse test | achievable as stated |
| 35 mover sound keys leave the static-content hash unchanged | `level_content_digest` test | achievable as stated |
| 36 unknown key warns once, level loads; known key silent | install check test with log capture | achievable as stated |
| 37 manifest attenuation changes gain; default when omitted; malformed warns naming field | manifest parser tests (JS and Luau) plus capturing-backend gain test | achievable as stated |
| 38 P9 attenuation reload applies to new sounds only | spatial test: start, change settings, start again | achievable as stated |
| 39 P3 hot-reload key change plays new key; unknown warns once | staged-commit test over the rebuilt table | achievable as stated |
| 40 P2 unload fades all positional voices; none survives or follows | spatial `stop_all_positional` test plus `unload_level` wiring test | achievable as stated |
| 41 typedef fixtures match | `committed_sdk_types_match_current_registry` and snapshot tests | achievable as stated |
| 42 grep gate: kira spatial calls only in chokepoint module | preflight grep (`add_spatial_sub_track`, `SpatialTrackBuilder`, spatial `set_position`) | achievable as stated |
| 43 grep gate: sim names no audio types; IR types Number and Bool; no sound key on wire | preflight grep over `crates/sim`, `crates/ai`, `crates/physics`, `crates/net` plus `IrType` read | achievable as stated |
| 44 own actions heard centered, full level | owner, in-engine | manual |
| 45 enemy attack from its direction, quieter moving away | owner, in-engine | manual |
| 46 door pans to its side; re-pans smoothly on head turn | owner, in-engine | manual |
| 47 killed enemy's attack sound plays out where it died | owner, in-engine | manual |
| 48 SFX volume scales positional sounds | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Extract `crates/audio` (`postretro-audio`) from `crates/postretro/src/audio/`, splitting `mod.rs` (tests to a sibling, boundary types to their own file); no behavior change; layering test pins sim-side crates off it; `crate-graph.md` regenerated | integrating executor | — | done: `cargo test -p postretro-audio` 31 passed (31 before the move); `layering_invariants_hold` passes; `cargo check -p postretro --tests` and `--features dev-tools` clean; `crate-graph --write` |
| 2 | Thinnest risky slice: the `audio/spatial.rs` chokepoint (admission against kira occupancy, SFX capacities 2× cap, deferred start, reposition and freeze, own-pawn treatment, attenuation at start, fade-all). Plus runtime-only mover sound fields and mover emissions anchored at bounds center, played end to end. Rows 1–9, 38, 40, 42 | integrating executor | 1 | |
| 3 | KinematicGeometry v7: codec, compiler KVPs, FGD, loader, component population; hash test. Rows 34, 35 | delegated worker | 2 (component fields) | |
| 4 | Listener is the rendered eye: extract frame-eye evaluation ahead of audio; `ListenerState` attached pawn. Rows 10, 11 | integrating executor | 2 | |
| 5 | Sim emitter plumbing: emissions for weapon (local and remote), reload, impacts (hitscan per activation; projectile contacts grouped per activation per tick, now firing `impact`), AI attack and state entry (including sound-only entry), movement, crush per victim. Projectile stamps weapon name and activation key. Rows 13, 14, 16, 19, 20 (sim half) | integrating executor | 2 | |
| 6 | Emitter token: fire context carries emitter; `playSound(sound, { bus, at })` in Rust, TS and Luau; `EmitterParams`, `DISPATCH_PARAMS.emitter`; warn-once skip; V4a/V4b extension; `at` with non-SFX bus rejected; consumers fixed; typedefs regenerated. Rows 28, 29, 31, 32, 41 | integrating executor | 5 | |
| 7 | Descriptor sound fields (weapon `sounds`, movement `sounds`, `AttackParams.sound`, activity `sound`) in foundation, JS, Luau and the SDK. `DescriptorSoundTable` built at install and rebuilt on commit. Unknown-key check at install and commit. `sound_events` drain resolves descriptor sounds and dispatches with emitter. Rows 12, 17, 18, 20, 21, 30, 33, 36, 39 | integrating executor | 5, 6 | |
| 8 | Manifest `audio.attenuation` (JS and Luau drains, warn-and-fallback, commit applies to new voices). Row 37 | delegated worker | 2 | |
| 9a | Hit-declaration wire: `HitRecord.normal`; hitscan world contacts under the sentinel; host validation of presentation-only hitscan contacts; declared normals in `WeaponImpact`; remote-activation impact emission on host; `WIRE_VERSION` 23. Row 15 (remote half) | integrating executor | 5 | |
| 9 | Client hears its own actions: ammo-gated fire and dry-fire prediction, hitscan world hits and normals (declared through 9a), predicted projectile impacts, reload-edge tracker, client emissions. Rows 22–26 | integrating executor | 5, 7, 9a | |
| 10 | Content: Scripting-surface fixtures (TS and Luau) under their own canonical names, generated sound assets under `content/dev/sounds/sfx`, `door.open` fixture, `arena-lights.ts` migrated. Row 27 | integrating executor | 6, 7, 3 | |
| 11 | Grep gates and full preflight. Rows 42, 43 | integrating executor | all | |
