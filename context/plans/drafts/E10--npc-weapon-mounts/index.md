# E10--npc-weapon-mounts

Brief · compact · draft, owner rulings of 2026-11-23 applied; owner API review pending · Epic 10 · reads: `context/lib/entity_model.md` §2 (Weapon vocabulary, Weapon mounts), §7 (Skeletal hit zones), §7c (Clip-timed attacks) · `context/lib/resource_management.md` §7 (Sockets) · `context/lib/networking.md` §Weapon placement is content · depends on `E21--character-weapon-mounts` (done) · read at 73b072de3

## Problem
Owner-observed defect (2026-11-23): NPC rifle shots launch from the top of the hitbox, above the head (`research.md` §NPC shot origin). The cause: the AI fire step launches every NPC shot from the AI eye, the top of the hitbox, and an NPC has no mounted weapon to launch from instead: only pawns resolve E21's `mesh.weaponMounts`, and NPCs carry their rifle as a base attachment. When done, NPCs mount the weapons their attacks name through the character-owned table pawns use, and an NPC's shot leaves the muzzle of the weapon it fires: the mounted gun's, or a body socket for a weapon built into the body. Co-op clients see the same mounts with no wire change. `drafts/E10--npc-equipment-sets/` builds on this.

## Decisions
- **NPCs mount through `mesh.weaponMounts`; every attack weapon is carried.** An NPC's mounted list is its base `mesh.attachments`, then the entry of each weapon its attacks name, later winning per socket: E21's composition over several weapons. This ends `entity_model.md` §2's "only pawns resolve the table". `drafts/E10--npc-equipment-sets/` later narrows what is carried.
- **Carried weapons share a socket only on one model.** Entries naming the same model on a socket mount it once: the limitator's burst and long rifles on one AR_4 `hand_r`. Different models on one socket reject load (Load rules).
- **No wire change.** Each peer composes an NPC's mounts from its own descriptors and the NPC's `descriptor:` class, as each peer composes a pawn's from `active_weapon_archetype` after E21. Nothing carried changes at runtime here, so no live update exists. Peers with differing descriptors present differently (`entity_model.md` §7d precedent).
- **Missing data is loud, as in E21.** On an NPC that declares a table, an attack weapon without an entry warns once per archetype and weapon at load and mounts nothing. E21's other table warnings apply unchanged. An NPC without a table mounts no weapon and logs nothing (E21's opt-out).
- **NPC shots leave the carried weapon's muzzle.** The launch point is the rig's holder joint × the rig bake × the bake's rigid `muzzle` socket. It passes through the obstruction fallback and convergence players use (`resolve_muzzle_launch_origin`), with the AI eye as the aim origin, and aims at the target from there. The eye stays the line-of-sight and fire-gate origin.
- **A built-in weapon's muzzle is a body socket.** An entry `{ weapon, muzzle: "<body socket>" }` names no `attachments`; its shots launch from that posed socket on the body model. The owner chose this shape because it avoids a precedent mutually exclusive with melee weapons, which later briefs expand on.
- **The muzzle pose reads game-tick clip time.** The host poses the drawn state at the brain's clip time, the clock clip-timed attacks use (`entity_model.md` §7c), never the render clock, so the launch point is deterministic. The muzzle pose joins §7c's one exception to a single animation clock, so the drawn gun may lead or trail the launch point by a frame, as it does a clip moment.
- **Sockets enter one gameplay query: host NPC shot origin.** This reverses E21's "Gameplay never reads a mount" (`networking.md` §Weapon placement is content) and `resource_management.md` §7's "sockets never participate in gameplay queries" for NPC shots. Only the host computes an NPC's shot and no peer reproduces it, so peers need not agree on the pose; hit zones and collision still ignore attachments.
- **The actor-kind launch branch stays.** Pawns launch from placement plus `muzzleOffset`; NPCs from the mount. `entity_model.md` §2 admits no branch on actor kind; this one is admitted in launch origin only, because a pawn's origin must be reproducible by client prediction from replicated placement (`networking.md`) and an NPC's is host-only. The weapon descriptor stays one type, and the obstruction and convergence rule is shared.
- **Dev content is the first consumer.** Each AR_4 rig bake gains a rigid `muzzle` socket at its barrel tip. The limitator and the rifle squads move from `mesh.attachments` to `mesh.weaponMounts`, with entries for both rifles on their own rig bake.
- **Load rules (both runtimes, twin behavior).** Each rejects load, naming the character:
  - an entry naming both `attachments` and `muzzle`, or neither. An attachment map that is present and empty still rejects (E21);
  - two attack weapons whose entries put different models on one socket. The rejection also names the socket.

  `muzzle` joins the entry keys; any other key still rejects (E21).
