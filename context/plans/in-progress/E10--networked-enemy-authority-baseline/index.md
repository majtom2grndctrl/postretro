# E10 -- Networked Enemy Authority Baseline

## Goal

Make the shipped enemy foundation obey the Epic 15 host/client model before steering-feel work adds more agent state. A host owns enemy AI, steering, damage, death, and despawn. Connected clients do not simulate those enemies; they receive descriptor-backed remote enemy presentation entities from server snapshots.

This is a bridge spec, not a steering spec. It makes existing enemies visible and lifecycle-correct in co-op so `E10--enemy-steering-feel`, `E10--enemy-stuck-recovery`, `E10--enemy-locomotion-animation`, and `E10--enemy-combat-positioning` can stay server-authoritative and focused.

## Scope

### In scope

- Host registration for map-placed descriptor enemies that carry `Brain` + `Agent`.
- Connected-client level-load policy that skips local authoritative enemy spawns for those descriptor placements.
- Descriptor-backed client materialization for remote enemy presentation. First version attaches render-facing descriptor data, not client-authoritative AI.
- Host-authoritative mesh animation current-state replication for descriptor mesh presentation. Descriptor-owned mesh data stays local.
- Snapshot metadata that can carry descriptor class for non-player replicated entities.
- Remote enemy transform interpolation through the existing client interpolation buffer.
- Server-authoritative enemy death/despawn replication.
- Tests that prove no duplicate client enemy exists: no static map-spawned local enemy plus moving replicated enemy.
- Manual loopback check with one host and one connected client on a map containing the reference enemy.

### Out of scope

- Client prediction for enemies. Enemies are remote entities on every client.
- Replicating `BrainComponent`, `AgentComponent`, or `HealthComponent` as full wire payloads.
- Replicating enemy HP UI, health bars, hit markers, damage numbers, or combat-event facts. State-slot and combat-event specs own that; this bridge intentionally skips client `HealthComponent`.
- Replicating mesh descriptors, animation clips, state tables, or fade policy. Those remain local descriptor data; the wire carries only the current mesh animation state name.
- Enemy steering feel, stuck recovery, combat positioning, or pathfinding changes.
- General remote descriptor materialization for every gameplay archetype. This spec targets map-placed AI enemies only.
- Networking movers, pickups, projectiles, combat props, or other non-enemy descriptor remotes. Those get their own specs when they become gameplay requirements.
- FGD, PRL, TypeScript, or Luau surface changes.

## Acceptance criteria

- [ ] On a listen host, each map-placed descriptor enemy carrying AI is registered as a replicated authoritative entity and receives a `NetworkId` stable for that loaded level lifetime; non-AI static descriptor props remain unregistered unless an existing net path registers them.
- [ ] A connected client loading the same level does not spawn local authoritative copies of map-placed AI enemies during level install. Those enemies appear only when the host sends full baselines.
- [ ] A remote enemy full baseline carrying a descriptor class materializes a client entity with `Transform` plus the descriptor's mesh presentation data, so the mesh renderer can draw it. It does not materialize client-authoritative `Brain`, `Agent`, `Health`, `Weapon`, or `PlayerMovement` components.
- [ ] Remote enemy snapshots carry finite `Transform` payloads into `RemoteInterpolationBuffer` and may carry current mesh animation state name. Sampled remote poses come from snapshot interpolation / hold / bounded extrapolation, and remote enemy entities carry no local `Brain` or `Agent` simulation components (runnable harness assertions under 150 ms RTT + 5% loss + jitter). Visual feel is judged only by the manual loopback check.
- [ ] Server-authoritative enemy despawn removes the remote client entity and its interpolation buffer state. A late-joining client receives the current live enemy set only.
- [ ] A host-side enemy that attacks or dies remains authoritative: the client does not run AI ticks, agent steering, weapon fire, or death sweep for remote enemies.
- [ ] Snapshot wire validation accepts structurally valid non-despawn `Transform` records with `entity_class` metadata and still rejects malformed metadata on despawn records. Descriptor lookup and unknown-class behavior happen in engine apply. Old wire versions are rejected by the existing version gate.
- [ ] Existing movement prediction/reconciliation tests remain green: `local_player` and movement-authority metadata remain valid only for records carrying `PlayerMovementState`.
- [ ] Manual loopback check on `content/dev/maps/combat-demo.prl` (compiled from `content/dev/maps/combat-demo.map` if absent): host and connected client both see one moving enemy, not a duplicate; killing the enemy on the host removes it from the client under the Phase 2/3 `75ms 30ms loss 5%` loopback profile.

