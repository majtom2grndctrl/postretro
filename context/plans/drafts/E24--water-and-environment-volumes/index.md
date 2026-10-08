# E24--water-and-environment-volumes

Brief · resumable · Epic 24 (Environment Volumes & Fluids) · reads: `context/lib/entity_model.md` §4, §5, §7 · `context/lib/movement.md` §2, §4, §7 · `context/lib/networking.md` §What gates, and what replicates instead · `context/lib/build_pipeline.md` §Source-format neutrality, §Compiler pipeline · `context/lib/development_guide.md` §Workspace · `context/lib/rendering_pipeline.md` · `context/lib/audio.md` §6 · read at 90c9b9687

## Problem
Owner-requested capability. Modern retro shooters experiment with design fundamentals, as 90s games did before standards existed, and the engine exists to support that through an expressive high-level API and approachable FGD keys. Mappers want water to swim in, regions with their own gravity, and regions that push: low-gravity rooms, updrafts, wind, currents. The engine has neither. World gravity is one level-wide scalar, seeded from worldspawn `initialGravity` (stored in the FogVolumes section) and threaded into movement as `gravity: f32`. Collision has no volume query, no contents filtering, no fluid and no swim state, and scripts cannot see a player's movement state. When done:
- A mapper places a fluid volume or a gravity volume. Overlapping volumes combine. A fluid can be any convex shape, floating in mid-air included.
- A gravity volume sets how strongly bodies fall there. A push volume adds a force in any direction: an updraft that lifts, a wind or current that pushes.
- A player in a fluid swims and can jump from the surface.
- A player, an AI agent and a particle in a gravity volume take that volume's gravity. Players and particles take a push volume's push.
- Fluid faces are visible from inside and outside.
- With the eye inside a fluid, the view tints and audio muffles.
- Hitscan crossing a fluid face splashes.
- A mod reads, per player, whether that player is swimming, how deep, and in which fluid: the base co-op drowning, air meters and fluid damage build on. These per-owner slots sit on E16's deferred per-seat crossing spec (Decisions).
- A co-op client predicts all of it with no corrections.
The query layer this adds is the first piece of the engine-owned collision substrate described in `research.md` §Substrate direction.

## Decisions
- **New crate `postretro-collision` for the volume substrate.**
  - Pure geometry over level data: no movement, no entities. Physics depends on it. It is the home later E24 steps move mover and trimesh collision into without inverting a physics dependency. This amends `development_guide.md` §Workspace "Target shape"; precedent: `visibility`.
  - Convex leaves (plane set, AABB, contents bitmask) under a BVH built at level load, not baked into PRL (`research.md` §Bake versus load-time build). Leaves answer point-containment, capsule-immersion and ray-crossing queries, each filtered by a contents mask.
  - Sized for ambitious content, not expected counts, and held to the stress budget below.
  - The query core takes its own plain input types; a thin adapter converts loaded level data. In this brief `level-compiler` does not depend on the crate.
  - parry is not used. Static trimesh and mover collision stay in `physics::collision`.
- **Environment resolution is one pure function.** It maps a body's position and capsule, the volumes and the level default to gravity (signed vertical), push (a vector), fluid and immersion. Host movement, client prediction, reconcile replay, AI agents and particles call it per step, per body, at that body's position. Intents receive the resolved value and never query; environment is a function of position and static data, so `movement.md` §4's forward-contact rule does not govern it.
- **Purpose-named authoring entities, one canonical environment volume.**
  - The FGD exposes `fluid_volume` (`fluid`, `priority`), `gravity_volume` (`gravity`, `priority`) and `push_volume` (`push`, `priority`). `priority` defaults to 0. The format adapter translates each into the canonical environment-volume record with only its field set; nothing downstream sees the authoring split (`build_pipeline.md` §Source-format neutrality).
  - `gravity` is one number, signed like worldspawn `initialGravity` (`"-3"` is low gravity, `"0"` is weightless). `push` is three numbers, an acceleration in map axes in m/s² (`"0 0 15"` is an updraft, `"8 0 0"` a wind). Any other shape fails the build, naming the entity.
  - A blank `fluid`, `gravity` or `push` fails the build, naming the entity. A brush that needs several fields is coincident entities, which per-field resolution combines; a current is a `fluid_volume` with a coincident `push_volume`.
  - Each brush becomes one convex leaf. The record carries its source map entity index, so every diagnostic can name the entity.
  - Volumes are non-solid. They are peeled before the BSP like `trigger_volume`, and never seal cells, block portals, or enter static geometry, static collision, lightmaps, SDF or navmesh.
