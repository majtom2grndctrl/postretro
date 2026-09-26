# E12--positional-sound-events

Brief · resumable · Epic 12 · reads: `context/lib/audio.md`, `context/lib/scripting.md` §10.4 §12, `context/lib/entity_model.md` §Components (Weapon vocabulary), `context/lib/build_pipeline.md` §PRL section IDs (id 43) · read at b3548b603

## Problem

A requested capability, raised by the developer, whose basis is a false premise in the roadmap. Epic 12 assumed gameplay already emits typed sound events that audio only needs to play. It does not. Every gameplay event reaches the app as a bare global name (`TickEvents`), and the emitter identity each push site holds — firing pawn, impact point, enemy, mover — is dropped at the push. The one gameplay path to `Audio::play` is `SystemReactionCommand::PlaySound { sound, bus }`, which has no position. The audio module plays everything dry on a single listener. So nothing can be spatialized, and a reaction addressed `"activate"` fires for every weapon and every pawn. When this is done, weapons, enemies, player movement and movers play authored sounds that come from where they happen: they attenuate with distance, pan with facing, and follow a moving source. Modders author those sounds on descriptors and can position a scripted `playSound` at the event's emitter. Step 1 of the re-split epic; reverb zones, peer audio and occlusion follow (roadmap Epic 12).

## Decisions

- **Spatial playback and sound events ship as one unit.** Spatial audio with no positioned caller would land as a stub. This merges the roadmap's former "Spatial audio" and "Sound-event playback" items.
- **Two authoring surfaces, both in this brief.** Descriptor sound fields are the common path on every emitting kind. A positional `playSound` covers scripted cases. On one event, a descriptor sound and a reaction addressed to that event both play; neither suppresses the other.
- **Why every kind gets descriptor sounds.** Where the engine owns an event name shared across instances (`activate`, `impact`, `enemyAttack`), an addressed reaction is global, so the descriptor is the only per-weapon or per-attack path. Movers and behavior states have author-owned addresses, and movement events fire for the local pawn only, so a positional reaction already serves them. They get descriptor sounds anyway, as a declared ergonomic: script-free authoring, with the sound declared on the thing that makes it.
- **Descriptor fields sit beside each kind's presentation-only fields.**
  - `WeaponDescriptor.sounds` sits beside `viewmodel`.
  - `PlayerMovementDescriptor.sounds` sits beside `view_feel`. This is a new top-level block, against the `player-descriptor-composition` draft's direction; if that draft lands first, `sounds` moves into its shape.
  - `AttackParams.sound` plays on that attack's `enemyAttack`.
  - `BehaviorActivityDescriptor.sound` plays on entry, whether or not `on_enter` is authored.
  - Movers get the FGD keys `open_sound`, `close_sound`, `blocked_sound` and `crush_sound`.

  Sound fields are presentation-only descriptor content. They reach runtime only as components already hold descriptors: the mover component carries its keys as it carries `*_event`, and a brain holds its behavior graph by reference. No sound key enters the wire or the replicated snapshot.
