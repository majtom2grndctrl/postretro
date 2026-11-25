# E10--npc-weapon-mounts

Brief · resumable · owner signed off 2026-11-23 · Epic 10 · reads: `context/lib/entity_model.md` §2 (Weapon vocabulary, Weapon mounts), §7 (Skeletal hit zones), §7c (Sight sees movers, Clip-timed attacks) · `context/lib/resource_management.md` §7 (Sockets) · `context/lib/networking.md` §Weapon placement is content · depends on `E21--character-weapon-mounts` (done) · the pose-and-stagger brief later supplies its aiming classification (`research.md` §Handed to the pose brief) · read at dcd60bfb3

## Problem
Owner-observed defect (2026-11-23): NPC rifle shots launch from above the head (`research.md` §NPC shot origin). The cause: the AI fire step launches every NPC shot from the AI eye, the top of the hitbox, and an NPC has no mounted weapon to launch from instead: only pawns resolve E21's `mesh.weaponMounts`, and NPCs carry their rifle as a base attachment. When done, NPCs mount the weapons their attacks name through the character-owned table pawns use, renamed `mesh.mounts` so every item a character carries shares it, and an NPC's shot leaves the muzzle of the weapon it fires: the mounted gun's, or a body socket for a weapon built into the body. An NPC whose gun is blocked by cover close in front of it holds fire rather than shooting from its head or into that cover, and its behavior can see the held shots. It fires only while aiming, never mid-reaction. Obstacles farther along the shot's path, such as a column the player ducks behind, still take the shot. Co-op clients see the same mounts with no wire change. `drafts/E10--npc-equipment-sets/` builds on this.

## Decisions
- **One `mesh.mounts` table for every carried item.** E21's `mesh.weaponMounts` becomes `mesh.mounts`, each entry `{ item, attachments }` or `{ item, muzzleSocket }`; here `item` takes a weapon handle, and `drafts/E10--npc-equipment-sets/` extends it to gear. The owner chose one per-rig presentation table for everything a character carries or wears (weapons, gear, built-in weapons, later melee and dismemberable parts), for the reason the `muzzleSocket` entry has: no precedent that excludes melee. Pre-stable (`index.md`): `weaponMounts` and the entry key `weapon` retire with no alias, and every consumer migrates in this change.
- **NPCs mount through `mesh.mounts`; every attack weapon is carried.** An NPC's mounted list is its base `mesh.attachments`, then the entry of each weapon its attacks name, later winning per socket. This ends `entity_model.md` §2's "only pawns resolve the table". `drafts/E10--npc-equipment-sets/` later narrows what is carried.
- **Carried weapons share a socket only on one model.** Entries naming the same model on a socket mount it once: the limitator's burst and long rifles on one AR_4 `hand_r`. Different models on one socket reject load (Load rules): every attack weapon is carried, so one model would not draw while its shots left it.
- **No wire change.** Each peer composes an NPC's mounts from its own descriptors, as it composes a pawn's after E21. Nothing carried changes at runtime here, so no live update exists. Peers with differing descriptors present differently (`entity_model.md` §7d precedent). This replaces `done/E21--character-weapon-mounts/`'s line that NPC carried weapons stay host-only: every peer composes them. `drafts/E10--npc-equipment-sets/` replicates only what varies per instance.
- **Missing data is loud, as in E21.** On an NPC that declares a table, an attack weapon without an entry warns once per archetype and weapon at load and mounts nothing. E21's other table warnings apply unchanged. An NPC without a table mounts no weapon and logs nothing (E21's opt-out).
- **NPC shots leave the carried weapon's muzzle.** The launch point is the bake's `muzzle` socket where the posed attachment draws, and the shot aims from there at the target's aim point. This supersedes `done/E16--enemy-ranged-projectile/`'s first-cut `enemy_eye` origin, which that brief deferred as a presentation follow-on and this brief builds as a host gameplay query (`research.md` §Prior commitments touched). The eye stays the origin of sight, perception, target choice and the fire gate's line of sight.
- **Only cover near the gun holds a shot.** At the fire moment the host tests two near paths, world and movers at the tick's pose (`entity_model.md` §7c, Sight sees movers): the path from the body to the muzzle, and the path the shot will fly from the muzzle toward the target's aim point, at the projectile's radius, out to the near distance. Either blocked counts as a failed fire gate, a whiff wherever §7c makes a failed gate one, and the shot never launches from the eye. Past the near distance the shot flies as fired and hits whatever it meets.
  - The owner's reason: an NPC shooting the lip of its own cover reads as broken, while tricking enemies into wasting shots on columns or on each other is part of the fun of shooters.
  - The near distance is an engine constant, 2 m measured from the muzzle along the shot's path, not authored: an NPC's own cover sits within about 1 m of its muzzle, and the dev rifles engage at 12–20 m. With the target's aim point nearer, the path is tested to one shot radius short of it (owner ruling 2026-11-24: a surface the shot would only graze beside a point-blank aim point, such as a lintel over a player's eye, does not hold it). The owner ruled it floor under `scripting.md` §1: an NPC never shoots its own cover. No knob is exposed until a modder needs one.
  - Cost: an NPC whose eye sees over cover its gun cannot clear holds fire there. Its behavior sees the held shots (`attacksHeldInActivity`), and cover choice must clear the gun (Cover choice clears the gun, in the cover brief). The eye's height does not cure it: the gun sits below the eye wherever the eye is.