- **One regional overlap rule.** For each field, the value comes from the winning volume among those that set that field and are in reach. With none, the level default applies.
  - Winner: highest priority; then the smallest summed brush volume, so a pool inside a room wins without priorities; then earliest map entity order, for exact ties only.
  - Gravity and push reach is the body's position.
  - Fluid reach is the capsule's vertical segment, feet to head. The fluid is the winner among fluid volumes intersecting it. Immersion is the segment's coverage by the union of the winner's brushes, so leaving through any face, bottom included, drops it.
  - Eye-dependent presentation keys on the camera eye point, with hysteresis (Presentation).
  - Reverb, when built, becomes a point field under this rule. `audio.md` is amended then, not at this promotion.
- **Gravity is vertical and defines falling; push is a force in any direction.** Two fields, so authored meaning never changes later.
  - `gravity` keeps today's meaning in every movement state, with no new rules. It is what the later gravity-frame spec orients the player to.
  - `push` is an extra acceleration that never defines "down". Ground classification, the capsule, step-up, ground-stick, camera and navmesh keep +Y as up. Lineage: Quake 2's `trigger_push` and water currents.
  - `push_volume` sits beside `movement.md` §3's "never generic `apply_force(body)`" guardrail and stays clear of it: a push is static, map-authored regional data, never a script primitive that forces an arbitrary body.
  - Three rules make push coherent (`research.md` §Gravity as a force):
    1. **Lift-off.** A grounded body whose push points up harder than gravity pulls down leaves the ground, as a knockback launch does. With no push, nothing changes, so upward level gravity behaves as before.
    2. **Terminal speed** caps speed along the push direction, so an updraft shaft does not accelerate forever.
    3. **After the intent.** Push adds to velocity after the state intent and is never clipped by the intent's speed caps. Ground friction gives a steady drift.
  - Slide boost and AI agents read only `gravity`, so push never feeds a slope boost and agents stay on the navmesh.
  - Swimmers take push, so a fluid with a coincident push volume is a current. Particles take push unscaled by buoyancy.
- **Level default gravity moves out of FogVolumes** into the new section's header. `initialGravity`, `getGravity` and `setGravity` (over the raw `worldGetGravity`/`worldSetGravity` primitives) keep their signed vertical shape and set it, so a map with only `initialGravity`, upward included, behaves as before. The level has no default push. Volume gravity overrides it inside the volume.
  - `gravity` is absolute and never depends on script state, so `setGravity` never reaches inside a gravity volume. A separate key added later, such as `gravityScale`, could scale level gravity without changing `gravity`'s meaning, so absolute forecloses nothing (rival: `research.md` §Fourth direction review).
  - A vector script API and M7's `area.setGravity` are not built here.
- **Ownership is split three ways, as a closed field set.**
  - The volume owns the place: shape, priority, gravity, push, which fluid. A mod-manifest fluid descriptor owns the substance: viscosity, view tint, muffle, surface opacity, enter and exit sounds. The player descriptor's `movement.swim` block owns the swimmer.
  - Amend `entity_model.md` §4, `build_pipeline.md` §Load-time gameplay declarations and `movement.md` §7 at promotion: maps may author static regional gravity, push and fluid placement, baked at compile time. Any further regional field needs its own amendment. Maps still cannot mutate gameplay after load or override per-archetype tuning. Gas and haze stay with fog volumes.
  - A fluid field out of range, or a duplicate fluid name, is rejected with a warning naming the field; the later duplicate loses.
- **Swim is a native movement state.**
  - Any state enters swim when immersion reaches `enterDepth`, and the capsule returns to standing size on entry. Swim leaves when immersion falls below `exitDepth`.
  - Drag is the fluid's viscosity, only while swimming. Speeds and acceleration come from the descriptor. With no vertical input a swimmer sinks at `sinkSpeed` scaled by the resolved gravity relative to standard gravity (−9.81 m/s²), so a zero-gravity pool floats and an upward-gravity pool rises. Push acts on swimmers as a current.
  - Fluid jump: with the tick eye from the resolve outside the fluid, jump sets upward velocity to `fluidJumpVelocity` and leaves swim, with no solid needed. When exit and a fluid jump both qualify on one tick, the jump wins. After a fluid jump, swim cannot re-enter until immersion first falls below `exitDepth`.
  - A player descriptor without a `swim` block cannot swim; the pawn walks fluid floors, a legitimate wade-only design. A level with fluid volumes loaded for such a player warns once.
  - Swim tuning is never map-overridable.