- **Weapon sounds follow the weapon, whoever wields it.** An enemy attack that names a weapon plays that weapon's fire and impact sounds, even though enemy attacks bypass the weapon stage; this avoids the actor-kind branch `entity_model.md` forbids. `AttackParams.sound` plays as well when authored. Every projectile records at spawn the weapon descriptor it was fired from, so its contact resolves sounds from the projectile itself, whoever fired it.
- **Mover sounds append to KinematicGeometry (id 43) as v7.** Versions 1–6 stay loadable and decode with no sounds, per the section's append policy (`build_pipeline.md` id 43). The keys stay out of the multiplayer static-content hash.
- **Positional `playSound` uses an opaque emitter token, following the `@activators` precedent.** Named-event sources (weapon, reload, impact, AI, movement, mover) publish `emitter` in their dispatch scope, and `playSound`'s `at` accepts only that token. `scripting.md` §12 keeps both of its gates: a source that does not publish `emitter` skips a reaction reading it, with a warn-once, and never plays it dry. The install checks that already reject a sentinel after `wait` and a scoped reaction as a `fire` target extend to the new token. This amends §12's "tokens are legal only in trigger-event builders". No Vec3 IR value type is added.
- **Impacts: one event per activation per tick, carrying every contact of that tick.** Each contact carries its point, its normal, and what was hit (an entity or world geometry). A hitscan multi-pellet shot therefore yields one impact; a projectile contact yields its own. Projectile contacts now fire `impact` reactions as hitscan does; today they fire none. `impact` therefore fires for every weapon's contacts, whoever wields it, enemy projectiles included. No current content addresses `impact`. The descriptor impact sound and `at: on.emitter` resolve to the contact nearest the listener of the frame that plays it, so each peer can later resolve against its own listener. Carrying the normal and the hit target lets a later surface-material brief choose sounds by surface without new plumbing.
- **Every other event has a defined anchor.** Weapon fire, dry fire and reload anchor at the firing pawn. AI attack and state entry anchor at the enemy. Movement events anchor at the local pawn. A mover anchors at the center of its current world bounds, because mover transforms are origin-relative. An entity anchor follows the entity's render-interpolated pose each frame. Once the entity is gone, the sound freezes at its last position and plays out. Anchors capture a point at fire time, so a same-tick despawn still has a position.
- **The listener's own pawn plays non-spatial.** A sound anchored on the pawn the listener is attached to plays on SFX without spatialization, which avoids panning at near-zero distance. Every other pawn's sounds spatialize, including a remote co-op player's fire heard on the host. A sound keeps the treatment it started with.
- **The listener is the rendered eye.** The listener pose moves from the raw tick camera to the interpolated eye render uses, including the view-feel offset. It is evaluated once per frame and shared by audio and render. Audio → Render order stays (`index.md` §2).
- **The voice counter never disagrees with kira.** Each positional sound plays on its own kira spatial track under the SFX bus, so the SFX volume control governs it. A request is admitted only when the engine's voice count and kira's live slot occupancy under SFX both have room. A slot the engine has reclaimed but kira has not yet removed still counts as occupied. Over the cap a request is refused, never queued. This keeps `audio.md` §1's rule, that no play the counter accepts fails in kira, exact under kira's deferred removal. Reclaim never cuts a tail.
- **A connected client hears its own actions.**
  - Fire prediction is gated on the replicated ammo slot, so an empty magazine predicts a dry fire, not a fire and muzzle flash.
  - Hitscan prediction keeps world-geometry hits and every hit's normal, so predicted impacts match the host's contact data.
  - Reload edges derive from the replicated owner-private reload and ammo slots. Start is the reload flag rising. A shell is ammo rising while reloading. Complete is the flag falling after ammo rose. A cancel plays nothing. Own reload sounds lag by one round trip, and shells that arrive in one snapshot sound once.
  - Landing and jumping come from predicted movement.
  - Remote peers' and world sounds wait for step 3, which reuses this derive-from-replicated-state pattern for door edges.
- **Level lifetime.** Unload, restart and return-to-frontend stop every positional voice with a short fade; none outlives its world. A hot-reload manifest commit rebuilds the descriptor sound table and reruns the unknown-key check, beside the V4a/V4b rerun. Attenuation applies when a sound starts, so live sounds keep theirs.
- **Unknown sound keys are caught at install and on reload.** Every key a descriptor or a `playSound` names is checked after the registry loads. An unknown key warns once and the level still loads. The play-time drop remains as the backstop.
- **Attenuation is a mod-wide setting with an engine-seeded default** (`scripting.md` §1: expose the axis, seed the default). It lives in the manifest's `audio.attenuation` block, beside `render.bloom` and `movers.autoCloseMs`, and follows their policy: a malformed value warns naming the field and falls back to the default.
- **Placement: presentation, host-local, app drain.** Sound stays in the Presentation class, resolved after the tick loop. The sim carries emitter identity and contact data only, and never touches audio.
- **Non-goals.**
  - Reverb, peer audio and occlusion are later Epic 12 steps.
  - Doppler and HRTF: kira has neither. HRTF is already an `audio.md` §7 non-goal.
  - Surface-material impact sounds and footsteps: the impact data is carried for a later brief, and no footstep event exists.
  - Remote pawns' movement sounds: those events are local-pawn only by design (`run_movement_tick`).
  - Descriptor keys for dash, crouch, slide, reload cancel, reload blocked and projectile spawn: their events stay reachable through reactions. Keys can be added when content asks.
  - Looping positional sounds, such as a mover travel loop: every path plays one-shots, and a loop needs a stop edge this brief does not define.
  - `at` on trigger events: the trigger residual drain holds the trigger and its activator but passes no fire context, and a brush volume has no settled anchor point.
  - Per-descriptor event addresses: the owner rejected them.
  - Per-voice volume, pitch randomization and per-sound attenuation: the mod-wide setting covers tuning for now.
  - Target-anchored sounds (pain, flesh hits): these belong to the E16 impact-policy presentation lane (`impact_policy`).
  - Per-activation sounds for alt-fire: `context/research/weapon-model.md` §3 puts sound under each activation, so flat `sounds` moves with damage and resolution when alt-fire lands.
  - The kira 0.12.4 bump: Epic 12 step 0, a separate chore.