- **Each shot of a burst re-tests its near paths.** A later shot whose near paths are blocked launches nothing and the burst continues, so no shot enters the cover lip. The skipped shot still spends one shot of the weapon's ammo, heat or cell and restarts the attack's cooldown, as if it fired, and the next shot re-tests. The owner's reason is the one above: wasted shots are part of the fun. The owner ruled the spend floor under `scripting.md` §1, like the near distance: cover never changes a weapon's rate, its cooldown or what it spends.
- **The fire gate tests what perception does not.** `done/E10--enemy-line-of-sight-cover/` made one debounced eye verdict that perception and the fire gate share. It stays shared: `targetVisible` remains the eye's sight, and the fire gate still reads it. The near-path test is a second test on the fire tick that only the fire gate reads, so `targetVisible` can be true while every shot is held.
- **`attacksHeldInActivity` reports held shots.** A scope-relative guard fact beside `attacksFiredInActivity`: shots the near-path test held since the evaluated activity was entered, visible on the next tick's guard refresh. Each held shot counts once:
  - a clip moment that whiffs on a blocked near path;
  - a burst shot skipped on a blocked near path;
  - an attack with no clip moment, which includes a clip-timed attack whose clip cannot time its moment as `entity_model.md` §7c already treats it, when it becomes held, and again at each activity entry while still held (pin NM-7), not on each tick it stays held, so the count does not grow with tick rate (`entity_model.md` §7c, Elapsed time).

  An eye-gate failure, a missing muzzle, a shot fired while not aiming (Fire only while aiming) and an obstacle past the near distance count nothing. It is host-only, like its sibling, with no wire change. A peek can leave on a fired or a held shot.
- **Cover choice clears the gun, in the cover brief.** The shipped combat-slot scorer keeps only spots from which the eye sees the target (`research.md` §Fire decision), so it can park an NPC where its eye sees and its gun is held. The owner accepts that gap here, with `attacksHeldInActivity` as the backstop, and hands it to `drafts/E10--enemy-cover/`: there a `half` point and a combat slot qualify only when the standing gun's near path to the target is clear as well as the eye's sight. That draft owns how it estimates the gun at a spot the NPC has not reached, and proves the estimate against the fire tick's posed muzzle.
- **A missing muzzle fires from the eye.** A carried weapon with no muzzle to read launches its shot from the eye. It has none when:
  - its mounted bake lacks a `muzzle` socket, or its entry's `muzzleSocket` names a socket the body model lacks: an authoring error, warned once per archetype and weapon at load;
  - it has no entry: only the missing-entry warning fires;
  - the fire tick's drawn clip does not resolve, or the clip clock tracks no state because the activity draws no animation: an authoring gap with no pose to read, and no new warning.

  Missing is not blocked: a shot with no muzzle has no near path to test, so only the eye gate holds it. `drafts/E10--npc-equipment-sets/` adds a muzzle model displaced by gear. Accepted residue: these shots still launch from the eye, above the head, as today.
- **Fire only while aiming.** At the launch an enemy is in one of three states, read from explicit state, never by comparing state names:
  - aiming: the shot leaves the muzzle posed on the state the clip clock tracks. The brain's own draw landing this tick, an activity entry or a stride or travel-variant change, makes it aiming; a pending redraw or the airborne clip holds through the tick that redraws it, and the next tick fires;
  - not aiming: it fires nothing, from the eye or the muzzle; a due burst shot is skipped and spends as a blocked one does;
  - no pose: a mesh with no animation block, or an activity that draws no animation, fires from the eye as a missing muzzle.

  The pose-and-stagger brief supplies the classification and decides which interruptions count as aiming. This brief lands first, under an interim rule: an enemy is not aiming while a switch the brain did not make stays drawn, until the brain's next own draw; while a knockback's airborne clip shows; and while a redraw is pending. Under a flinch route, today's stagger applies unchanged. A failed own switch is no pose until the pose brief decides. Accepted cost of the interim rule: in an activity with no flinch route and no own draw due, such as a planted shooter, a reaction silences the enemy until its activity changes, and behavior sees no held shot, so only a timer row leaves; no shipped content triggers it. Aiming reads the muzzle on the tracked state's pose; a stagger kind that keeps firing while drawing its own clip needs another source, which the pose brief adds. The owner's rule: an enemy fires only from a stance where it is aiming.
