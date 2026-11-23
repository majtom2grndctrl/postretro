# E10--npc-equipment-sets — research

Read at e761fb13. Read at 07c46bddd: facts E21 changed. Read at bb7323fc5: source corrections throughout. Read at 73b072de3 (no source change since bb7323fc5): §Replication. Mount, shot-origin and NPC-materialize facts moved to `drafts/E10--npc-weapon-mounts/research.md` at the split. Findings that inform the brief; the brief carries the conclusions.

## Today's binding
- Attack rows name weapons statically: `AttackParams.weapon: Option<String>` (`crates/foundation/src/data_descriptors/types/behavior.rs`). The SDK passes the string through (`sdk/lib/data_script/attacks.ts`, `.luau`). `validate_attacks` checks shape only, not that the name exists. Stance lowering never reads `weapon`. Every `content/` call site that names a weapon string is in `content/dev/scripts/limitator.ts`. Identity-lowering precedent: `lowerWeaponMountReferences` (`sdk/lib/data_script.ts`, `.luau`).
- `resolve_projectile_attacks` (`crates/ai/src/brain_programs.rs`) builds per-entity `BrainEntityPrograms` entries, but their content is a pure function of the shared graph and the descriptors. It is refreshed when the descriptor generation changes. Projectile weapons only: a hitscan weapon disables the attack with a warn-once. The rule is enforced at runtime only, by that filter and by `advance_actor`'s `.expect("AI sync admits projectile weapons")`; no load check exists.
- Fire: the fire proposal in `compute.rs` builds `WeaponAttackRequest { canonical_weapon, attack_name }`, and `AiWeaponControllers::advance_actor` runs it.
- Row behaviour toward an unresolved attack: nothing on the row side checks resolution. A marked or queued row still takes and holds a turn and whiffs at fire. Only an unmarked pending attack never requests a turn.
- Row skip seam: `RowSelection::skips` (`crates/ai/src/graph_eval.rs`), used by `first_enabled_target`, sees the row and its direct target activity (`envelope.activities.get(&row.to)`). It does not see nested descendants.
- Cancellation: `arbitrate_attack_turns` cancels only an attack that leaves the graph. A weapon that stops resolving is dropped in `AiWeaponControllers::sync` (`retain`, `active` cleared).
- Pre-existing defect: an attack whose weapon does not resolve whiffs and sets no cooldown, because the cooldown lookup in `compute.rs` goes through `resolved_projectile_attack`, which returns `None`. The attack can re-queue at once.
- `AiWeaponControllers` / `ActorWeapons` (`weapon_controller.rs`): weapons are keyed by canonical name, each with a full `WeaponComponent` (resource, activation cursor, recovery). `sync` keeps live resources and is derived from resolved attacks. It rebuilds only when the graph `Arc` (`Arc::ptr_eq`) or `descriptor_generation` changes; its early exit sees no other input.
- Guard facts: `attackTurnFree(attack)` (`sdk/lib/brain.ts`) is the per-handle Bool precedent. It reads through a prefix-namespaced name, `@attackTurnFree.<attack>` (`ATTACK_TURN_FREE_INPUT_PREFIX`, `crates/foundation/src/brain.rs`), parsed to `BrainInputRef::AttackTurnFree` and interned by `intern_attack_turn_read` (`crates/ai/src/brain_scope.rs`). Load checks it against the archetype in `check_guard_attack_turn_reads` (`crates/scripting-core/src/runtime/types.rs`). The fixed `BRAIN_INPUTS` table is not on this path.

## No per-instance path today
- `DescriptorMapOverride` (`crates/entities/src/provenance.rs`) covers light and emitter `initial_*` fields only.
- The `AiEnemy` FGD base has `enabled_on_spawn` and `always_active`. The FGD is hand-written, and no generator exists.
- `entity_spawner` reads `archetype`, `count` and `spawned_tags` only.
- No NPC inventory: `inventory.loadout` is composed for player pawns only (`compose_wieldable_inventory`).
- `grant_layer` refills existing layers only. `npcs()` steps are `update`, `damage`, `applyStatus` and `clearStatus`.
- Group steps: `NpcGroup` steps live in `sdk/lib/data_script/commands.ts` / `.luau`, bound in Rust as `BoundTriggerCommand` (`crates/sim/src/trigger_commands.rs`). `applyStatus` warns and skips per subject but takes `HandleRef::Status`; `HandleRef` has no equipment-set kind. The queued host-only steps (Trap, AttackPool) mutate state outside the step and are the closer precedent for one that rebinds a mesh.
- Variant precedent: the `rifleEnemy` factory in `content/dev/scripts/rifle-squads.ts` spreads `limitator` and swaps the `hand_r` attachment.

