# coop--bound-presentation-despawn — research

Read at `2fc8abc43` (branch `coop/networked-by-default`). Evidence behind the brief; the brief carries conclusions.

## No shipped surface removes a client-local presentation entity

| Candidate | Finding |
|---|---|
| `despawnEntity` script primitive | Deleted with the live VM (`plans/done/remove-live-vm`): an imperative tick-API primitive, not a reasoned rejection of reaction-level removal. `primitives/mod.rs` test asserts it stays absent. |
| Impact-policy `despawn` | `bind_effect` (`impact_policy.rs`) requires target `@impact.target`. `ImpactDispatch` is pushed only by `apply_damage_with_context` (`entities/components/health.rs`), which returns early without `HealthComponent`. A light-only, emitter-only, or map `billboard_emitter` entity can never be an impact target. A descriptor with health *and* a light is networked under the parent (Decision 1), so its despawn already tombstones the display copy. |
| Reaction registry | Registered names (`fx/emitter_reactions`, `fx/fog_reactions`, `system_reactions.rs`, `mover_commands.rs`, `trigger_system.rs`, `spawner.rs`, `health/reactions.rs`, `grant`, `animation.rs`, `npc_state.rs`): none removes an entity. |
| Sequence primitives | Light (`lighting/script_primitives.rs`), fog, mover, trigger, spawner, `wait`, `fire`: none removes. |
| Member verbs (`scripting.md` §12 table; `postretro.d.ts`) | light: `pulse fade flicker colorShift sweep`; fog: `pulse fade flicker pulseSaturation fadeSaturation`; emitter: snapshot only, no verbs. |
| `mark_for_end_of_frame_removal` callers | Production: `impact_effects.rs` only (deferred `Despawn` and terminal despawn). Test callers in `observability/driver.rs`, `touch.rs`. |
| Other production `registry.despawn` | Projectiles (`weapon/mod.rs`, `projectile_stage.rs`), particles (`particle_sim.rs`), net display-copy release (`net_descriptor.rs`), mover batch rollback (`runtime_movers.rs`). None reachable from authoring for presentation entities. |
| World-item pickup | Strips `Touchable` and mesh; does not despawn. Not presentation. |

Conclusion: the user story is unauthorable today. Without a removal verb, the nearest idiom is "turn off" — `setEmitterRate` `rate: 0` (raw tag-keyed descriptor; `scripting.md` §10.1 says `rate = 0` is the inactive state) and a light `fade` to zero — which the parent spec already replicates.

## Spawn on each peer

- Map `billboard_emitter` and `prop_mesh`: built-in `apply_classname_dispatch` (`sim/scripting/builtins/mod.rs`); no `DescriptorProvenance`.
- Light-only / emitter-only descriptor placements: `apply_data_archetype_dispatch`; lights forced dynamic; enrolled by `LightBridge::absorb_dynamic_lights` into runtime slots.
- PRL lights: `LightBridge::populate_from_level_with_influences`, authored prefix of `entity_ids`. Fog: `FogVolumeBridge::populate_from_level`.
- Members: `getMapEntities("light")` → `worldQuery` `QueryFilter::Light`, filtered by `is_map_placed`. Whether descriptor placements pass `is_map_placed` was not traced.

## Removing on a client: bridge bookkeeping self-heals

| Kind | On missing entity |
|---|---|
| Runtime light (descriptor) | `LightBridge::reclaim_missing_runtime_slots` and the `update` missing-entity branch mark the slot reclaimed, push it to `free_slots`, drop the snapshot, set dirty. Reserve slot returns. |
| Authored dynamic PRL light | Repack keeps its compact forward slot zeroed (influence retained); not reclaimed. |
| Authored static light with animated compose slot | Repack writes a zeroed compose descriptor ("explicit compose tombstone"). Whether the baked base excludes this light's contribution was not verified. |
| Authored static slotless light | Contribution is baked into lightmap/SH. Despawning the entity cannot remove it. |
| Emitter | `EmitterBridge::purge_stale` drops per-emitter state. In-flight particles orphan and finish their lifetime at last spin (`entity_model.md`, particle section). |
| Fog volume | `FogVolumeBridge::update_volumes` emits a zero-density placeholder for a missing component; visually equal to `setFogDensity(0)`. |

Removal path convention: `entity_model.md` §Destruction routes scripted removal through the end-of-frame removal pass (`run_end_of_frame_removal_pass`), whose callback feeds progress and kill credit. A health-less entity reports `kill_credit: None`, `depleted: false`; whether a tagged presentation entity can sit in a `progress` set was not traced.

## Why a tombstone cannot carry removal

- `ServerReplication::ingest_tick` (`net/replication.rs`) makes a tombstone only for a `NetworkId` that was in the previous tick's tracked set and vanished. An entity never registered before its despawn produces nothing.
- `register_client` pre-acks every active tombstone for a joiner ("it never had mappings for those despawned entities"), and `prune_acked_tombstones` drops a tombstone once every current client acks. A late joiner therefore never sees a removal tombstone, and its own map install spawned the entity.
- Host serialize walks the live replicable set and borrows the registry (`networking.md` §Data path). A despawned entity has no components to serialize, so a persistent removal fact must come from a host-side table keyed by binding, not from a registry walk.
- Parent Task 10 rule ("a despawn tombstone for a bound `NetworkId` removes the mapping only; the local entity stays") is unreachable today: parent registrations hold until level change, and no surface despawns a bound entity on the host.

## Strongest rival

E18's shared-slot channel (`plans/done/E18--trigger-event-fanout`; `content/dev/scripts/closet-reveal.ts`): the host trigger writes a `network: "shared"` slot, and a client crossing runs `remove()` locally. Once a removal verb exists, this gives late-join correctness with no net code, because crossings are every-machine and a joiner's slot baseline fires its crossing. The parent rejected this channel as the default for presentation state (every change needs a slot and a crossing). A removal that needs that ceremony while a `fade` does not would be inconsistent.