- **A built-in weapon's muzzle is a body socket.** An entry `{ item, muzzleSocket: "<body socket>" }` names no `attachments`; its shots launch from that posed socket on the body model. The owner chose this shape because it avoids a precedent mutually exclusive with melee weapons, which later briefs expand on.
- **The muzzle pose reads game-tick clip time.** The host poses the state the clip clock tracks at the brain's clip time, the clock clip-timed attacks use (`entity_model.md` §7c), never the render clock, so the launch point is deterministic. The muzzle pose joins §7c's one exception to a single animation clock, so the drawn gun may lead or trail the launch point by a frame, as it does a clip moment. The pose omits presentation-only modifiers, as gameplay probes do (`entity_model.md` §7). Foreclosure: a later presentation-only modifier on an NPC's arms, such as an aim bend or recoil, would move the drawn gun off the launch point, so that work decides whether its modifier joins this pose.
- **Sockets enter one gameplay query: the host NPC shot.** Its origin and its near-path gate read the mount. This reverses E21's "Gameplay never reads a mount" (`networking.md` §Weapon placement is content) and `resource_management.md` §7's "sockets never participate in gameplay queries" for NPC shots. Only the host computes an NPC's shot and no peer reproduces it, so peers need not agree on the pose for gameplay; hit zones and collision still ignore attachments. Gap: a client draws the host's launch point against its own pose of the gun, so on a client a shot leaves the drawn barrel only as closely as its clip phase tracks the host's. That agreement is unverified (Manual).
- **The actor-kind launch branch stays.** Pawns launch from placement plus `muzzleOffset` and fall back to the eye when the muzzle is obstructed; NPCs launch from the mount and never fall back from an obstructed one. `entity_model.md` §2 admits no branch on actor kind; this one is admitted in launch origin only, because a pawn's origin must be reproducible by client prediction from replicated placement (`networking.md`) and an NPC's is host-only. The fallback splits with it: a pawn's eye is its camera, while falling back to an NPC's eye would restore the defect. The weapon descriptor stays one type.
- **Dev content is the first consumer.** Each AR_4 rig bake gains a rigid `muzzle` socket at its barrel tip. The limitator and the rifle squads move from `mesh.attachments` to `mesh.mounts`, with entries for both rifles on their own rig bake. exo_red's table moves to `mesh.mounts` unchanged in content. The first consumer of `{ item, muzzleSocket }` is `cannon_brute`, on `models/mutant_arm_cannon/`: the mutant with its right hand replaced by a cannon rigidly skinned to `RightForeArm`, and a `cannon_tip` skin joint at the bore. Its arm-cannon weapon and attack are new `content/dev` content.
- **Load rules (both runtimes, twin behavior).** Each rejects load, naming the character:
  - an entry naming both `attachments` and `muzzleSocket`, or neither. An attachment map that is present and empty still rejects (E21);
  - two attack weapons whose entries put different models on one socket. The rejection also names the socket.

  The entry keys are `item`, `attachments` and `muzzleSocket`; any other, `weapon` included, rejects (E21). `weaponMounts` is an unknown `mesh` key and rejects under E21's rule.
- **Non-goals.**
  - The eye's derivation from the hitbox (`research.md` design smell 1): it stays the origin of sight and the eye gate. Authoring it is separate work, and it does not cure a held shot (Only cover near the gun holds a shot).
  - Off-hand IK onto a weapon grip socket. Mounts anchor at one socket, and two-handed clips place the off hand. A weapon model may already carry a `grip_l` socket, so nothing here blocks later IK.
  - The observer's third-person muzzle for pawns, deferred by `networking.md`.
  - Non-weapon items in `mounts`: `drafts/E10--npc-equipment-sets/` adds gear.
  - One weapon for players and NPCs: NPCs keep `enemy_rifle*`. `drafts/descriptor-instance-overrides/` owns sharing a weapon with per-use tuning.
  - Consolidating two weapons that share one model into one modal weapon: `drafts/E16--persistent-firing-modes/` owns it and builds next.
  - Contact attacks under a reaction: the interim rule gates launched shots only, and a contact strike behaves as today. The pose-and-stagger brief decides melee.
  - Who owns a living enemy's pose and what a foreign switch does: the default flinch, its end and redraw, stagger kinds, revival, lasting script poses, and the final aiming classification, which replaces the interim rule. The pose-and-stagger brief owns them (`research.md` §Handed to the pose brief).