- **Non-goals.**
  - The eye's derivation from the hitbox (`research.md` design smell 1): it stays the line-of-sight origin, and authoring it is separate work.
  - Off-hand IK onto a weapon grip socket. Mounts anchor at one socket, and two-handed clips place the off hand. A weapon model may already carry a `grip_l` socket, so nothing here blocks later IK.
  - The observer's third-person muzzle for pawns, deferred by `networking.md`.
  - One weapon for players and NPCs: NPCs keep `enemy_rifle*`. `drafts/descriptor-instance-overrides/` owns sharing a weapon with per-use tuning.
  - Consolidating two weapons that share one model into one modal weapon: `drafts/E16--persistent-firing-modes/` owns it.
  - Dismembering a built-in weapon, the body part that carries its muzzle: a later brief.
- **Docs amended at promotion:**
  - `entity_model.md` §2: Weapon vocabulary (the launch-origin branch, and why); Weapon mounts (NPCs resolve the table with every attack weapon carried; carried weapons share a socket only on one model; the `muzzle` entry; the attachments-or-muzzle rule; an NPC shot reads the mount). §7c: the muzzle pose joins the clip-time exception.
  - `resource_management.md` §7 Sockets: the NPC shot-origin exception to presentation-only; mounted while carried.
  - `networking.md` §Weapon placement is content: an NPC's fire origin reads its mount on the host; peers compose NPC mounts locally.
  - Human-facing: `docs/weapon-mounts.md` (NPCs mount through `weaponMounts`; a rig bake carries a `muzzle` socket; body muzzles), `docs/scripting-reference.md`, and the SDK types in both runtimes.

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
      // Moved from `attachments`. Both rifles show one AR_4 bake, so they share `hand_r`;
      // shots from either leave the bake's `muzzle` socket.
      weaponMounts: [
        { weapon: burstRifle, attachments: { hand_r: "models/ar_4/limitator.gltf" } },
        { weapon: longRifle, attachments: { hand_r: "models/ar_4/limitator.gltf" } },
      ],
      // animations… as today
    },
    // health, behavior… as today; its attacks name both rifles
  },
});

// A weapon built into the body: no prop; its shots leave a socket on the body model.
export const sentry = defineEntity({
  canonicalName: "sentry",
  components: {
    mesh: {
      model: "models/sentry/model.gltf",
      weaponMounts: [{ weapon: armCannon, muzzle: "cannon_tip" }], // `attachments` or `muzzle`, never both
      // animations…
    },
    // health, behavior with an attack on armCannon…
  },
});
```

### Boundary inventory
Both runtimes ship.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| weapon mounts | defined by `E21--character-weapon-mounts` | as defined there | as defined there | as defined there | n/a |
| built-in muzzle | optional body socket on a weapon-mount entry | `"muzzle"` inside a `weaponMounts` entry | `muzzle` | `muzzle` | n/a |
| bake muzzle | socket named `muzzle` on a mount bake | glTF node `extras.socket = "muzzle"` | n/a | n/a | n/a |

## Acceptance
### Automated
**NPC mounts**
- [ ] The limitator and each rifle squad mount their own rig's AR_4 bake once on `hand_r`, from `mesh.weaponMounts`, on the host and on a connected client. No `content/` NPC mounts a weapon through `mesh.attachments`.
- [ ] An NPC mounts the entry of each weapon its attacks name, over its base attachments, later winning per socket. An entry for a weapon no attack names mounts nothing and logs nothing.
- [ ] An archetype whose attack weapons' entries put different models on one socket rejects load in both runtimes, naming the archetype and the socket. The same entries on different sockets, or naming one model on the shared socket, load.
- [ ] On an NPC with a table, an attack weapon without an entry warns once per level load, naming the archetype and the weapon, and mounts nothing; a second spawn of that archetype does not warn again. An NPC without a table mounts no weapon and logs nothing.
- [ ] No wire record gains a field. A connected client's NPC mounts match the host's, composed from the client's own descriptors.

**Shot origin**
- [ ] An NPC's shot launches from its fired weapon's posed bake `muzzle`, and the origin moves with the clip pose. For the limitator and each rifle squad, the origin sits at the rifle's barrel tip, below the head top. Line of sight and the fire gate read the eye, not the muzzle.
- [ ] Shots from the limitator's burst and long rifles both launch from the one shared bake's `muzzle`.
- [ ] An entry naming `muzzle` and no `attachments` loads in both runtimes and mounts nothing; its shots launch from that body socket and move with the clip pose.
- [ ] A muzzle behind world geometry, with the eye's view clear, launches the shot from the eye, as for a player. An unobstructed muzzle launches from the muzzle and converges on the target.
- [ ] The launch point is a function of the fire tick: two runs of one fire tick under different frame splits and dev time scales launch from the same point, which matches the bake's `muzzle` posed at the brain's clip time for the drawn state.
- [ ] A pawn's fire origin is unchanged, and still identical with and without a `weaponMounts` table on its descriptor.
- [ ] Hit tests against an NPC are identical with and without its `weaponMounts` table: a mounted bake takes no hit and blocks nothing.

**Load rules**
- [ ] An entry naming both `attachments` and `muzzle`, or neither, rejects load in both runtimes, naming the character. An entry with only one loads.
- [ ] Each AR_4 rig bake carries a rigid `muzzle` socket, and each still reports `VERIFIED: mount check passed.` under `solve-weapon-mount --check` on its rig.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` fixture in both runtimes and lowers to the same descriptor.
### Manual
- [ ] On a dev map, the limitator's and each rifle trooper's shots visibly leave the gun's barrel, not the space above the head, through every clip they fire in.
- [ ] Two-client co-op at 150 ms RTT (`POSTRETRO_NET_LAG`): the client sees each rifle NPC's gun seated in hand, as the host does.