## Spawn and composition
- Placement and spawner both end in `attach_descriptor_components` (`data_archetype.rs`). The spawner's per-copy loop (`spawn_resolved_spawners`, `crates/sim/src/spawner.rs`) builds a synthetic `MapEntity` and reseeds the brain with `BehaviorDrawSalt::Spawned{spawner, count}`. It claims `next_spawn_count` after attach, and only when a Brain exists.
- Placement keys: attach reads them from `MapEntity.key_values` (`always_active`, `enabled_on_spawn`). A spawner copy's synthetic `MapEntity` has no KVP bag. The spawner's own KVPs are readable with `registry.get_map_kvp(spawner_id, key)`. Spawner archetypes resolve at install (`crates/postretro/src/startup/lifecycle_spawners.rs`).
- FGD: `entity_spawner` is `base(EntitySpawn)`, not `AiEnemy`.
- Diagnostics: the trap's unknown-weapon warning names an `EntityId`; `MapEntity::diagnostic_origin()` gives a placement's map origin.
- Per-instance seed: `behavior_draw_seed` (`crates/foundation/src/behavior_draws.rs`) is pure, an FNV hash of the descriptor name and the salt, so it is independent of spawn order. Draws read `behavior_draw_unit(seed, stream, counter)`; a one-off draw can pass counter 0 and store nothing. Adding a `BehaviorDrawStream` variant grows the serde'd `counters` array. Placement salt: `placement_draw_salt`, `BehaviorDrawSalt::Placement(map_ordinal)`.
- `MeshComponent.attachments` is per entity, copied at spawn by `descriptor_mesh_component`. Bindings resolve via `resolve_mesh_entity_bindings[_for_entities]` at load, at spawner spawn and on client materialize. A changed list must rebind, and its models must be in the upload sweep.
- `DefenseLayersComponent::from_descriptor` holds per-entity values and parameters. Expression parameters (`DefenseParameters`) are keyed by the descriptor's canonical name.
- Hot reload rebuilds the stack from the descriptor (`DefenseLayersComponent::refreshed`, called from `plan_defense_layers_refresh` in `refresh_plan.rs`), which would drop set overrides unless the refresh reapplies them. `refreshed` clamps each kept layer's `value` to its new capacity before any caller could reapply an override. No per-instance override storage exists. Mesh refresh is unsupported, so live attachments are kept.
- An expression-capacity layer re-evaluates its capacity every tick (`DefenseParameterSource::evaluate`), which would overwrite a literal override.
- A capacity-0 layer is skipped by every hit: `drain_layers` skips a layer whose `value <= 0`.
- Hot reload deferral: `defer_visual_asset_descriptor_refreshes` (`crates/scripting-core/src/runtime/core.rs`) defers `mesh` whole, `mesh.weaponMounts` included. It does not cover an `equipment` block, and it does not retain a removed descriptor's models.
- `HealthComponent.max` is per entity and reset from the descriptor on reload.
- Mount facts (entry shape, the E21 writer, the install check, the preload sweep, the AR_4 bakes): `drafts/E10--npc-weapon-mounts/research.md` §Mounts today. Set gear is not in any model sweep.