## Tasks

### Task 1: Split remote descriptor materialization helpers

Move descriptor materialization code that netcode needs out of the oversized `scripting/builtins/data_archetype.rs` into a focused helper module, e.g. `scripting/builtins/net_descriptor.rs`. Keep `spawn_net_slot_pawn` and `materialize_net_local_movement_component` behavior-identical. Add a new presentation-only helper, `materialize_net_remote_enemy_presentation`, that takes an `entity_class`, descriptor table, registry, and existing entity id, then attaches only descriptor presentation components needed for a remote enemy mesh. It must be idempotent and must not attach `Brain`, `Agent`, `Health`, `Weapon`, or `PlayerMovement`.

### Task 2: Split net snapshot apply/metadata glue

Before extending `netcode/client.rs`, `netcode/mod.rs`, and `crates/net/src/wire.rs`, extract small focused modules for descriptor-class metadata and remote presentation materialization call sites. Preserve current player `entity_class` behavior exactly. This is a behavior-preserving split plus call-site routing, so the later wire change does not deepen the large files.

### Task 3: Wire descriptor class and mesh animation state for remote enemies

Bump `SNAPSHOT_VERSION` and the transport wire gate. Keep the existing `RawEntityRecord.has_entity_class` + `entity_class` fields, but change validation semantics: descriptor class is valid on non-despawn entity records that carry a finite `Transform`. Add a registry-blind `WireTransform` finiteness check in `crates/net/src/wire.rs` and use it when validating `Transform` payloads and `entity_class` records. Add a mesh-animation-state payload that carries only the current state name. Movement-authority metadata remains valid only with `PlayerMovementState`. Despawn records still reject every metadata flag and non-empty class string. Update raw-to-typed validation tests, drift guards, malformed-envelope tests, and snapshot encode/decode round trips.

### Task 4: Host enemy registration

After descriptor map placements materialize on a host, call the registration helper from `startup/lifecycle.rs` immediately after `apply_data_archetype_dispatch`. Register map-placed entities carrying both `Brain` and `Agent` in `ReplicableSet` and stamp their `NetworkId`. Registration is host-only and reload-safe: ids are stable only for the lifetime of the loaded host level through the existing allocator, and stale enemy ids from the previous level are unregistered or ignored before new level entities are registered. Store map-placed enemy ids in host endpoint tracking so reload cleanup has one owner. Snapshot production stamps `entity_class` from `DescriptorProvenance` for those map-placed enemies, not only for `NetworkSlot` movement pawns. Also update `MovementAuthority::for_recipient` in `crates/net/src/replication.rs` so `entity_class` can ride non-despawn finite-`Transform` records while `local_player` and resolved-tick metadata remain movement-only.

### Task 5: Connected-client spawn suppression

Extend level install so a connected client skips descriptor map placements that would create authoritative AI enemies. This must happen before or inside `apply_data_archetype_dispatch`; the existing connected-client `player_spawn` suppression runs later and is too late for descriptor map placements. Use a shared helper predicate, e.g. `is_networked_ai_map_enemy`, whose contract is map-placement descriptor spawn plus live `Brain` and `Agent` components, or add a test invariant that every descriptor `ai` materialization produces live `Brain` + `Agent` components. Do not use `DescriptorProvenance.owned_components`; it does not include AI. Static props, FX, lights, and other non-AI descriptor placements keep materializing locally as today. Add tests proving connected clients skip AI descriptors while single-player and host installs keep them.