## Path
- Seams:
  - Shot origin: the fire proposal sets `WeaponAim.origin` from `perception.enemy_eye` (`crates/ai/src/compute.rs`), and `AiWeaponControllers::advance_actor` launches at it. `resolve_muzzle_launch_origin` (`crates/sim/src/weapon/mod.rs`) is private to sim, which the AI crate depends on, and composes its muzzle from placement. An NPC caller holds a world muzzle point, so the obstruction test splits from that composition.
  - Muzzle pose: `HitZoneStore::posed_socket_world` samples a body socket on the animation clock through `sample_world_pose_for_probe`, and does not compose an attachment's own socket. The clip clock is `ClipClock` (`state`, `time`), advanced in `clip_clock::begin_tick` for every brain. Gameplay probes omit presentation-only pose modifiers (`entity_model.md` §7). `research.md` §Muzzle pose.
  - Composition: `descriptor_mesh_component` (`data_archetype.rs`) builds an NPC's mesh on host attach and on client materialize (`materialize_net_descriptor_presentation`), so one seam serves both peers. E21's `compose_mounted_attachments` (`crates/netcode/src/weapon_mounts.rs`) composes base plus one entry, for pawns.
  - Entry shape: `WeaponMountDescriptor`, `validate_weapon_mounts`, `WEAPON_MOUNT_KEYS` / `check_weapon_mount_key` (`crates/entities/src/data_descriptors/types/weapon_mounts.rs`); `attachments` becomes optional. SDK lowering: `lowerWeaponMountReferences` (`sdk/lib/data_script.ts`, `.luau`).
  - Missing entry: `check_weapon_mount_tables` (`crates/sim/src/scripting_systems/weapon_mounts.rs`) already walks NPC tables at install.
  - Preload: `weapon_presentation_models` already sweeps every descriptor's table, NPCs included.
  - Bakes: `content/dev/models/ar_4/`; the raw source lives outside the repo, so the socket node goes into each bake. `prop_to_gltf.py --socket` and the cyberpunk viewmodels' `muzzle` nodes are the precedent (`docs/weapon-mounts.md`).
- Shape chosen: the carried weapon's mounted muzzle, through the shared obstruction rule, posed on the game-tick clip clock.
- Strongest rival: an authored chest or shoulder launch offset in the facing frame. Rejected by the owner: it leaves the shot off the drawn gun. Others: `research.md` §NPC shot origin.
- First slice: muzzle launch for the limitator in single player, with its bake's `muzzle` socket, before the other bakes and client work. It tests the riskiest premise: that the host can pose the drawn state at the brain's clip time at the AI fire step, and compose the bake's `muzzle` on the posed `hand_r`.
- Files past ~800 lines this extends: `compute.rs`, `weapon/mod.rs`, `hit_zones.rs`, `data_archetype.rs`. Split a cohesive block out first where this adds one, in its own behavior-preserving commit.
- Derivation and verified facts: `research.md`.

## Open questions
- A fired weapon with no muzzle to launch from: its mounted bake lacks a `muzzle` socket, its entry's `muzzle` names a socket the body model lacks, or it has no entry. Recommended: launch from the eye; warn once per archetype and weapon at load, except where the missing-entry warning already fired. — **delegated**
- A fire tick on which the drawn clip does not resolve, or the clip clock tracks no state. Recommended: launch from the eye, silently. — **delegated**
- A `muzzle` on a player pawn's entry: pawns launch from placement plus `muzzleOffset`. Recommended: it validates, mounts nothing, and is ignored for fire origin. — **delegated**
- An entry mounting several models, more than one carrying a `muzzle` socket. Recommended: warn once at load and launch from the first in socket-name order. — **delegated**
