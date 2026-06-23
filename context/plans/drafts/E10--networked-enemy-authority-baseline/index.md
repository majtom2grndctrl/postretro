# E10 -- Networked Enemy Authority Baseline

## Goal

Make the shipped enemy foundation obey the Epic 15 host/client model before steering-feel work adds more agent state. A host owns enemy AI, steering, damage, death, and despawn. Connected clients do not simulate those enemies; they receive descriptor-backed remote enemy presentation entities from server snapshots.

This is a bridge spec, not a steering spec. It makes existing enemies visible and lifecycle-correct in co-op so `E10--enemy-steering-feel`, `E10--enemy-stuck-recovery`, `E10--enemy-locomotion-animation`, and `E10--enemy-combat-positioning` can stay server-authoritative and focused.

## Scope

### In scope

- Host registration for map-placed descriptor enemies that carry `Brain` + `Agent`.
- Connected-client level-load policy that skips local authoritative enemy spawns for those descriptor placements.
- Descriptor-backed client materialization for remote enemy presentation. First version attaches render-facing descriptor data, not client-authoritative AI.
- Snapshot metadata that can carry descriptor class for non-player replicated entities.
- Remote enemy transform interpolation through the existing client interpolation buffer.
- Server-authoritative enemy death/despawn replication.
- Tests that prove no duplicate client enemy exists: no static map-spawned local enemy plus moving replicated enemy.
- Manual loopback check with one host and one connected client on a map containing the reference enemy.

### Out of scope

- Client prediction for enemies. Enemies are remote entities on every client.
- Replicating `BrainComponent`, `AgentComponent`, or `HealthComponent` as full wire payloads.
- Replicating enemy HP UI, damage numbers, or combat-event facts. State-slot and combat-event specs own that.
- Animation-state replication. The first remote enemy may render in descriptor default animation while its transform moves. `E10--enemy-locomotion-animation` may derive locomotion from replicated transform velocity or add a focused animation-state payload.
- Enemy steering feel, stuck recovery, combat positioning, or pathfinding changes.
- General remote descriptor materialization for every gameplay archetype. This spec targets map-placed AI enemies only.
- FGD, PRL, TypeScript, or Luau surface changes.

## Acceptance criteria

- [ ] On a listen host, each map-placed descriptor enemy carrying AI is registered as a replicated authoritative entity and receives a stable `NetworkId`; non-AI static descriptor props remain unregistered unless an existing net path registers them.
- [ ] A connected client loading the same level does not spawn local authoritative copies of map-placed AI enemies during level install. Those enemies appear only when the host sends full baselines.
- [ ] A remote enemy full baseline carrying a descriptor class materializes a client entity with `Transform` plus the descriptor's mesh presentation data, so the mesh renderer can draw it. It does not materialize client-authoritative `Brain`, `Agent`, `Health`, `Weapon`, or `PlayerMovement` components.
- [ ] Remote enemy snapshots carry finite `Transform` payloads and feed the existing remote interpolation buffer. At 150 ms RTT + 5% loss + jitter, the client sees enemy movement as interpolated remote motion, not local simulation.
- [ ] Server-authoritative enemy despawn removes the remote client entity and its interpolation buffer state. A late-joining client receives the current live enemy set only.
- [ ] A host-side enemy that attacks or dies remains authoritative: the client does not run AI ticks, agent steering, weapon fire, or death sweep for remote enemies.
- [ ] Snapshot wire validation accepts `entity_class` metadata on descriptor-backed non-movement full baselines/deltas and still rejects malformed metadata on despawn records. Old wire versions are rejected by the existing version gate.
- [ ] Existing movement prediction/reconciliation tests remain green: `local_player` and movement-authority metadata remain valid only for records carrying `PlayerMovementState`.
- [ ] Manual loopback check on a reference-enemy dev map: host and connected client both see one moving enemy, not a duplicate; killing the enemy on the host removes it from the client.

## Tasks

### Task 1: Split remote descriptor materialization helpers