### Task 6: Client remote enemy materialization

When client snapshot apply spawns an unmapped non-local entity from a full baseline that carries `entity_class`, call `materialize_net_remote_enemy_presentation` before the frame renders. Do not do descriptor lookup inside `ClientReplication::apply_snapshot`, where descriptor tables are unavailable. Instead, either surface a remote-materialization request in `ApplyOutcome` or call from `client_receive_and_apply`, where descriptors are already in scope. Keep local-player arming on its current movement path. On deltas and re-baselines, materialization is idempotent and does not reset mesh animation state. Unknown descriptor class logs and leaves the entity transform-only rather than rejecting the snapshot.

### Task 7: Integration tests and loopback check

Add focused unit tests for host registration, client spawn suppression, non-movement `entity_class` validation, remote presentation materialization, registered-enemy despawn cleanup, and no-duplicate enemy state. Tests cover missing/despawned registered enemies flowing through the existing absent-from-tick tracker path, host tracking cleanup or stale-id ignore, client interpolation-buffer forget, and late join receiving only live enemies. Add or extend an in-memory net harness that drives host enemy transform replication to a connected client registry. Finish with a manual loopback recipe: compile `content/dev/maps/combat-demo.map` to `content/dev/maps/combat-demo.prl` if needed, run a host with `cargo run -p xtask -- run --host content/dev/maps/combat-demo.prl`, run a client with `cargo run -p xtask -- run --connect 127.0.0.1:<port> content/dev/maps/combat-demo.prl`, and use `tc netem delay 75ms 30ms loss 5%` for the conditioned profile.

## Sequencing

**Phase 1 (sequential):** Task 1, Task 2 -- split large files before adding behavior.

**Phase 2 (sequential):** Task 3 -- changes the wire contract consumed by host production and client apply.

**Phase 3 (concurrent):** Task 4, Task 5 -- host registration and connected-client spawn suppression use separate seams but must both exist before integration.

**Phase 4 (sequential):** Task 6 -- consumes Task 1's materializer and Task 3's non-movement descriptor metadata.

**Phase 5 (sequential):** Task 7 -- verifies the whole host-to-client enemy path after all behavior lands.

## Rough sketch