### Scripting surface

```ts
// Descriptor path — engine plays each at the event's anchor, on SFX.
defineEntity({
  canonicalName: "reference_shotgun",
  components: {
    weapon: {
      // …tuning, models, placement…
      sounds: {
        fire: "sfx/shotgun_fire",
        dryFire: "sfx/click",
        impact: "sfx/pellet_hit",
        reloadStart: "sfx/shotgun_open",
        reloadShell: "sfx/shell_in",      // perShell reload style
        reloadComplete: "sfx/shotgun_pump",
      },
    },
  },
});
// player entity: components.movement: { /* … */ sounds: { land: "sfx/land", jump: "sfx/jump" } }
// behavior graph: attacks: { bite: { damage: 10, sound: "sfx/bite" } }
//                 activity: { onEnter: "alerted", sound: "sfx/growl" }
// TrenchBroom kinematic_mover KVPs: open_sound, close_sound, blocked_sound, crush_sound

// Mod-wide attenuation, in the start script. Every field is optional; omission uses the engine default.
defineMod({
  // …name, id, version…
  audio: { attenuation: { minDistance: 2, maxDistance: 60, curve: "linear" } }, // curve: "linear" | "quadratic"
});

// Reaction path — second argument becomes an options object (breaking; consumers fixed in-pass).
defineReaction("door.open", (on: EmitterParams) =>
  playSound("sfx/door_open", { at: on.emitter }));
defineReaction("lowHealthAlert", playSound("sfx/test_tone", { bus: "sfx" })); // unanchored = 2D
```

The TS surface ships with its Luau mirror: `playSound(sound, { bus = "sfx", at = on.emitter })` and `EmitterParams`. `at` with `bus` other than SFX is rejected at install.

## Acceptance

### Automated
Playback
- [ ] A positioned request plays on a spatial track under SFX. Muting SFX while it is already playing silences it.
- [ ] With an SFX cap of N, the Nth concurrent positional voice plays and the (N+1)th is refused. A finished voice frees its slot, and its track is not dropped before its sound ends.
- [ ] After every SFX voice has finished and been reclaimed, a full cap of new positional requests issued before the mixer runs again all play. The mixer refuses none of them. (pin P1)
- [ ] A voice that finishes frees its slot at the next audio step. A request earlier in that frame is still refused at the cap. (pin P8)
- [ ] A positioned sound to the listener's right is louder in the right channel than the left. Turning the listener to face the other way swaps which channel is louder. The same sound twice as far away, beyond the minimum distance, is quieter.
- [ ] A sound anchored on a moving entity tracks that entity's interpolated pose across frames. After the entity despawns, the sound holds its last position and completes.
- [ ] A sound whose entity is removed in the frame the sound starts, before audio first moves it, plays out at its fire-time point. (pin P6)
- [ ] A sound keeps the spatial or non-spatial treatment it started with when the listener changes pawn. (pin P12)
- [ ] A sound anchored on the listener's own pawn plays non-spatial. The same event on another pawn plays spatial.
- [ ] The listener pose for a frame equals the eye render uses that frame, including the view-feel offset.
- [ ] With the eye computed ahead of audio, view bob and view impulses advance once per frame, and render and the listener read one evaluated eye. (pin P10)

