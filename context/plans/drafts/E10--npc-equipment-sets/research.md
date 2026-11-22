# E10--npc-equipment-sets — research

Read at e761fb13. Findings that inform the brief; the brief carries the conclusions.

## Today's binding
- Attack rows name weapons statically: `AttackParams.weapon` (`crates/foundation/src/data_descriptors/types/behavior.rs`). `validate_attacks` checks shape only, not that the name exists. Stance lowering never reads `weapon`.
- `resolve_projectile_attacks` (`crates/ai/src/brain_programs.rs`) builds per-entity `BrainEntityPrograms` entries, but their content is a pure function of the shared graph and the descriptors. It is refreshed when the descriptor generation changes. Projectile weapons only: a hitscan weapon disables the attack with a warn-once.
- Fire: the fire proposal in `compute.rs` builds `WeaponAttackRequest { canonical_weapon, attack_name }`, and `AiWeaponControllers::advance_actor` runs it.
- Row behaviour toward an unresolved attack: nothing on the row side checks resolution. A marked or queued row still takes and holds a turn and whiffs at fire. Only an unmarked pending attack never requests a turn. `arbitrate_attack_turns` cancels a running burst whose attack stops resolving.
- `AiWeaponControllers` / `ActorWeapons` (`weapon_controller.rs`): weapons are keyed by canonical name, each with a full `WeaponComponent` (resource, activation cursor, recovery). `sync` rebuilds on a graph `Arc` or descriptor-generation change, keeps live resources, and is derived from resolved attacks.
- Guard facts: `BRAIN_INPUTS` (`crates/foundation/src/brain.rs`). Named reads go through `BrainInputRef`. `attackTurnFree(attack)` (`sdk/lib/brain.ts`) is the per-handle Bool precedent, checked at load against the archetype.

## No per-instance path today
- `DescriptorMapOverride` (`crates/entities/src/provenance.rs`) covers light and emitter `initial_*` fields only.
- The `AiEnemy` FGD base has `enabled_on_spawn` and `always_active`. The FGD is hand-written, and no generator exists.
- `entity_spawner` reads `archetype`, `count` and `spawned_tags` only.
- No NPC inventory: `inventory.loadout` is composed for player pawns only (`compose_wieldable_inventory`).
- `grant_layer` refills existing layers only. `npcs()` steps are `update`, `damage`, `applyStatus` and `clearStatus`.
- Variant precedent: the `rifleEnemy` factory in `content/dev/scripts/rifle-squads.ts` spreads `limitator` and swaps the `hand_r` attachment.

## Spawn and composition
- Placement and spawner both end in `attach_descriptor_components` (`data_archetype.rs`). The spawner's per-copy loop (`spawn_resolved_spawners`, `crates/sim/src/spawner.rs`) builds a synthetic `MapEntity` and reseeds the brain with `BehaviorDrawSalt::Spawned{spawner, count}`.
- Per-instance seed: `behavior_draw_seed` (`crates/foundation/src/behavior_draws.rs`) with `BehaviorDrawStream` streams. It is independent of spawn order.
- `MeshComponent.attachments` is per entity, copied at spawn by `descriptor_mesh_component`. Bindings resolve via `resolve_mesh_entity_bindings[_for_entities]` at load, at spawner spawn and on client materialize. A changed list must rebind, and its models must be in the upload sweep.
- `DefenseLayersComponent::from_descriptor` holds per-entity values and parameters. Expression parameters (`DefenseParameters`) are keyed by the descriptor's canonical name.
- Hot reload rebuilds the stack from the descriptor (`DefenseLayersComponent::refreshed`), which would drop set layers unless the refresh composes base plus set. Mesh refresh is unsupported, so live attachments are kept.
- `HealthComponent.max` is per entity and reset from the descriptor on reload.
- `WeaponDescriptor.third_person_model` exists. It mounts at a pawn's hand socket with no placement offset (`networking.md` §Weapon placement is content). Rig-specific mount bakes (`rifle-squads.ts` comment, `resource_management.md` §7) make it unsuitable as an NPC set's attachment default.

## Replication
- An NPC's snapshot identity is `entity_class = "descriptor:<canonical_name>"` (`crates/netcode/src/descriptor_class.rs`). Its per-instance payloads are `Transform`, `MeshAnimationState`, `StatusPresence` and `presentation_binding`. No KVPs, no tags.
- The client builds `MeshComponent` from its own descriptor (`materialize_net_mesh_presentation`, `net_descriptor.rs`).
- Pawn precedent: `active_weapon_archetype` (presence flag plus string on `RawEntityRecord`). It is filled by `replication.rs::active_weapon_archetype`, validated by `validate_active_weapon_metadata`, and applied by `synchronize_weapon_attachment_for_pawn`.
- AI weapon cues carry resolved sound keys (`freeze_observer_weapon_cue`). Projectiles carry `projectile:<weapon>`. Clients need no NPC weapon name for either.
- `WIRE_VERSION` (`crates/net/src/handshake.rs`) and `SNAPSHOT_VERSION` (`crates/net/src/wire.rs`) both bump for a new record field.
- The compatibility digest (`mod_digest.rs`) excludes entity descriptors.

## Prior commitments touched
- `done/E16--enemy-ranged-projectile/`: enemies carry no `Inventory`; a per-enemy weapon instance entity is a non-goal. Kept: a set is not an inventory.
- `done/E10--enemy-stances-and-telegraphs/`: the exhaustive `attacks` list is the load-time half of weapon-gated attacks.
- `done/E21--bone-sockets-attachments/`: deferred a runtime attach/detach API to replicated weapon identity. This brief's replicated NPC mount list delivers it for NPCs.
- `done/E18--spawner-and-closet-containment/`: no per-fire spawn arguments. The `equipment` key is a placement-time spawner key, not a per-fire argument.
- `entity_model.md` §4: maps select, never tune.
- `networking.md` randomness posture: host-only, load-time, consequences-only. Enemy behavior draws are a second carved exception, "decided, not yet built" in the doc, though `behavior_draws.rs` exists. The set draw rides it and runs at spawn, so the doc amendment says so.
- `done/E16--wieldable-switching-inventory/`: a live loadout edit applies at the next install, not to a live inventory. Held NPC equipment follows that rule on hot reload.
- `done/E10--enemy-stances-and-telegraphs/`: rejects a reached rest with no way out. This brief runs that check once per declared set.
- Roadmap "Patrol routes": per-instance variants chosen by map or script. It reuses this brief's selection pattern.

## Validation history input
The first `/validate-plan` returned Reshape: set identity as runtime state forecloses single-weapon changes. The owner chose held weapons as the state (2026-11-22).