## Replication
- An NPC's per-instance payloads are `Transform`, `MeshAnimationState`, `StatusPresence` and `presentation_binding`. No KVPs, no tags. Its class and client materialize path: `drafts/E10--npc-weapon-mounts/research.md` §Mounts today. No client path updates an NPC's attachments after materialize.
- Pawn precedent: `active_weapon_archetype` (presence flag plus string on `RawEntityRecord`). It is filled by `replication.rs::active_weapon_archetype` and validated by `validate_active_weapon_metadata` (`crates/net/src/wire.rs`), which gates on the movement component, not the class. The net crate is registry-blind; class prefixes live in `crates/netcode/src/descriptor_class.rs`. A client applies it through `update_weapon_mount_attachments`, against the pawn's class and its own descriptors (`snapshot_apply.rs`). The host-side writers, `synchronize_weapon_owner_attachments` and `synchronize_weapon_attachment_for_pawn`, call the same function.
- AI weapon cues carry resolved sound keys (`freeze_observer_weapon_cue`). Projectiles carry `projectile:<weapon>`. Clients need no NPC weapon name for either.
- `WIRE_VERSION` (`crates/net/src/handshake.rs`, 43 at 73b072de3) and `SNAPSHOT_VERSION` (`crates/net/src/wire.rs`, 27) both bump for a new record field.
- `validate_active_weapon_metadata` rejects a false presence flag with a non-empty string (`MalformedActiveWeaponMetadata`) and a present value on a record without movement state (`ActiveWeaponMetadataWithoutMovement`).
- The compatibility digest (`mod_digest.rs`) excludes entity descriptors.

## Prior commitments touched
- `done/E16--enemy-ranged-projectile/`: enemies carry no `Inventory`; a per-enemy weapon instance entity is a non-goal. Kept: a set is not an inventory.
- `done/E10--enemy-stances-and-telegraphs/`: the exhaustive `attacks` list is the load-time half of weapon-gated attacks.
- `done/E21--bone-sockets-attachments/`: deferred a runtime attach/detach API to replicated weapon identity. This brief's replicated carried weapons deliver it for NPCs.
- `done/E18--spawner-and-closet-containment/`: no per-fire spawn arguments. The `equipment` key is a placement-time spawner key, not a per-fire argument.
- `entity_model.md` §4: maps select, never tune.
- `networking.md` randomness posture: host-only, load-time, consequences-only. Enemy behavior draws are a second carved exception, "decided, not yet built" in the doc, though `behavior_draws.rs` exists. The set draw rides it and runs at spawn, so the doc amendment says so.
- `done/E16--wieldable-switching-inventory/`: a live loadout edit applies at the next install, not to a live inventory. Held NPC equipment follows that rule on hot reload.
- `done/E10--enemy-stances-and-telegraphs/`: rejects a reached rest with no way out. This brief runs that check once per declared set.
- Roadmap "Patrol routes": per-instance variants chosen by map or script. It may cite this brief's selection rule but decides its own unknown-name behavior.
- `done/E21--character-weapon-mounts/`: its loud missing-entry rule for a declared table extends to set weapons through `drafts/E10--npc-weapon-mounts/`, which owns NPC mount consumption, the shot origin and the dev content move.

## Validation history input
The first `/validate-plan` returned Reshape: set identity as runtime state forecloses single-weapon changes. The owner chose held weapons as the state (2026-11-22). The second `/validate-plan` returned Reshape on layers: replacing the whole stack diverged from `grantLayer`'s existing-layers-only rule and the reload path. The owner chose literal capacity overrides on declared layers, and kept the selection rule specific to equipment rather than a general variant pattern (2026-11-22).

The third `/validate-plan` returned Reshape: weapon mounts in sets ignored per-rig bakes. The owner chose archetype-owned weapon mounts, noting that the player avatar mount did not seat weapons correctly then, so this is a new NPC path rather than a copy of it. The owner also allowed weight-0 sets, renamed the state "carried" with the guard `carries(weapon)`, and required weapons referenced by identity rather than by string (2026-11-22). The owner will personally review the API and the brief before promotion.

Owner rulings of 2026-11-23 on shot origin, mount sharing and built-in muzzles now live in `drafts/E10--npc-weapon-mounts/research.md` §Validation history input.

A later `/validate-plan` returned Reshape (2026-11-23). The owner split the brief: `drafts/E10--npc-weapon-mounts/` first, this brief on top. Rulings for this brief: the wire carries carried weapon identities, not resolved `(socket, model)` mounts; an unused mount entry warns rather than rejects, and only on an archetype that declares `equipment`.