- **Docs amended at promotion:**
  - `entity_model.md` §2 Components, Weapon vocabulary and Weapon mounts; §7c Clip-timed attacks and Sight sees movers: each takes the Decisions above that change its contract.
  - `scripting.md` §11: `attacksHeldInActivity` and the `targetVisible` contract. §10.3: under the interim rule, a reaction on a living ranged enemy holds its fire until the brain's next own draw.
  - `resource_management.md` §7 Sockets: `mesh.mounts`; the NPC shot exception to presentation-only; mounted while carried.
  - `networking.md` §Weapon placement is content: `mesh.mounts`; an NPC's fire origin and near-path gate read its mount on the host; peers compose NPC mounts locally.
  - `index.md`: the third-person weapon mount router line names `mesh.mounts`.
  - Human-facing: `docs/weapon-mounts.md` (`mesh.mounts` and `item`; NPCs mount through it; a rig bake carries a `muzzle` socket, and moving it changes where the NPC's shots start and which cover blocks them; body muzzles), `docs/scripting-reference.md` (a reaction on a living ranged enemy holds its fire), `docs/character-animation.md`, and the SDK types in both runtimes, including the `targetVisible` doc comment and the new fact's.

### Scripting surface
Owner reviews this surface personally before promotion.
```ts
import { defineEntity } from "postretro";
import { enemyRifleBurstEntity as burstRifle, enemyRifleLongEntity as longRifle } from "./enemy-rifle";
import { armCannonEntity as armCannon } from "./arm-cannon";

export const limitator = defineEntity({
  canonicalName: "limitator",
  components: {
    mesh: {
      model: "models/limitator/model.gltf",
      mounts: [
        { item: burstRifle, attachments: { hand_r: "models/ar_4/limitator.gltf" } },
        { item: longRifle, attachments: { hand_r: "models/ar_4/limitator.gltf" } },
      ],
      // animations… as today
    },
    // health, behavior… as today; its attacks name both rifles.
    // A row that leaves once the activity fired, or its gun was held by near cover:
    //   { to: "settle", when: brain.attacksFiredInActivity.ge(1).or(brain.attacksHeldInActivity.ge(1)) }
  },
});

// A weapon built into the body: `cannon_tip` is a skin joint on the model, not a mounted bake.
export const cannonBrute = defineEntity({
  canonicalName: "cannon_brute",
  components: {
    mesh: {
      model: "models/mutant_arm_cannon/model.gltf",
      mounts: [{ item: armCannon, muzzleSocket: "cannon_tip" }],
      // animations… from the mutant's `mixamo_brute` clips
    },
    // health, behavior with an attack on armCannon…
  },
});
```

### Boundary inventory
Both runtimes ship; the Luau example mirrors the TypeScript one key for key.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| mount table | mesh descriptor field, renamed from E21's | `"mounts"` inside `"mesh"`: list of `{item, attachments}` or `{item, muzzleSocket}` | `components.mesh.mounts` | `components.mesh.mounts` | n/a |
| mount entry type | entry descriptor | n/a | `MountDescriptor` | `MountDescriptor` | n/a |
| entry item | weapon canonical name | name string | weapon `defineEntity` value | weapon `defineEntity` value | n/a |
| built-in muzzle | optional body socket on a mount entry | `"muzzleSocket"` inside a `mounts` entry | `muzzleSocket` | `muzzleSocket` | n/a |
| held-shot fact | scope-relative brain count per nesting depth, beside `attacks_fired_in_activity`; host-only | brain field with a serde default, as its sibling; no wire | `brain.attacksHeldInActivity` | `brain.attacksHeldInActivity` | n/a |
| bake muzzle | socket named `muzzle` on a mount bake | glTF node `extras.socket = "muzzle"` | n/a | n/a | n/a |
| retired names | E21's table field and entry key removed | `"weaponMounts"` rejected as an unknown `mesh` key; `"weapon"` rejected as an unknown entry key | `weaponMounts`, `WeaponMountDescriptor` removed | same | n/a |

## Acceptance
### Automated
**Rename**
- [ ] `mesh.weaponMounts` on any descriptor rejects load in both runtimes as an unknown `mesh` key, naming the key and the character. A `mounts` entry keyed `weapon` rejects as an unknown entry key. The same tables written as `mesh.mounts` with `item` load.
- [ ] A pawn mounts from `mesh.mounts` as it did from `weaponMounts`: E21's resolution, missing-data and load-rule rows hold under the new names (regression guard).
- [ ] No file under `content/`, `sdk/`, `docs/` or `context/lib/` names `weaponMounts` or `WeaponMountDescriptor`, and under `crates/` only the tests proving the old key's rejection name `weaponMounts` (a grep gate).

**NPC mounts**
- [ ] The limitator and each rifle squad mount their own rig's AR_4 bake once on `hand_r`, from `mesh.mounts`, on the host and on a connected client. No `content/` NPC mounts a weapon through `mesh.attachments`: no loaded content descriptor's base `mesh.attachments` names a bake under `models/ar_4/`.
- [ ] An NPC mounts the entry of each weapon its attacks name, over its base attachments, later winning per socket. An entry for a weapon no attack names mounts nothing and logs nothing.
- [ ] An archetype whose attack weapons' entries put different models on one socket rejects load in both runtimes, naming the archetype and the socket. The same entries on different sockets, or naming one model on the shared socket, load.
- [ ] On an NPC with a table, an attack weapon without an entry warns once per level load at install, whether or not the archetype spawns, naming the archetype and the weapon, mounts nothing, and launches from the eye with no other warning; no spawn warns again. An NPC without a table mounts no weapon, launches from the eye and logs nothing.
- [ ] An NPC entry whose socket its rig lacks, or whose model the level did not load, warns once per level load as a pawn's does, draws nothing on that socket, and launches that weapon's shots from the eye with no further warning (pin NM-3).
- [ ] No wire record gains a field (a diff gate on the wire record types). A connected client's NPC mounts match the host's, composed from the client's own descriptors.

**Shot origin**
- [ ] An NPC's shot launches from its fired weapon's posed bake `muzzle`, and the origin moves with the clip pose. For the limitator and each rifle squad, the origin sits at the rifle's barrel tip, below the head top. Line of sight, perception and target choice read the eye, not the muzzle.
- [ ] Shots from the limitator's burst and long rifles both launch from the one shared bake's `muzzle`.
- [ ] An entry naming `muzzleSocket` and no `attachments`, as on the built-in-weapon fixture, loads in both runtimes and mounts nothing; its shots launch from that body socket and move with the clip pose.
- [ ] The launch point is a function of the fire tick: two runs of one fire tick under different frame splits and dev time scales launch from the same point, which matches the bake's `muzzle` posed at the brain's clip time for the tracked state.
- [ ] On a fire tick where the NPC turns and enters a new clip, its near-path tests and its shot start from one point: the bake's `muzzle` posed on that tick's clip and clip time, at that tick's facing (pin NM-1).
- [ ] A pawn's fire origin is unchanged, and still identical with and without a `mounts` table on its descriptor. A pawn whose muzzle is obstructed still launches from its eye.
- [ ] Hit tests against an NPC are identical with and without its `mounts` table: a mounted bake takes no hit and blocks nothing.

**Near-path gate, both sides**
- [ ] A cover lip within the near distance, below the muzzle-to-target line but within the shot's radius of it: the shot whiffs, and no projectile launches. With the line raised clear of the lip by more than the radius, the shot launches from the muzzle.
- [ ] A barrel tip pushed through a wall, or through a closed door, that the eye's path to the target does not cross: the shot whiffs, and no projectile launches. With the NPC moved back so the tip clears the wall or door, or with the door opened, the shot launches from the muzzle. A closed door within the near distance that crosses the muzzle's path but not the eye's blocks the shot as world geometry does.
- [ ] An obstacle on the muzzle-to-target path just inside the near distance whiffs the shot; the same obstacle moved just past it lets the shot launch from the muzzle and strike it. With the target's aim point nearer than the near distance, an obstacle anywhere on the path, up to one shot radius short of the aim point, whiffs the shot.
- [ ] A column the target ducks behind mid-burst, past the near distance: the burst's later shots launch from the muzzle and strike the column, and none is skipped, both while the eye gate still passes and after the column blocks the eye's view past its loss grace.
- [ ] An attack with no clip moment whose near path is blocked fires nothing on that tick, and fires from the muzzle on the first tick the path clears and its other gates pass.
- [ ] A burst whose later shot meets a blocked near path skips that shot, launches nothing into the cover, and spends one shot of the weapon's ammo, heat or cell, as a fired shot does; each next shot re-tests its paths, and the shots whose paths are clear fire, while each skipped shot spent its own. A burst whose paths stay clear fires every shot from the muzzle.
- [ ] A burst whose last shot is skipped, for a blocked near path or because the enemy is not aiming, starts its next burst on the same tick as the same burst fired unobstructed: a skipped shot restarts the attack's cooldown as a fired shot does and adds nothing to `attacksFiredInActivity`, so a burst whose first shot fires and later shots are all skipped reads 1 (pin NM-11).
- [ ] The existing low-cover fire test (Path) keeps proving the eye gate with its contact attack, and gains a meshed projectile NPC on the same walls: a clip-timed shot behind the low wall near the NPC, whose lip sits above its gun and below its hitbox top with the eye's view clear, whiffs with no projectile from the eye or the muzzle; with a wall below both paths, or none, it fires from the muzzle at the target's aim point; the full-cover wall fails the eye gate as before.

**What the behavior sees**
- [ ] While near cover holds every shot, with the eye's view clear, `targetVisible` reads true and `attacksFiredInActivity` stays 0.
- [ ] `attacksHeldInActivity` counts one per whiffed clip moment and one per skipped burst shot on a blocked near path, and is visible on the next tick's guard refresh. It resets on the activity's entry, scope-relative per nesting depth as `attacksFiredInActivity` is.
- [ ] An attack with no clip moment held across many ticks counts once; it counts again only after it fires, or its path clears, and it is held again, or after its activity is entered again with the attack still held; an enclosing activity not entered again counts the hold once, and a script reaction drawn over the hold adds nothing once the brain's own stride or travel-variant change draws over it in the same activity (pins NM-7, NM-29). Two runs of one held span at different AI tick rates read the same count.
- [ ] An eye-gate failure, a missing muzzle that fires from the eye, a shot that strikes an obstacle past the near distance, a shot withheld while the enemy is not aiming, and a flinch that cancels a burst or closes a moment behind blocking cover each leave `attacksHeldInActivity` unchanged (pins NM-5, NM-8).
- [ ] A burst that outlasts its activity and skips a later shot on a blocked near path counts that shot in the activity entered, as a later fired shot counts in `attacksFiredInActivity` (pin NM-9).
- [ ] A dev behavior row guarded on `attacksFiredInActivity.ge(1).or(attacksHeldInActivity.ge(1))` leaves its activity after a held shot behind near cover, and after a fired shot in the open.
- [ ] The `targetVisible` SDK doc comment, in both runtimes, says it is the eye's sight and that the fire gate also tests the gun's near path; `attacksHeldInActivity` is typed and documented in both runtimes.

**Missing muzzle**
- [ ] A carried weapon whose mounted bake has no `muzzle` socket, or whose entry's `muzzleSocket` names a socket the body model lacks, launches its shots from the eye. The warning fires after the level's model sweep, once per level load, naming the archetype and the weapon, for each carried bake that lacks a `muzzle` socket and each loaded body model that lacks its entry's socket; a second spawn does not warn again, and an archetype whose body model the level never loaded logs no body-socket warning (pin NM-4).
- [ ] A fire tick whose drawn clip does not resolve, or whose activity draws no animation so the clip clock tracks none, launches from the eye and adds no warning.
- [ ] Behind the low cover that whiffs a muzzle shot (Near-path gate), the same NPC with its bake's `muzzle` socket removed fires from the eye: a missing muzzle runs no muzzle-path test.

**Fire only while aiming**
- [ ] A script reaction that switches the drawn state, in an activity with no flinch route, while an attack with no clip moment is ready: the enemy is not aiming, and no shot launches that tick or on any later tick the reaction stays drawn, from the eye or the muzzle; on the tick the brain's next own draw lands, it fires from the muzzle (pins NM-2, NM-5, NM-16). In an activity with a flinch route, today's stagger applies (pin NM-8).
- [ ] A held enemy's drop with its fresh mesh not yet redrawn: a due shot launches nothing and `attacksHeldInActivity` does not change, even when the mesh's default state is the tracked one; after the redraw it fires from the muzzle (pin NM-25).
- [ ] A dropped enemy given a script reaction before its redraw lands, in an activity with no flinch route: the redraw is dropped as today, nothing launches until the brain's next own draw, and the tick that draw lands fires from the muzzle (pin NM-31).
- [ ] A reaction that an earlier NPC's contact hit runs in the same AI tick, after this NPC's fire decision and before its launch, switches its drawn state: the enemy is not aiming at the launch, nothing launches, and a due burst shot is skipped and spends; with a flinch route, the next tick's flinch cancels the rest of the burst, as today (pin NM-6).
- [ ] An earlier NPC's contact hit runs a reaction that switches this NPC's drawn state in the same AI tick, after its fire decision, on a tick its own activity entry lands: the enemy is aiming, and the shot launches from the muzzle posed on the entered state; the same with a stride change and no flinch route; with a flinch route, the stride change leaves the reaction drawn, nothing launches, and a due burst shot is skipped and spends (pin NM-28).
- [ ] A burst attack whose fire decision forms before an earlier NPC's contact hit switches this NPC's drawn state in the same AI tick does not start: no shot of that burst launches or spends then or after the reaction ends, and an attack with no clip moment fires its whole burst from the muzzle on the first eligible tick after (pin NM-13).
- [ ] A reaction still drawn on a tick the brain's own draw lands, an activity entry or a stride or travel-variant change: the enemy is aiming, and the shot launches from the muzzle posed on the state settle tracks; a clip moment reached on that entry tick fires (pin NM-16).
- [ ] A reaction drawn over an activity whose state the next entry shares, the entry tick firing an attack with no clip moment: the shot launches from the muzzle posed at the start the entry's switch draws, not at the clip time the clock kept under the reaction; the same holds for any own draw that replaces a reaction onto the tracked state (pin NM-26).
- [ ] A knocked-back enemy drawing its airborne clip fires nothing, a due burst shot is skipped and spends, and an attack with no clip moment fires from the muzzle on the first tick after landing that its gates pass (pin NM-17).
- [ ] A knocked-back enemy whose airborne stretch has not passed the fall delay, or whose mesh has no usable `jump_loop`, fires from the muzzle posed on its tracked state while airborne; only ticks whose launch sees the airborne clip drawn are not aiming (pin NM-30).
- [ ] An own switch that fails with an unknown state, the mesh keeping the prior state: the enemy has no pose and fires from the eye, adding no warning beyond apply's own.
- [ ] An own switch that fails while a script reaction is drawn leaves the enemy not aiming: no shot launches from the eye or the muzzle until a later own draw lands (pin NM-32).
- [ ] The interim not-aiming record set by a switch the brain did not make clears on the brain's next own draw and on no other tick; it survives a save and restore, so a restored enemy fires on the tick an unsaved run does. Without a reaction, a burst fires every shot as before.
- [ ] A strafing NPC mid-burst whose stride stops or starts, or whose travel variant changes, on a due shot's tick fires that shot from the muzzle posed on the newly tracked state; no due shot is skipped for the brain's own switch (pin NM-10).

**Load rules**
- [ ] An entry naming both `attachments` and `muzzleSocket`, or neither, rejects load in both runtimes, naming the character. An entry with only one loads.
- [ ] Each AR_4 rig bake carries a rigid `muzzle` socket, and each still reports `VERIFIED: mount check passed.` under `solve-weapon-mount --check` on its rig.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` fixture in both runtimes and lowers to the same descriptor.
### Manual
- [ ] On a dev map, the limitator's and each rifle trooper's shots visibly leave the gun's barrel, not the space above the head, through every clip they fire in.
- [ ] On a dev map with chest-high cover, a rifle trooper behind it never fires into the cover's lip or from above its head.
- [ ] Two-client co-op at 150 ms RTT (`POSTRETRO_NET_LAG`): the client sees each rifle NPC's gun seated in hand, as the host does.
- [ ] The same session: on the client, each rifle NPC's shots appear to leave its drawn barrel, not the air beside it, through every clip they fire in. Record the largest visible offset if any.

## Path
- Seams:
  - Shot origin: the fire proposal sets `WeaponAim.origin` from `perception.enemy_eye` (`compute.rs`), and `AiWeaponControllers::advance_actor` launches each due shot at that tick's aim. The missing-muzzle eye shot keeps that origin (`research.md` §NPC shot origin).
  - Near-path gate: split the obstruction sweep out of `resolve_muzzle_launch_origin` (`crates/sim/src/weapon/mod.rs`) for a caller that holds a world muzzle. The muzzle-to-target leg sweeps at the radius, clipped to the near distance, beside the moment check in the fire proposal; the body leg runs from the eye, as pawns run it, or from the holder joint; `advance_actor` re-runs both per due burst shot (`research.md` §Blocked muzzle).
  - Shared verdict and held fact: `fire_gate_projects_the_shared_perception_verdict` (`perception.rs`) keeps its proof; the held count follows `attacks_fired_in_activity` through `ScopeRelativeValues`; `targetVisible`'s comment comes from the typedef templates (`research.md` §Fire decision).
  - Eye low-cover test: `enemy_eye_height_clears_low_cover_but_full_cover_blocks_fire` (`ai_tests.rs`); its meshed projectile twin proves the near-path and missing-muzzle rows.
  - Muzzle pose and eligibility: compose the holder joint, the rig bake and the bake's `muzzle` on `ClipClock` time; `HitZoneStore::posed_socket_world` samples on the animation clock and composes no attachment socket. The launch reads the three-state contract. Its interim classifier is a host-only record beside `redraw` and the airborne launch clip, serialized with the clip clock: a foreign switch sets it, seen by `begin_tick` or by apply before this enemy's launch, and the brain's next own draw clears it; the switch count alone marks only the switch tick. The launch reads whether apply's own draw lands this tick (apply's switch predicate, its flinch-route suppression included, evaluated before `advance_actor`), and the record clears when that draw lands. The pose-and-stagger brief replaces the classifier (`research.md` §Muzzle pose, §Fire decision). The clip clock's `PartialEq` is written field by field, so the new record joins it or the round-trip test cannot see it.
  - Composition: `descriptor_mesh_component` (`data_archetype.rs`) builds an NPC's mesh on host attach and client materialize from its `descriptor:` class; E21's `compose_mounted_attachments` (`crates/netcode/src/weapon_mounts.rs`) composes one entry, for pawns.
  - Entry shape: `WeaponMountDescriptor`, `validate_weapon_mounts`, `WEAPON_MOUNT_KEYS` (`crates/entities/src/data_descriptors/types/weapon_mounts.rs`); `attachments` becomes optional. SDK lowering: `lowerWeaponMountReferences` (`sdk/lib/data_script.ts`, `.luau`).
  - Missing entry and missing muzzle: `check_weapon_mount_tables` runs before the model sweep in `install_world_cpu`, so the missing-muzzle check runs after the sweep, against loaded models only (pin NM-4).
  - Bakes: `content/dev/models/ar_4/`; the raw source lives outside the repo, so the socket node goes into each bake. `prop_to_gltf.py --socket` and the cyberpunk viewmodels' `muzzle` nodes are the precedent (`docs/weapon-mounts.md`).
  - Rename consumers: every symbol, call site and test file is in `research.md` §Rename consumers.
  - Built-in fixture: `content/dev/models/mutant_arm_cannon/`; its `license.txt` records how it was built from the mutant. `cannon_tip` has no clip tracks, so it rides the forearm. `socket_dump` (`crates/model/examples/`) resolves it under `mixamo_brute` clips and shows it moving with `idle` and `punch` (`research.md` §Built-in fixture).
- Art is gameplay now: re-baking an AR_4 or moving its `muzzle` node changes where an NPC's shot starts and which cover blocks it. `solve-weapon-mount --check` proves orientation only, so a bake edit re-runs the shot-origin and near-path rows.
- Shape chosen: the carried weapon's mounted muzzle, posed on the game-tick clip clock, gated by the paths its shot needs clear.
- Strongest rival: an authored chest or shoulder launch offset in the facing frame. The owner rejected it: it leaves the shot off the drawn gun. Runner-up: the bake's `muzzle` posed once at load per archetype and attack state, kept as a facing-relative offset. For a clip moment it is the drawn muzzle, and it drops pins NM-1 and NM-2, but bursts and shots with no clip moment fire at any point of a looping clip, whose pose one offset cannot follow, so their shots would leave beside the gun by an unmeasured amount; the owner kept the clip-time pose. For the interim aiming rule, the rival was to keep firing from the eye under a reaction until the pose brief lands: it restores the defect on every reaction, so the owner chose not aiming. Others: `research.md` §NPC shot origin, §Blocked muzzle, §Fire decision, §Validation history input.
- First slice: muzzle launch for the limitator in single player, with its bake's `muzzle` socket, before the other bakes and client work. It tests the riskiest premise: that the host can pose the tracked state at the brain's clip time at the AI fire step, and compose the bake's `muzzle` on the posed `hand_r`. The rename lands first, in its own behavior-preserving commit.
- Files past ~800 lines this extends: `compute.rs`, `hit_zones.rs`, `data_archetype.rs`, `ai_tests.rs`. Split a cohesive block out first where this adds one, in its own behavior-preserving commit.
- Derivation and verified facts: `research.md`.

## Open questions
- The fixture's attack clip: `mixamo_brute` has no aim or shoot clip. Recommended: `punch`, with the shot's clip moment at the strike, if the cannon then points at the target; otherwise another brute clip that does. — **delegated**
- A `muzzleSocket` on a player pawn's entry: pawns launch from placement plus `muzzleOffset`. Recommended: it validates, mounts nothing, and is ignored for fire origin. — **delegated**
- An entry mounting several models, more than one carrying a `muzzle` socket. Recommended: warn once at load and launch from the first in socket-name order. — **delegated**
- Whether the near-path test shares the eye verdict's loss grace. Recommended: no; it is a raw test on the fire tick, since a grace would launch a shot into cover. — **delegated**