Sources and anchors
- [ ] Each descriptor field plays exactly once on its event with the defined anchor: fire, dry fire, impact, reload start, shell and complete; enemy attack and state entry; land and jump; and a mover's open, close, blocked and crush edges.
- [ ] A multi-pellet shot with several contacts plays exactly one impact sound and fires `impact` reactions exactly once, both at the contact nearest that frame's listener. A shot with no contact plays no impact sound. Two separate shots in one tick play two impact sounds. (pin P4)
- [ ] A projectile despawned on the tick it hits still plays its impact sound, at the contact point, and fires `impact` reactions once. A projectile weapon whose contacts land on different ticks sounds once per tick with a contact. (pin P14)
- [ ] Every impact request carries each contact's point, its normal, and whether it hit an entity or world geometry.
- [ ] An enemy projectile's contact fires `impact` reactions once and plays its weapon's impact sound, resolved from the projectile, not from the enemy.
- [ ] An enemy attack naming a weapon plays that weapon's fire sound at the enemy and its impact sound at the contact. An `AttackParams.sound` on the same attack also plays.
- [ ] A descriptor sound and a reaction addressed to the same event both play.
- [ ] A mover crushing two actors on one tick plays two crush sounds. (pin P11)
- [ ] A behavior activity with a `sound` and no `onEnter` plays its sound on entry and fires no reaction.
- [ ] An event whose descriptor names no sound plays nothing and warns nothing.
- [ ] On a connected client, the local player's own fire, dry fire, impact, reload start/shell/complete, landing and jumping each play once. A remote peer's fire and any mover edge play nothing there. (pin P5)
- [ ] On a connected client with an empty magazine, pressing fire predicts a dry fire: the dry-fire sound plays, and no fire sound or muzzle flash does. With ammo it predicts a fire and no dry fire.
- [ ] On a connected client, a predicted hitscan shot into a wall plays its impact sound at the wall contact.
- [ ] On a connected client, a cancelled reload plays its start sound only; a completed one plays start and complete.
- [ ] A predicted shot that the host rejects has already played its fire sound once, and plays nothing more. (pin P13)

Scope token
- [ ] The Scripting surface example installs and runs as a `content/dev` fixture in TS and Luau, under its own canonical name, with every sound key it names present under `content/dev/sounds`. The `door.open` fixture hands audio one request, anchored at the mover's bounds center. Every "plays" in Sources and anchors is observed the same way: as the request audio receives, with its anchor.
- [ ] A reaction reading `on.emitter`, fired from `levelLoad` or a crossing, is skipped with one warning and plays nothing, including no dry fallback.
- [ ] A reaction reading `on.emitter`, fired from a trigger, a death, or another reaction's completion, is skipped with one warning and plays nothing. (pin P7)
- [ ] A reaction reading `on.emitter` on fire, reload, enemy attack, state entry, landing and a mover edge plays at that event's defined anchor.
- [ ] Install rejects the token after a `wait`, and rejects firing a reaction that reads it through `fire`. The same reactions without the token install.
- [ ] Install rejects `at` paired with a non-SFX bus, and accepts `at` alone or with the SFX bus.

Content and format
- [ ] The descriptor sound fields parse in JS and Luau and round-trip. An unknown key inside `sounds` is rejected, and every field is optional.
- [ ] KinematicGeometry v7 round-trips `open_sound`, `close_sound`, `blocked_sound` and `crush_sound`. A v6 file still loads with no mover sounds, and a blank KVP reads as absent.
- [ ] Mover sound keys do not change the multiplayer static-content hash.
- [ ] An unknown sound key named anywhere warns once at install and the level loads. A known key warns nothing.
- [ ] A manifest attenuation changes the gain of a sound at a fixed distance, relative to the default. Omitting the block uses the seeded default. A manifest with `minDistance` ≥ `maxDistance`, a negative distance, or an unknown curve warns naming the field and uses the default; the mod still loads.
- [ ] A manifest reload that changes attenuation applies to sounds started after it. Sounds already playing keep their attenuation. (pin P9)
- [ ] A hot-reload commit that changes a descriptor sound key plays the new key on the next event, and one that introduces an unknown key warns once. (pin P3)
- [ ] Unloading, restarting or returning to the frontend with positional voices live fades every one of them out. None plays after the next level installs, and none follows an entity from the next level. (pin P2)
- [ ] The generated typedef fixtures match the new surface.
- [ ] (grep gate) The sim crates name no audio types. The IR value types stay Number and Bool. No sound key appears in a snapshot or other wire type.