Move descriptor materialization code that netcode needs out of the oversized `scripting/builtins/data_archetype.rs` into a focused helper module. Keep `spawn_net_slot_pawn` and `materialize_net_local_movement_component` behavior-identical. Add a new presentation-only helper that, given an `entity_class`, descriptor table, registry, and existing entity id, attaches only descriptor presentation components needed for a remote enemy mesh. It must be idempotent and must not attach `Brain`, `Agent`, `Health`, `Weapon`, or `PlayerMovement`.

### Task 2: Split net snapshot apply/metadata glue

Before extending `netcode/client.rs`, `netcode/mod.rs`, and `crates/net/src/wire.rs`, extract small focused modules for descriptor-class metadata and remote presentation materialization call sites. Preserve current player `entity_class` behavior exactly. This is a behavior-preserving split plus call-site routing, so the later wire change does not deepen the large files.

### Task 3: Wire descriptor class for remote enemies

Bump `SNAPSHOT_VERSION`. Keep the existing `RawEntityRecord.has_entity_class` + `entity_class` fields, but change validation semantics: descriptor class is valid on non-despawn entity records that carry a `Transform`. Movement-authority metadata remains valid only with `PlayerMovementState`. Despawn records still reject every metadata flag and non-empty class string. Update raw-to-typed validation tests, drift guards, malformed-envelope tests, and snapshot encode/decode round trips.

### Task 4: Host enemy registration

After descriptor map placements materialize on a host, register map-placed entities carrying both `Brain` and `Agent` in `ReplicableSet` and stamp their `NetworkId`. Registration is host-only and reload-safe: stale enemy ids from the previous level are unregistered or ignored before new level entities are registered. Snapshot production stamps `entity_class` from `DescriptorProvenance` for those map-placed enemies, not only for `NetworkSlot` movement pawns.

### Task 5: Connected-client spawn suppression

Extend level install so a connected client skips descriptor map placements that would create authoritative AI enemies. The predicate is descriptor-data based, not classname-list based: a descriptor with an `ai` block is skipped on connected clients. Static props, FX, lights, and other non-AI descriptor placements keep materializing locally as today. Add tests proving connected clients skip AI descriptors while single-player and host installs keep them.

### Task 6: Client remote enemy materialization

When client snapshot apply spawns an unmapped non-local entity from a full baseline that carries `entity_class`, call the presentation-only materializer before the frame renders. Keep local-player arming on its current movement path. On deltas and re-baselines, materialization is idempotent and does not reset mesh animation state. Unknown descriptor class logs and leaves the entity transform-only rather than rejecting the snapshot.

### Task 7: Integration tests and loopback check

Add focused unit tests for host registration, client spawn suppression, non-movement `entity_class` validation, remote presentation materialization, despawn cleanup, and no-duplicate enemy state. Add or extend an in-memory net harness that drives host enemy transform replication to a connected client registry. Finish with a manual loopback recipe on a reference-enemy map under the Phase 2/3 conditioned link profile.

## Sequencing

**Phase 1 (sequential):** Task 1, Task 2 -- split large files before adding behavior.

**Phase 2 (sequential):** Task 3 -- changes the wire contract consumed by host production and client apply.

**Phase 3 (concurrent):** Task 4, Task 5 -- host registration and connected-client spawn suppression use separate seams but must both exist before integration.

**Phase 4 (sequential):** Task 6 -- consumes Task 1's materializer and Task 3's non-movement descriptor metadata.

**Phase 5 (sequential):** Task 7 -- verifies the whole host-to-client enemy path after all behavior lands.

## Rough sketch

