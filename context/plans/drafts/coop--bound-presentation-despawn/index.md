# coop--bound-presentation-despawn

Brief · compact · depends on `coop--networked-by-default` (Tasks 5, 7, 9, 10) · reads: `context/lib/networking.md` §Data path, `context/lib/scripting.md` §10.1, §12, `context/lib/entity_model.md` §Destruction · read at 2fc8abc43

## Problem
Modder-raised capability, anchored on a co-op level with a "kill the power" lever. Pulling it should remove the emergency strobe (a map `billboard_emitter`), a hanging lamp (a light-only descriptor placement), and a sparking-wire emitter. Two gaps stand between the modder and that level. First, no authoring surface removes a light, emitter, or light-only/emitter-only placement at runtime. `despawnEntity` left with the live VM, and impact-policy `despawn` reaches only damageable targets, which the parent spec already networks. Second, once removal exists, a host trigger's removal stays host-local. These entities are client-local spawns each peer builds from the shared map. A tombstone cannot reach a client that joins later: the host pre-acks active tombstones for a joiner and prunes them once acked, and the joiner's own install spawned the entity. When done, a removal from a host-only reaction takes the entity away on every client, a late joiner included. A removal from an every-machine source stays local to each machine. Players stop arguing over whether the power is off.

## Decisions
- **Builds on the parent.** Identity is the parent's `(kind, index, origin)` binding (its Decision 5, Task 7), checked the parent's way (Decision 6). Capture sits at the parent's reaction-dispatch chokepoint (Task 9), and client apply runs through its bindings (Task 10). This brief adds no identity scheme and no second capture path.
- **Origin rule reused unchanged.** A removal is captured only under a host-only origin, per the parent's *Drain origins* table. Removals from levelLoad, crossings, movement, or weapon events record nothing and stay local. Every machine runs those itself.
- **Engine mechanism, author policy.** Removal is an engine verb with replication built in. Whether a level removes or dims is the author's call. Placement: host capture and the removal table in netcode beside the parent's override table; client removal in `client/bindings`; the verb in the SDK member wrappers and its primitive in sim.
- **Non-goals.** Networked descriptor entities (health, mesh, touchable, behavior) need nothing here: their despawn already tombstones the display copy (parent AC 1). The same holds for descriptor lights and emitters riding such an entity. `prop_mesh` (a mesh without provenance) has no removal need in the user story; its classification belongs to the parent. Per-particle control stays out: in-flight particles finish their lifetime (`entity_model.md`, particles).

### Scripting surface
Proposed, pending open questions 1–3. Shape follows the existing member verbs (`mover.stop()`, `light.fade()`).

```typescript
import { defineReaction, getMapEntities } from "postretro";

const doomed = [
  ...getMapEntities("emitter", { tag: "power_strobe" }),
  ...getMapEntities("light", { tag: "power_lamp" }),
  ...getMapEntities("emitter", { tag: "power_sparks" }),
];
const killPower = defineReaction("power.kill", {
  sequence: doomed.flatMap((m) => m.remove()),
});

export function setupLevel() {
  return {
    reactions: [killPower],
    triggerEvents: getMapEntities("trigger", { tag: "power_lever" }).map((t) => t.on("enter", [killPower])),
  };
}
```

`remove(): SequenceStep[]` on light and emitter members, mirrored in Luau as `m:remove()`. It removes the whole entity.

## Acceptance
### Automated
**Host-only removal reaches everyone**
- [ ] The scripting-surface example, run as a `content/dev` fixture in TS and Luau, removes all three entities on the host. Each connected client removes its local copy within one snapshot.
- [ ] A client that joins after the lever was pulled never holds any of the three once it has converged within the parent's baseline-budget bound (parent AC 13).
- [ ] A `remove()` after a trigger's `wait` replicates the same as one before it.
- [ ] A removal on a host-only drain with no subject (an enemy-death reaction) replicates.

**Every-machine removal stays local**
- [ ] A `remove()` from levelLoad, a crossing, or a movement or weapon event records nothing on the host and sends nothing to any client. A client's own crossing-driven removal leaves the host and other clients unchanged.
- [ ] In single player, `remove()` removes locally and records nothing.

**Ordering and idempotence**
- [ ] An override followed by a removal in one tick sends only the removal. No later state record resurrects the entity on any peer.
- [ ] A removal followed by a later light or emitter step on the same member applies nothing on any peer and logs nothing above debug.
- [ ] Two removals of one entity, or a resent removal record (lost ack, re-baseline), remove once on the client, warn nothing, and touch no other entity.
- [ ] A removal record whose binding fails the parent's mismatch check warns once and removes nothing.
- [ ] A join with no removals sends zero removal records.
- [ ] A level change or restart clears removals on host and clients; the reloaded level shows every entity.