### Manual
- [ ] Own fire, dry fire, reload, landing and jumping are heard centered, at full level.
- [ ] An enemy's attack is heard from its direction and gets quieter as it moves away.
- [ ] A door opening off to one side pans to that side. Turning the head re-pans it without a jump.
- [ ] When an enemy is killed while its attack sound plays, the sound plays out from where it died rather than cutting off.
- [ ] Changing the SFX volume setting scales positional sounds with the rest of SFX.

## Path

- **Identity plumbing.** `TickEvents` fields go from bare names to name plus emitter plus fire-time point. Impacts carry a per-activation contact list (`WeaponImpact` already holds point, normal and target; `ProjectileContactEvent` needs its normal and target kept). Each push site already has what it needs; see `research.md` §Anchor sources. The first slice is movers: `mover_event_dispatch_addresses` already scans the entities, so a door can play positioned end to end with the fewest touches. It falsifies the riskiest assumption, the spatial-track lifecycle under kira, before the source fan-out.
- **Fire context.** Extend `SystemCommandFireContext` / `NamedEventDispatchContext` with an optional emitter and point, populate them in `drain_named_events_with_sequences`, and have the `playSound` handler read them. The binder's presentation-sentinel rejection in `partition_direct_reaction`, and its named-dispatch twin in `reaction_dispatch`, admit `@emitter` for `playSound` only.
- **Audio module.** `SoundRequest` gains an optional anchor: an entity, a point, or an impact's contact set. The voice table holds the spatial track beside the sound handle, and `update` repositions live anchors and resolves a contact set to its nearest point. Admission checks kira's live occupancy through the SFX track handle's `num_sounds()` / `num_sub_tracks()` against its capacities. kira shape and capacity notes are in `research.md` §kira.
- **Descriptor sound dispatch.** At install, gather a table from descriptor canonical name to sound keys, beside `weapon_presentation_models`. The weapon is found at fire time through `DescriptorProvenance.canonical_name`.
- **Rival shapes.**
  - Per-descriptor event addresses (e.g. `events: { fire: "pistolFire" }`) plus positional reactions, with no descriptor sounds. The owner rejected it: more ceremony per sound, and the global-name problem moves into every mod.
  - Descriptor sounds only for weapons and attacks, with the other kinds relying on their authored addresses. The owner kept all four for script-free authoring.
- **Split-first files.** `crates/postretro/src/main.rs` far exceeds 800 lines. Land the new drain and sound-routing code in the `audio/` module or a sibling of the named-event drain, not inline. `audio/mod.rs` (930 lines) splits before the anchor and voice work lands.

## Open questions

- Exact eye-height offset for a pawn anchor, feet versus eye — **delegated**.
- Fade length at unload — **delegated**.
- Seeded attenuation default values in engine units (kira's defaults are 1–100 units, linear) — **delegated**: pick them against the dev maps' scale and record them.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Weapon sounds | `WeaponDescriptor.sounds` | `sounds` { `fire`, `dryFire`, `impact`, `reloadStart`, `reloadShell`, `reloadComplete` } | same | same | n/a |
| Movement sounds | `PlayerMovementDescriptor.sounds` | `sounds` { `land`, `jump` } | same | same | n/a |
| Attack sound | `AttackParams.sound` | `sound` | same | same | n/a |
| Activity entry sound | `BehaviorActivityDescriptor.sound` | `sound` | same | same | n/a |
| Mover sounds | `KinematicMoverComponent.{open,close,blocked,crush}_sound` | PRL id 43 v7 strings | n/a | n/a | `open_sound` `close_sound` `blocked_sound` `crush_sound` |
| Emitter token | emitter sentinel (beside `SequenceTarget`) | `at: "@emitter"` | `on.emitter` (`EmitterParams`) | `on.emitter` | n/a |
| Attenuation | `ModManifestResult` audio attenuation | `audio.attenuation` { `minDistance`, `maxDistance`, `curve` } | same | same | n/a |
| playSound options | `PlaySoundArgs { sound, bus, at }` | `{ sound, bus?, at? }` | `playSound(sound, { bus?, at? })` | `playSound(sound, { bus?, at? })` | n/a |

## Wire format

KinematicGeometry id 43 v7 mirrors how v6 appended carried-light links: a per-mover tail after the v6 fields. It holds four optional strings in the order open, close, blocked, crush, each with the same optional-string encoding the `*_event` fields use, so an absent value encodes exactly as an absent event does. Little-endian throughout. A v1–v6 reader path decodes all four as absent.