- Existing wire fields already carry descriptor class: `RawEntityRecord.has_entity_class` and `entity_class`. Today `RawEntityRecord::validate_entity_class` rejects class metadata unless the record carries `PlayerMovementState`. Relax only this rule to accept structurally valid non-despawn `Transform` records. Keep descriptor lookup and unknown-class policy in engine apply. Keep `validate_movement_metadata` unchanged.
- Existing wire component payloads stay limited to `Transform`, `PlayerMovementState`, and `MeshAnimationState`. Enemy movement rides as `Transform`; enemy presentation animation rides as current mesh state name only. This avoids full `Agent` / `Brain` / descriptor payload design.
- Add a host-side helper near replication ownership, not in the net crate. Candidate shape: collect entities whose `DescriptorProvenance.spawn_path == DescriptorSpawnPath::MapPlacement` and whose live registry columns include both `ComponentKind::Brain` and `ComponentKind::Agent`, stamp/register them in the host `ReplicableSet`, and keep a small host tracking set for reload cleanup. Do not use `DescriptorProvenance.owned_components` for AI; it does not include `Brain` or `Agent`.
- Generalize `movement_entity_class` in `netcode/replication.rs` into descriptor-class extraction. It still returns `"player"` for `NetworkSlot` movement pawns. It also returns descriptor canonical names for registered map-placed AI enemies.
- Client apply currently spawns an unmapped full baseline from its first finite `Transform`, maps `NetworkId -> EntityId`, and applies payloads. Thread the typed record's `entity_class` into that spawn path, or record a post-apply materialization request in `ApplyOutcome` so `client_receive_and_apply` can call the descriptor helper where descriptor tables are already in scope.
- Presentation materialization should attach `MeshComponent` from the local descriptor, including animation state declarations and default state. Snapshot apply may update only the mesh's current state name. Do not attach `BrainComponent` or `AgentComponent`: connected clients skip full sim and must not carry hidden authoritative AI state.
- Connected-client spawn suppression must run before or inside descriptor map dispatch. `apply_data_archetype_dispatch` runs before boot-player suppression, so the AI descriptor filter cannot live beside `spawn_from_player_starts`. Pass the role/client flag into descriptor map dispatch or filter descriptor `map_entities` before dispatch. Avoid classname special cases; use the shared AI-enemy predicate or the descriptor `ai` materialization invariant.
- Registered enemies that vanish from the host registry should use the existing absent-from-tick tracker path to emit despawn records. Cleanup host tracking on removal, or make stale registrations harmless. Client despawn apply must remove the remote entity and forget interpolation-buffer state. Late join baselines include only currently live registered enemies.
- Remote interpolation already samples all remote mapped entities. A transform-only remote enemy should follow the same path as the Phase 2 demo mover once it is registered and mapped.
- Oversized-file watch: `netcode/mod.rs`, `netcode/client.rs`, `crates/net/src/wire.rs`, `startup/lifecycle.rs`, `scripting/builtins/data_archetype.rs`, and `scripting/registry.rs` are all over the split smell threshold. Prefer focused helper modules and small call-site edits.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Descriptor class metadata | `EntityRecord::entity_class` / `RawEntityRecord.entity_class` | `entity_class: String` with `has_entity_class: bool`; structurally valid on non-despawn `Transform` records | n/a | n/a | existing `entity_class` only for player spawns; no new KVP |
| Enemy replicated pose | `ComponentPayload::Transform` | `component_kind = 0`, `WireTransform` | n/a | n/a | n/a |
| Player movement authority | `ComponentPayload::PlayerMovementState` | `component_kind = 6`; only records with this payload may carry `local_player` or `last_processed_client_tick` | n/a | n/a | n/a |
| Enemy mesh animation state | `ComponentPayload::MeshAnimationState` | `component_kind = 9`; current state name only | n/a | n/a | n/a |
| Remote enemy presentation | descriptor `mesh` block -> client `MeshComponent` | descriptor class names plus current mesh animation state; descriptor lookup is engine-side and mesh data stays shared local content | existing descriptor data, no shape change | existing descriptor data, no shape change | n/a |
| Authoritative enemy sim | host `BrainComponent` + `AgentComponent` + `HealthComponent` | not serialized in this spec | n/a | n/a | n/a |

## Wire format

This plan changes binary snapshot validation semantics, adds a mesh-animation-state payload, and bumps `SNAPSHOT_VERSION` plus the transport wire gate.

- Existing bitcode envelope remains: `RawSnapshotMessage` with version, sequence, server tick, entity records, state-slot fingerprint, and state records.
- Existing `RawEntityRecord` field order remains unchanged. No new field is added.
- `RawComponentPayload` gains `mesh_animation_state`. It carries only current state name; descriptor mesh data stays local.
- `has_entity_class = false` still requires `entity_class == ""`.
- `has_entity_class = true` on `FullBaseline` or `Delta` requires at least one structurally valid finite `Transform` payload in the record. It no longer requires `PlayerMovementState`.
- `WireTransform` validation in `crates/net/src/wire.rs` rejects NaN/inf in position, rotation, or scale before typed apply. This check is registry-blind and also backs the `entity_class` + `Transform` rule.
- `has_entity_class = true` on `Despawn` remains invalid. A non-empty `entity_class` on `Despawn` remains invalid.
- `local_player` and `last_processed_client_tick` rules do not change: either requires `PlayerMovementState`.
- Peers with the prior snapshot version are rejected by the existing handshake/version gates before apply.