- **Who resolves what.** Player pawns resolve the full environment. AI agents resolve gravity only; they ignore push, do not swim and walk fluid floors. Particles resolve gravity and push at their own position on every particle step, once per rendered frame on frame time; buoyancy scales the resolved vector, and particles are unhashed presentation. Projectiles apply no gravity today and are untouched.
- **Scripts read the environment per player, through per-owner engine slots built on E16's deferred per-seat crossing spec.**
  - Three readonly slots, one value per seat: `player.swimming` (bool), `player.immersion` (0..=1) and `player.fluid` (fluid name, empty when dry), in the generated TypeScript and Luau SDKs. Owner-private: each client reads its own.
  - Nothing shipped carries them. Engine catalog slots are never per-owner (`EngineStateCatalogEntry::slot_record`), and `onStateCrossing` on a per-owner slot is rejected at bind, with per-seat crossing deferred to its own spec (`plans/done/E16--per-player-currency`, Out of scope and Decisions). Derivation: `research.md` §Fourth direction review.
  - That spec, to be drafted, is a prerequisite: per-owner engine catalog slots, a per-seat `previous`, and an owner-carrying crossing dispatch input. E24 adds no event source; the owner-carrying input is the prerequisite's.
  - The slot slice lands last and is blocked until the prerequisite lands. Substrate, gravity, push, fluids, swim and presentation do not wait on it.
  - This is the first script-visible projection of movement state, and amends `entity_model.md` §7b ("purely engine-internal").
- **Co-op parity.**
  - **Hashed.** Volume records (shape, priority, gravity, push, fluid name, map order) join `level_content_digest`, like static collision. Surface faces and the header's default gravity are named skips.
  - **Sent.** Level gravity rides the host's tuning payload, which the host resends whenever it changes, so a mid-level `setGravity` reaches clients. It applies on arrival, and one correction per change is accepted. The client never reads its own level gravity. This is in effect a one-value lane, and partly answers `plans/done/E15--session-lifecycle`'s deferred gravity replication; runtime-mutable volumes still need the general lane (Non-goals).
  - **Fluids.** The host resolves names against its manifest at install and on every manifest reload, warning on an undeclared name, which loads dry. The tuning payload carries the host's fluid table with movement fields only. A client resolves names only against that table, never its own registry, because admission gates on mod id and never version (`networking.md` §Mod identity). Presentation fields resolve locally by name, as sound keys do; a client without the fluid shows none. Fluid declarations stay out of the mod compatibility digest.
  - A hot reload that rebuilds a swimming pawn's movement component may cost one correction, accepted as a dev-time cost.
  - `Swim` joins the movement-state discriminant, which bumps the wire version. Gravity and the fluid table join the tuning payload, which bumps `TUNING_PAYLOAD_EPOCH`.
  - "No corrections" means every recorded correction is at most 1e-4 m under `light_link`.
- **Presentation.**
  - Fluid faces draw in a renderer-owned translucent pass after opaque forward and before smoke: alpha blend at the fluid's surface opacity, depth test on, depth write off, two-sided, turbulent UV warp. They are lit per fragment by baked SH ambient, baked direct SH and dynamic lights, on smoke's camera, lighting and SH layouts plus a fluid material group.
  - The surface is every brush face not buried in solid world and not shared with another brush of the same entity, textured from that face. A face partly buried is clipped at compile time to its exposed part.
  - Tint is an engine-owned screen-effect layer, never a write to the script-writable `screen.*` slots.
  - Tint and muffle switch on an eye-depth hysteresis band: on once the camera eye is a few centimetres inside the fluid, off once it is that far outside. The tint ramps in and out. Head bob carries the eye across the surface and the tint is full-screen at strength up to 1, so it is also a channel of the photosensitivity limiter, beside `screen.flash` and `screen.vignette` (`rendering_pipeline.md` §7.8).
  - Muffle is a primitive-only low-pass on the world-sound bus while the camera eye is in a fluid. UI and music are not muffled, and it composes with per-voice occlusion. No kira types cross the boundary.
  - Entry and exit sounds play on swim transitions, from forward prediction only, never from replay.
  - Splash: a hitscan ray crossing a surface face before its blocking hit reports a fact (point, outward normal, fluid); its look and sound sit above it, per `plans/done/E16--combat-presentation-substrate`. A ray starting on a face never splashes there. Two adjacent fluid entities give two splashes.
  - Because water draws before smoke, a particle below the surface draws over the water seen from above. Accepted.