- Existing wire fields already carry descriptor class: `RawEntityRecord.has_entity_class` and `entity_class`. Today `RawEntityRecord::validate_entity_class` rejects class metadata unless the record carries `PlayerMovementState`. Relax only this rule. Keep `validate_movement_metadata` unchanged.
- Existing wire component payloads stay limited to `Transform` and `PlayerMovementState`. Enemy movement rides as `Transform` only. That is enough for remote interpolation and avoids full `Agent` / `Brain` payload design.
- Add a host-side helper near replication ownership, not in the net crate. Candidate shape: collect `DescriptorProvenance` entities with `spawn_path == MapPlacement` and both `ComponentKind::Brain` and `ComponentKind::Agent`, stamp/register them in the host `ReplicableSet`, and keep a small host tracking set for reload cleanup.
- Generalize `movement_entity_class` in `netcode/replication.rs` into descriptor-class extraction. It still returns `"player"` for `NetworkSlot` movement pawns. It also returns descriptor canonical names for registered map-placed AI enemies.
- Client apply currently spawns an unmapped full baseline from its first finite `Transform`, maps `NetworkId -> EntityId`, and applies payloads. Thread the typed record's `entity_class` into that spawn path, or record a post-apply materialization request in `ApplyOutcome` so `client_receive_and_apply` can call the descriptor helper where descriptor tables are already in scope.
- Presentation materialization should attach `MeshComponent` from the descriptor, including animation state declarations and default state. Do not attach `BrainComponent` or `AgentComponent`: connected clients skip full sim and must not carry hidden authoritative AI state.
- Connected-client spawn suppression belongs beside the existing boot-player suppression in level install. Pass the role/client flag into descriptor map dispatch or filter `map_entities` before dispatch. Avoid classname special cases; use the resolved descriptor's `ai` presence.
- Remote interpolation already samples all remote mapped entities. A transform-only remote enemy should follow the same path as the Phase 2 demo mover once it is registered and mapped.
- Oversized-file watch: `netcode/mod.rs`, `netcode/client.rs`, `crates/net/src/wire.rs`, `startup/lifecycle.rs`, `scripting/builtins/data_archetype.rs`, and `scripting/registry.rs` are all over the split smell threshold. Prefer focused helper modules and small call-site edits.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Descriptor class metadata | `EntityRecord::entity_class` / `RawEntityRecord.entity_class` | `entity_class: String` with `has_entity_class: bool`; valid on non-despawn `Transform` records | n/a | n/a | existing `entity_class` only for player spawns; no new KVP |
| Enemy replicated pose | `ComponentPayload::Transform` | `component_kind = 0`, `WireTransform` | n/a | n/a | n/a |
| Player movement authority | `ComponentPayload::PlayerMovementState` | `component_kind = 6`; only records with this payload may carry `local_player` or `last_processed_client_tick` | n/a | n/a | n/a |
| Remote enemy presentation | descriptor `mesh` block -> client `MeshComponent` | descriptor class names only; mesh data stays shared local content | existing descriptor data, no shape change | existing descriptor data, no shape change | n/a |
| Authoritative enemy sim | host `BrainComponent` + `AgentComponent` + `HealthComponent` | not serialized in this spec | n/a | n/a | n/a |

## Wire format

This plan changes binary snapshot validation semantics and bumps `SNAPSHOT_VERSION`.

- Existing bitcode envelope remains: `RawSnapshotMessage` with version, sequence, server tick, entity records, state-slot fingerprint, and state records.
- Existing `RawEntityRecord` field order remains unchanged. No new field is added.
- `has_entity_class = false` still requires `entity_class == ""`.
- `has_entity_class = true` on `FullBaseline` or `Delta` requires at least one finite `Transform` payload in the record. It no longer requires `PlayerMovementState`.
- `has_entity_class = true` on `Despawn` remains invalid. A non-empty `entity_class` on `Despawn` remains invalid.
- `local_player` and `last_processed_client_tick` rules do not change: either requires `PlayerMovementState`.
- Peers with the prior snapshot version are rejected by the existing handshake/version gates before apply.

## Open questions

- **Remote enemy animation baseline.** This spec deliberately defers animation-state replication. If a moving remote enemy stuck in its default idle pose is too distracting, pull a minimal animation-state payload into `E10--enemy-locomotion-animation` before steering feel playtest.
- **Health visibility.** Client materialization skips `HealthComponent`. That is correct for authority, but future hit markers, health bars, or debug overlays may need read-only replicated health or state-slot projection.
- **Broader descriptor remotes.** This plan targets AI enemies. Movers, pickups, projectiles, and combat props should get their own networking specs rather than silently riding this enemy-specific predicate.