**Client bookkeeping**
- [ ] After a client removes a light-only descriptor placement, its runtime dynamic-light slot is free for reuse, and no forward light record for it remains in the next upload.
- [ ] After a client removes an emitter, it spawns no new particles; particles already alive finish their lifetime.
- [ ] `remove()` on a kind outside the chosen set (open question 2) is rejected at install in both runtimes, naming the reaction.

### Manual
- [ ] Mac↔Windows on the fixture map: pulling the lever removes the strobe, lamp, and sparks on both screens. A client joining afterward sees them gone.

## Path
- **Host.** Extend the parent's `PresentationOverrides` capture with a removal entry. Capture the binding key and origin *before* the entity despawns: the entity has no components left to serialize afterward, and host serialize walks only the live replicable set. Despawn through the end-of-frame removal pass (`mark_for_end_of_frame_removal` → `run_end_of_frame_removal_pass`), per `entity_model.md` §Destruction. Check what its `on_removed` callback reports for a health-less entity.
- **Wire.** The removal must stay a live record for the level's life, not a tombstone. `ServerReplication::ingest_tick` makes a tombstone only for a registered entity that vanished. `register_client` pre-acks active tombstones for a joiner, and `prune_acked_tombstones` drops them. Candidate: a removed marker on the parent's `WirePresentationBinding` record, or a state payload. The parent sends no tombstone for a bound entity (its Task 10: nothing despawns one there); a tombstone is not the removal channel here either.
- **Client.** In `client/bindings`, resolve via the parent's resolver, despawn the local entity, and keep the mapping, so later records for that `NetworkId` no-op and a re-baseline does not warn. The bridges already self-heal on a missing entity: `LightBridge::reclaim_missing_runtime_slots` and the `update` missing branch, `EmitterBridge::purge_stale`, and the fog zero-density placeholder (research.md).
- **Build side.** `script-compiler/src/light_membership.rs` mirrors member verbs for build-time evaluation, so `remove()` needs a stub there. A light `remove()` may also need to count as an animated-slot demand, the same way a `fade` does, if slot-bearing static lights are in the chosen set.
- **Strongest rival.** E18's shared-slot channel gives late-join removal with no net code once `remove()` exists. It is rejected for consistency with the parent, which rejected that ceremony for every other presentation change (research.md).
- **First slice.** Host-only `remove()` on one light-only descriptor placement, with a late joiner, in the parent's end-to-end harness. This falsifies the persistent-record assumption first.
- **Split first.** `light_bridge.rs` is past 800 lines but needs no edit unless the slot-reclaim AC fails. The parent's Task 1 splits cover the netcode and wire files.

## Open questions
1. Is a removal verb wanted, or is "turn off" the idiom? Turn off means `rate: 0` plus a light `fade` to zero, which the parent already replicates; this brief would then close as a docs note. **Recommend: build `remove()`.** "Off" keeps a runtime light slot and emitter state alive, and a later reaction can turn it back on by accident. Authors reading "power is cut" expect the entity gone. — owner — **blocks build**
2. Which kinds does `remove()` accept? **Recommend:** light-only and emitter-only descriptor placements, map `billboard_emitter`, and dynamic PRL lights. Reject fog (removal looks the same as `setFogDensity(0)`) and static PRL lights. A slotless light's baked contribution cannot be removed. Whether a slot-bearing light's baked base excludes it is unverified (research.md). — owner — **blocks build**
3. Does a light member's `remove()` remove the whole entity or only its light component, when the entity also carries an emitter? **Recommend: the whole entity, documented.** "Remove" names the thing in the map, and component stripping opens a partial-entity state on both peers. — owner — **blocks build**
4. How is removal persisted for late joiners? **Recommend:** a bound record that stays live, marked removed, until level change. It rides the parent's binding, baseline budget, and late-join rails. Rivals: a join-time removal list message, or the E18 shared slot (Path). — owner — **blocks build**
5. Should this land as a follow-on with its own `WIRE_VERSION` bump, or fold into the parent's 26 → 27 bump? **Recommend: follow-on, own bump.** The parent calls bumps cheap, and folding grows a spec already under review. Fold only if the parent is still unbuilt at promotion and the owner prefers one bump. — owner — **blocks build**