- **Non-goals.**
  - Fluid damage, drowning and air meters: E24 builds none. Once the slot slice and its prerequisite land, mods can react to per-player swim state. Whether that reaches damage or a per-player air timer depends on what the prerequisite and E16's open policy deliver: environmental damage runs no impact policy in v1 (`plans/done/E16--impact-death-lifecycle`), and `perOwner` with `accumulate` is rejected (`store_bridge.rs`). E24 does not solve either.
  - Splash from non-hitscan projectiles, and a visual splash on player entry: follow-ups on the same fact.
  - Refraction and depth absorption: no plan yet.
  - Walking on walls or ceilings, and reorienting to gravity: the gravity-frame spec, which orients to `gravity`, never to `push`.
  - Wading drag and AI swimming: follow-ups.
  - Mover convex collision, static trimesh cleanup and clip volumes: later E24 steps (`research.md` §Substrate direction).
  - Runtime-mutable volumes and volumes on movers: they need E15's deferred continuous replication lane. Flood-and-drain set-pieces are the named follow-up.
- **Known limits, accepted.**
  - An AI agent knocked airborne in zero gravity stays airborne until something moves it.
  - A pool built from two `fluid_volume` entities draws a surface between them. The FGD help text tells mappers to make one entity with several brushes.
- **Doc amendments at promotion**, besides those above: `entity_model.md` says particles step once per rendered frame, and §7b records the readonly movement projection. `rendering_pipeline.md` §7.8 lists the fluid tint as a limiter channel.

### Scripting surface
TypeScript shown. The Luau SDK mirrors it with the same names, and both ship. The fixture lands in content/dev's existing manifest and player descriptor.

```ts
// start-script.ts — inside content/dev's existing defineMod
fluids: [
  defineFluid({
    name: "water",
    viscosity: 2.0,                      // 1/s drag while swimming; ≥ 0
    viewTint: [0.10, 0.30, 0.40, 0.35],  // rgb + strength, each 0..=1
    muffle: 0.8,                         // 0 = none, 1 = maximum low-pass
    surfaceOpacity: 0.6,                 // 0..=1
    enterSound: "sfx/splash_in",         // optional
    exitSound: "sfx/splash_out",         // optional
  }),
],

// player.ts — inside the player descriptor's `movement`; absent = cannot swim
swim: {
  speed: 4.5,             // m/s target swim speed
  accel: 6.0,             // m/s² toward wish velocity
  sinkSpeed: 0.8,         // m/s drift along gravity with no vertical input, at 9.81 m/s²
  enterDepth: 0.5,        // immersion fraction that enters swim
  exitDepth: 0.35,        // immersion fraction that leaves swim; < enterDepth
  fluidJumpVelocity: 6.5, // m/s upward on a jump from the surface
},
```

Readonly per-owner engine state, generated into both SDKs once the prerequisite lands: `player.swimming`, `player.immersion`, `player.fluid`.

## Acceptance
### Automated
**Queries and resolution**
- [ ] A point strictly inside, exactly on a face, and just outside a convex volume reports inside, inside, outside. A query whose mask excludes fluid never reports a fluid volume: a point inside a fluid volume queried with a gravity-only mask reports no fluid, and the same point under a fluid mask reports the fluid. This is a behavior test, not a grep gate.
- [ ] A pool with no gravity, inside a lower-priority room volume with low gravity: a point in both resolves the pool's fluid and the room's gravity. Two equal-priority volumes setting gravity resolve to the smaller one, whichever comes first in map order. Two identical equal-priority volumes resolve to the earlier one in map order. A point in no volume resolves level defaults.
- [ ] A large volume with higher priority beats a smaller overlapping volume with lower priority for the field both set.
- [ ] An entity of two brushes whose summed volume exceeds one equal-priority single-brush entity loses to it, even where each of its brushes is smaller.
- [ ] A level with zero volumes builds an empty BVH, and every point, capsule and ray query on it returns level defaults, no fluid and no crossing (`research.md` P9).
- [ ] Over a seeded random set of overlapping and nested volumes, every point, capsule and ray query through the BVH returns the same result as a brute-force scan of all leaves. This includes queries on leaf faces and at shared AABB boundaries.
- [ ] Immersion reads 0 with feet at the fluid top, 1 with the head below it, and a proportional value between. In a fluid block floating in mid-air, a capsule poking out the bottom reads the contained fraction only. A capsule wading below its body point, feet in a shallow pool, resolves the pool's fluid. A capsule spanning two fluids resolves the winner of the overlap rule, and its immersion counts only the winner's brushes.
- [ ] A capsule whose side overlaps a fluid while its vertical segment does not resolves dry.
- [ ] The environment volumes and their BVH are built before the level parity digest is set and before the first game tick, and installing a second level leaves none of the first level's volumes resolvable (`research.md` P8).
**Compiler**
- [ ] `gravity "-3"`, `"0"` and `"4"` compile. `gravity "0 0 -3"` fails the build and names the entity. `push "0 0 15"`, `"8 0 0"` and a tilted vector compile. A `push` of one, two or four numbers, or a non-number, fails the build and names the entity.
- [ ] A fluid volume or a gravity volume spanning a doorway leaves the cell count, portal count, static collision triangles and navmesh identical to the same map without it.
- [ ] Every PRL section other than EnvironmentVolumes and SH, lightmap and SDF included, is byte-identical to the map without the volume, and the EnvironmentVolumes section carries that volume's record.
- [ ] A fluid brush face flush against solid world, and a face shared by two brushes of one fluid entity, emit no surface. Every other face of the brush does.
- [ ] A fluid face partly flush against solid world emits only its exposed part.
- [ ] A `fluid_volume` with a blank `fluid`, or a `gravity_volume` with a blank `gravity`, fails the build and names the entity.
**Movement**
- [ ] Wading in until immersion passes `enterDepth` enters swim. Rising until it falls below `exitDepth` leaves it. Bobbing between the two thresholds never toggles state. Immersion exactly at `enterDepth` enters.
- [ ] Immersion exactly at `exitDepth` stays in swim.
- [ ] Two fluids differing only in viscosity: a swimmer with no input loses speed faster in the more viscous one. Wading below `enterDepth`, the same two fluids leave walking speed identical.
- [ ] Swimming with the eye above the surface in open water, far from any solid, jump sets upward velocity to `fluidJumpVelocity` and leaves swim. Fully submerged, jump gives no boost.
- [ ] After a fluid jump with immersion still at or above `enterDepth` on the next tick, swim does not re-enter until immersion first falls below `exitDepth` (`research.md` P10).
- [ ] On a tick where immersion falls below `exitDepth` and jump is pressed with the eye out, the fluid jump applies.
- [ ] With no vertical input, a swimmer drifts down at `sinkSpeed` under standard gravity, at half that under half gravity, not at all in a zero-gravity pool, and upward in an upward-gravity pool. A swimmer in a fluid with a coincident push volume drifts with the current.
- [ ] A crouching, dashing or sliding player entering deep enough water enters swim, standing-size capsule.
- [ ] A player whose descriptor has no `swim` block walks the floor of a pool, and loading a level with fluid volumes for that player warns once.
- [ ] The fluid jump's eye test uses the tick eye from the resolve, never the camera eye. With view feel moving the camera eye across the surface while the tick eye stays below it, jump gives no boost (`research.md` P5).
- [ ] Swimming down out of the bottom of a floating fluid block leaves swim, and level gravity takes over.
- [ ] Jumping up into the bottom of a floating fluid block enters swim once immersion reaches `enterDepth`.
- [ ] In a volume with low gravity the player falls at that gravity. With zero gravity, vertical velocity holds. On the first tick after leaving the volume, level gravity applies. An AI agent in the same volume falls at the volume's gravity.
- [ ] A grounded player entering a push volume whose upward push exceeds gravity leaves the ground on that tick and rises; ground-stick never snaps it back. In a tall updraft its speed along the push stops at the terminal cap. An upward push weaker than gravity leaves the player grounded.
- [ ] In a sideways push volume a player standing still drifts with the wind and settles at a steady speed. Airborne with movement input held, the push is not clipped by the air speed cap.
- [ ] A slide in a push volume gets no slope boost from the push.
- [ ] An AI agent in a push volume is unaffected and stays on the navmesh; in a low-gravity volume it falls at the volume's gravity.
- [ ] A map whose `initialGravity` is upward behaves as before: a grounded player stays grounded until it jumps.
- [ ] A particle in a push volume accelerates by the push regardless of its buoyancy.
- [ ] On the tick the player crosses out of a gravity volume, it still integrates the volume's gravity; on the next tick it integrates level gravity (`research.md` P1). The same holds for an AI agent.
- [ ] A particle inside a low-gravity volume accelerates at that volume's gravity times its buoyancy. On the particle step it drifts out, it takes level gravity. A zero-buoyancy particle floats in every volume.
- [ ] On the step a particle's position crosses a volume boundary, that step's velocity update uses the gravity at its post-step position (`research.md` P2).
- [ ] `setGravity` changes gravity outside volumes and leaves a gravity-setting volume's value in force inside it. A map with only `initialGravity` behaves as before. A PRL built before this change fails to load with a recompile message.
- [ ] Two `setGravity` calls on one tick leave the later value in force, and a gravity-setting volume's value holds inside it throughout.
**Co-op**
- [ ] In the predict/reconcile harness, a client crossing into a fluid and into a low-gravity volume produces zero reconciliation corrections, including when a correction's replay window spans both crossings.
- [ ] The harness also covers a reconcile whose acked baseline is `Swim` and whose replay exits swim, and one whose acked baseline is `Normal` and whose replay enters swim; both reconcile with no correction (`research.md` P3, P4).
- [ ] Changing a volume's shape, priority, gravity or fluid name changes the level content digest. Changing the level default gravity, or a fluid's sounds, tint or opacity, does not.
- [ ] Swapping the map order of two volumes changes the digest. Changing a surface face's texture or UV does not. The digest binds every field of the volume record and names the surface faces and the header's default gravity as its skips. The "does not" half on fluid sounds, tint and opacity needs no test: those fields never reach the function.
- [ ] A client joining a host whose level gravity differs from the client's, by `initialGravity` or by a script change before the join, predicts with the host's value and has zero reconciliation corrections.
- [ ] A host `setGravity` mid-level reaches a joined client through the tuning payload, with at most one correction.
- [ ] The harness seeds host and client with different level gravity and different fluid tables, and installs the host's values on the client through the tuning-payload install path, not the shared descriptor table.
- [ ] A client whose manifest gives `water` a different tint predicts with the host's viscosity and shows its own tint. A client with no `water` declaration shows no tint and still predicts swim.
- [ ] After a host manifest reload that adds a fluid, a volume naming it is no longer dry on the host or on clients.
- [ ] A client whose manifest declares different `water` numbers predicts with the host's values. A client whose manifest lacks `water`, or declares its fluids in a different order, still predicts swim with the host's fluid and has zero corrections.
- [ ] A volume naming a fluid the host lacks is dry on the client too, even when the client's own manifest declares that fluid.
- [ ] On the host, a `fluid_volume` naming an undeclared fluid loads as dry and warns, naming the map entity index and the fluid.
- [ ] Adding, removing, reordering or retuning a fluid declaration leaves the mod compatibility digest unchanged, so a client whose fluids differ from the host's stays participating.
**Presentation and surface**
- [ ] A hitscan ray from air into a fluid emits exactly one splash, at the crossing. A ray from inside a fluid out through a face emits one. A ray through a floating block emits two. A ray blocked before reaching a fluid face emits none. A ray that never crosses one emits none.
- [ ] A ray that enters a pool and stops on the floor beneath it emits exactly one splash, at the top (`research.md` P6). A ray through a pool built from two brushes of one entity emits no splash at their shared face (`research.md` P7). Each splash reports the crossing point, the crossed face's outward normal and the fluid.
- [ ] A ray whose origin lies on a fluid face never splashes at that face. A ray crossing the shared faces of two adjacent fluid entities emits two splashes.
- [ ] The muffle low-pass sits on the world-sound bus. Engaging it leaves UI and music unfiltered and adds no per-voice filter. A toggle reversed mid-ramp turns back without a step. A grep gate shows the audio crate's public API names no kira type.
- [ ] Entry and exit sounds play once per swim transition, never while bobbing between thresholds, and never from a reconcile replay.
- [ ] With a mod's script-set screen tint active, entering a fluid composes the fluid tint over it, and leaving the fluid restores the mod's tint unchanged. A script write to `screen.*` while the eye is in a fluid takes effect and does not clear the fluid tint. With the eye in a fluid, every `screen.*` slot keeps the value the mod last wrote.
- [ ] An eye bobbing across the surface at content/dev's head-bob amplitude and frequency, centred on it, toggles the tint at most once over the whole bob. An eye oscillating across the surface past the hysteresis band at 5 Hz presents at most 3 tint flashes in any 1 s window, counted on presented-frame time as `rendering_pipeline.md` §7.8 counts them. With the limiter off, the bob still toggles at most once and each toggle still ramps.
- [ ] Two grep gates. The projectile flight code makes no environment-resolve call. No FGD key on `fluid_volume` or `gravity_volume` names a `movement.swim` field.
- [ ] The Scripting surface example runs as a `content/dev` fixture in TypeScript and Luau. A fluid with negative viscosity, or a swim block with `exitDepth` ≥ `enterDepth`, is rejected with a warning naming the field.
- [ ] A fluid with an out-of-range `viewTint`, `muffle` or `surfaceOpacity`, or a second `defineFluid` with an existing name, is rejected with a warning naming the field; the first declaration stands.
- [ ] The Scripting surface's fluids and swim block land in content/dev's existing manifest and player descriptor, in TypeScript and Luau, and content/dev keeps its mod id.
**Swim slots** (blocked until E16's deferred per-seat crossing spec lands)
- [ ] `player.swimming`, `player.immersion` and `player.fluid` read false, 0 and empty when dry, and true, the immersion fraction and `water` while swimming in `water`. With two players, each client's HUD reads its own player's values, and on the host a reaction reads each player's slots by owner. A crossing on `player.swimming` fires once per player on entry and once on exit, and its reaction can tell which player crossed.
### Manual
- [ ] Fluid faces read as warped and translucent from inside and outside, a floating block included. They do not z-fight with surrounding geometry. Smoke above a fluid, seen from above, is not dimmed by it, and fog composites over it. Smoke below a fluid surface, seen from above, draws over the water: the accepted draw-order limit, confirmed but not a defect. A dynamic light near a large fluid face lights it smoothly, with no per-vertex faceting, and the surface matches baked lighting on adjacent walls.
- [ ] The tint and muffle turn on when the eye enters a fluid and off when it leaves, and treading at the surface shows no visible flicker or audible pumping.
- [ ] A swim and fluid-jump playtest: the surface jump feels reliable, and ledges within reach are climbable with it.
- [ ] A low-gravity room playtest, including a pool and a floating fluid block inside the room.
- [ ] A co-op client swims and crosses gravity volumes with no visible corrections.
- [ ] Stress run, measure and report.
  - Fixture: `content/dev/maps/stress-env-volumes.map`, generated by `tools/gen_stress_map.py` before this brief (`research.md` §Stress pre-flight). It has 1,000 fluid and gravity volumes, some overlapping and some floating, plus 64 AI agents, 1 player pawn and 10,000 live particles moving through them. There is one player pawn because no in-process multi-pawn route exists. Per-body cost is linear, and the agents dominate the body count.
  - Pinned: the fixture, the owner's Mac, a release build with dev-tools, a fixed `--start-pose`, and `POSTRETRO_CPU_TIMING` windows after warmup. The start pose is the mixed measurement room, `--start-pose=114.60,2.44,-138.99,45,0`. The run's manifest declares both `water` and `sludge`, so no fixture volume loads dry. Batch headless runs reject CPU timing.
  - Metrics: environment-resolution CPU time for bodies per tick, as its own `SimStage`; for particles per frame, as a stage nested in `particle_sim`; hitscan fluid-crossing query time; BVH build time at load.
  - These are per-stage numbers, so they stay readable while the fixture's other costs are high. The pre-flight measured AI at about 7.4 ms per tick and particle sim at about 4.5 ms per frame on this map before any volume does anything (`research.md` §Stress pre-flight). Those costs belong to separate optimization work and do not gate this brief. Baseline: `stress-env-volumes-baseline.map`, the same generated map with zero volumes, measured on the same build.
  - Budget (proposed), gated on the windowed mean with the max reported: bodies under 0.25 ms per tick, summing the player and agent call sites; particle environment resolution under 0.5 ms per frame; BVH build under 2 ms, from a one-time load record. Results are recorded in the plan of record.

## Path
- Compiler: peel the brush in the `parse.rs` entity sweep, as for `trigger_volume`. `resolve_brush_region_bounds` gives the planes and AABB, and `build_brush_volumes_with_ids` gives textured faces for the surface mesh. `parse.rs` is past 7,000 lines: land the peel in a new `parse/` submodule. Culling buried and shared faces needs a coplanar-overlap test against solid brush faces and sibling fluid faces.
- Movement: call the resolve where `dispatch.rs` receives `gravity` today, and widen that parameter to the resolved environment. `crates/physics/src/movement/mod.rs` is past 6,000 lines. The swim intent is a new `intents/swim.rs`. Reconcile replay (`reconcile.rs`) threads one gravity value today; it passes the level default and resolves per replayed tick.
- `prl_loader.rs` and `sim/mod.rs` are past 800 lines. New loader and tick code land as submodules.
- Surface culling: follow how `KinematicMoverRenderCollector` culls movers by AABB against visible-cell bounds. The smoke pass in `smoke.rs` is the blended-pass template, but water draws before it. The `has_any_sheet` gate does not cover water.
- Chosen shape: a load-time BVH, generic over leaf payload. Rival: a flat list with an AABB prefilter (the fog-volume precedent). It is faster below roughly 50–100 leaves but degrades linearly with author ambition. Another rival is per-cell volume lists, baked like `FogCellMasks`. Rejected: it ties collision to the visibility partition, entities store no cell, and rays cross cells.
- Particles: `particle_sim::tick` takes the scalar `gravity` today. It gains per-particle resolution through the same pure resolve, gravity and push fields.
- Push: every gravity site in the intents is gated on airborne today. Lift-off follows the knockback launch in movement `tick`; push's terminal speed follows `knockback::clamp_fall_speed`; slide's projection is in `intents/slide.rs` and stays gravity-only (`research.md` §Gravity as a force).
- Readonly slots: one `EngineStateCatalogEntry` each in `engine_state_catalog.rs`, per owner through the prerequisite's engine-slot support; the TS and Luau types regenerate from the catalog.
- Tint and muffle hysteresis: the trigger reads the eye's signed depth in the winning fluid. The limiter's per-channel clamp is where the tint channel joins it.
- First slice: the crate, section and resolve, proven by a low-gravity room through host movement, prediction and the replay harness. It falsifies the riskiest assumption, that per-tick resolution keeps host and client identical, before any swim or rendering work.
- Landing order: the slot slice is last and waits on E16's deferred per-seat crossing spec. If E24 reaches it first, the build checkpoints there with every other row proven.

## Open questions
- The hysteresis margin and the tint ramp's duration — **delegated**: the executor picks values that pass the bob row and reports them in the plan of record.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Fluid volume entity | canonical environment-volume record | PRL record (fluid set) | n/a | n/a | `fluid_volume` (brush) |
| Gravity volume entity | canonical environment-volume record | PRL record (gravity set) | n/a | n/a | `gravity_volume` (brush) |
| Priority | `i32` field | PRL `i32` | n/a | n/a | `priority` (default 0) |
| Push volume entity | canonical environment-volume record | PRL record (push set) | n/a | n/a | `push_volume` (brush) |
| Gravity override | `Option<f32>` | PRL presence flag + `f32` | n/a | n/a | `gravity` on `gravity_volume`, required; one signed vertical number, m/s² |
| Push | `Option<Vec3>` | PRL presence flag + 3×`f32` | n/a | n/a | `push` on `push_volume`, required; a vector in map axes, m/s², converted by the format adapter |
| Source entity | `u32` | PRL `u32` | n/a | n/a | the map entity's index, for diagnostics |
| Level gravity (sent) | `f32` | tuning payload (`TUNING_PAYLOAD_EPOCH` bump) | n/a | n/a | n/a |
| Fluid reference | resolved fluid index | PRL string | n/a | n/a | `fluid` on `fluid_volume`, required |
| Fluid registry | manifest field | tuning payload, movement fields only | `fluids` / `defineFluid` | `fluids` / `defineFluid` | n/a |
| Fluid fields | descriptor struct | viscosity in the tuning payload; presentation fields local | `name`, `viscosity`, `viewTint`, `muffle`, `surfaceOpacity`, `enterSound`, `exitSound` | same | n/a |
| Swim tuning | `PlayerMovementDescriptor` swim block | tuning payload | `movement.swim` (`speed`, `accel`, `sinkSpeed`, `enterDepth`, `exitDepth`, `fluidJumpVelocity`) | same | n/a |
| Swim state | `MovementStateKind::Swim` | movement-state discriminant (wire version bump) | n/a | n/a | n/a |
| Environment read | per-owner engine-state catalog slots (prerequisite spec) | owner-private | `player.swimming`, `player.immersion`, `player.fluid` | same | n/a |

## Wire format
**EnvironmentVolumes**: a new PRL section, next free id after `CellResidencySet` (51). Little-endian throughout. Mirrors TriggerVolumes (id 44): a leading `u16` version, `u32` counts, `u32` byte-length-prefixed UTF-8 strings.
- Header: `u16` version (1), then `f32` level default gravity.
- Then a `u32` volume count. Per volume:
  - `u32` source map entity index;
  - `i32` priority;
  - `u8` gravity present (0 or 1), then `f32` gravity, always written and zero when absent;
  - `u8` push present (0 or 1), then `f32×3` push, always written and zero when absent;
  - a string fluid name, empty = dry;
  - `u32` brush count, then per brush: `f32×3` AABB min, `f32×3` AABB max, `u32` plane count, then per plane `f32×3` normal and `f32` distance (inside when `dot(p, n) − d ≤ 0`);
  - `u32` surface face count, then per face: a `u32` `texture_index` (the level texture table, as KinematicGeometry face meta), a `u32` vertex count, and per vertex `f32×3` position and `f32×2` UV. A dry volume has zero faces.
- An empty list is a zero count. The section is always present, so a level with no volumes still carries its default gravity.

**FogVolumes** drops `initial_gravity` and bumps its version. Old PRLs fail to load with a recompile message, as the missing-gravity path does today.

The movement-state wire discriminant gains `Swim`, and the wire version bumps. The tuning payload gains level gravity (`f32`) and the host's fluid table (name and viscosity per fluid, in declaration order), and `TUNING_PAYLOAD_EPOCH` bumps.
