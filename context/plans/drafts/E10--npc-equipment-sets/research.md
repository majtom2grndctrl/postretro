# E10--npc-equipment-sets — research

Read at e761fb13. Read at 07c46bddd: facts E21 changed, §NPC shot origin, and §One rifle for players and NPCs. Read at bb7323fc5: source corrections throughout, and §Not verified. Findings that inform the brief; the brief carries the conclusions.

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
- Third-person mounts are character-owned (E21). `WeaponDescriptor.third_person_model` is retired. A character's `mesh.weaponMounts` maps each weapon to socket → model attachments baked for its rig (`resource_management.md` §7).
- The mount writer, `update_weapon_mount_attachments` (`crates/netcode/src/weapon_mounts.rs`), composes base `mesh.attachments`, then the one active weapon's entry. It runs for pawns only. NPCs still mount their rifle through `mesh.attachments`: the limitator directly, and the rifle squads by spreading its `mesh` and replacing `hand_r` (`rifle-squads.ts`).
- The limitator's attacks fire two weapons: `enemy_rifle_burst` (`shoot`, `spray`) and `enemy_rifle_long` (`longshot`, `content/dev/scripts/limitator.ts`). One `hand_r` attachment, `models/ar_4/limitator.gltf`, shows both. The rifle squads spread the limitator, keeping its attacks, and replace that attachment with their own bake.
- The weapon-mount entry type, `WeaponMountDescriptor` (`crates/entities/src/data_descriptors/types/weapon_mounts.rs`), holds `weapon` and an `attachments` map, not optional. `validate_weapon_mounts` rejects an empty map ("an entry mounts at least one socket"), and `check_weapon_mount_key` rejects any key outside `WEAPON_MOUNT_KEYS` (`weapon`, `attachments`). No entry field names a muzzle today.
- E21's install check, `check_weapon_mount_tables` (`crates/sim/src/scripting_systems/weapon_mounts.rs`), already warns for an NPC table entry whose weapon name resolves to no weapon.
- The level-install sweep and model prediction already collect every descriptor's `weaponMounts` models, NPCs included (`weapon_presentation_models`, `data_archetype.rs`). Set gear is not collected.
- E21 re-seated the AR_4 trooper and warrior bakes (`content/dev/models/ar_4/`). The warrior's rifle moved out of its torso. The trooper's bake scaled 1.41×, to 0.859 m, so its support hand reaches the handguard: the rig's hands sit ~0.54 m apart, wider than the stock AR_4's 0.29 m from grip to muzzle. Content scale standards are on the roadmap under Epic 21.

## Replication
- An NPC's snapshot identity is `entity_class = "descriptor:<canonical_name>"` (`crates/netcode/src/descriptor_class.rs`). Its per-instance payloads are `Transform`, `MeshAnimationState`, `StatusPresence` and `presentation_binding`. No KVPs, no tags.
- The client builds a remote NPC's `MeshComponent` from its own descriptor: `materialize_armed_remote_enemy` (`crates/netcode/src/remote_materialize.rs`) → `materialize_net_descriptor_presentation` (`net_descriptor.rs`). No client path updates an NPC's attachments after materialize.
- Pawn precedent: `active_weapon_archetype` (presence flag plus string on `RawEntityRecord`). It is filled by `replication.rs::active_weapon_archetype` and validated by `validate_active_weapon_metadata` (`crates/net/src/wire.rs`), which gates on the movement component, not the class. The net crate is registry-blind; class prefixes live in `crates/netcode/src/descriptor_class.rs`. A client applies it through `update_weapon_mount_attachments`, against the pawn's class and its own descriptors (`snapshot_apply.rs`). The host-side writers, `synchronize_weapon_owner_attachments` and `synchronize_weapon_attachment_for_pawn`, call the same function.
- AI weapon cues carry resolved sound keys (`freeze_observer_weapon_cue`). Projectiles carry `projectile:<weapon>`. Clients need no NPC weapon name for either.
- `WIRE_VERSION` (`crates/net/src/handshake.rs`, 43 at bb7323fc5) and `SNAPSHOT_VERSION` (`crates/net/src/wire.rs`, 27) both bump for a new record field.
- The compatibility digest (`mod_digest.rs`) excludes entity descriptors.

## NPC shot origin
Owner-observed 2026-11-23 on `movement-feel`: the `rifle_trooper` (white armor, `rodin_sci-fi_trooper_m`) fires from above its head. Pre-existing: `enemy_eye` dates from `3d45c494e` (2026-08-23, enemy static LOS fire gate), and E21 did not touch it.

Today:
- An NPC shot launches from the AI eye. `WeaponAim.origin` is `perception.enemy_eye` (`crates/ai/src/compute.rs:732-737`). The AI weapon controller launches the projectile at `aim.origin` (`crates/ai/src/weapon_controller.rs:295-297`).
- The eye is the top of the hitbox. `enemy_eye` (`crates/ai/src/perception.rs:76-90`) returns `position + hitbox.offset + Y * hitbox.half_extents.y` when the health block declares a hitbox. Without one, it falls back to `EYE_FACTOR` (0.9, `perception.rs:21`) × the baked nav agent height. The branches disagree: the hitbox branch applies no eye factor.
- Line of sight reads the same point (`perception.rs:136`).
- The AI path never calls `resolve_muzzle_launch_origin` or `muzzle_world_origin` (`crates/sim/src/weapon/mod.rs:1022`, `:432`). Those compose a player's shot from first-person placement plus `muzzleOffset`, fall back to the eye when the muzzle is obstructed, and converge on the aim target. No enemy rifle declares `muzzleOffset` (`content/dev/scripts/enemy-rifle.ts`), and adding one would change nothing.

Measured 2026-11-23:

| Archetype | Hitbox half-height and offset (y) | Shot origin | Head top | Hand and gun |
|---|---|---|---|---|
| `rifle_trooper` | 1.0, 1.0 | 2.00 m | 1.88–1.91 m | 1.5–1.6 m |
| `rifle_warrior` | 0.95, 0.95 | 1.90 m | ~1.80 m | not measured |
| `limitator` | 0.91, 0.91 | 1.82 m | ~1.74 m | not measured |

The enemy projectile sprite is 0.55 m (`enemyRifleProjectile`, `enemy-rifle.ts`), so it draws mostly above the head.

Design smells (owner-agreed):
1. The eye derives from the hitbox, a damage volume. A gameplay hitbox change moves where an NPC sees from.
2. Sight origin and shot origin are one point. Line of sight and cover are a different concern from weapon launch.
3. Launch origin branches by actor kind: player projectiles go through `resolve_muzzle_launch_origin`, NPCs bypass it. That runs against `entity_model.md` §2's one weapon vocabulary, which has no player-vs-enemy branch.

Directions weighed (the owner chose the held weapon's muzzle on 2026-11-23, with a built-in weapon's muzzle as a body socket):
- A separate NPC launch point, routed through `resolve_muzzle_launch_origin` for the obstruction fallback and convergence. The eye stays for line of sight. The launch point is one of:
  - an authored chest or shoulder offset in the facing frame;
  - the held weapon's muzzle: hand joint × rig bake × the bake's rigid `muzzle` socket. No rig bake carries a `muzzle` socket today, the AR_4 bakes included; the cyberpunk viewmodels do.
- Separately, make the eye an authored height or a factor-scaled one, so a hitbox edit stops moving it.

Costs of the held-muzzle source:
- Reading the held mount for gameplay reverses E21's Decision "Gameplay never reads a mount" (`networking.md` §Weapon placement is content) for host-only NPCs.
- Aim drift with clip pose: a muzzle read from the posed hand moves with every clip. For scale, Mixamo rifle clips hold the hand-to-hand line 24° or more off the +Z mount (measured 2026-11-05: `idle_aiming` 24°, `firing_rifle` 38°).

## One rifle for players and NPCs
- The dev enemy rifles (`enemy-rifle.ts`) all fire projectiles at 20 m/s: `enemy_rifle` (10 damage, 12 m), `enemy_rifle_long` (10 damage, 20 m) and `enemy_rifle_burst` (6 damage, three shots 120 ms apart, 12 m). The owner treats the slow, dodgeable shot as balance.
- `reference_rifle` (`reference-rifle.ts`) is hitscan, 9 damage, 80 m, with `muzzleOffset` and a first-person placement. The AI admits projectile weapons only: a hitscan attack weapon is disabled with a warn-once (§Today's binding). So NPCs cannot wield it as authored.
- Its exo_red bake already sits at `models/cyberpunk_weapons/rifle/exo_red.gltf`. An NPC rig bake would sit beside it as `<rig>.gltf`.
- The SDK has no per-use descriptor override. Spreading a weapon's components derives a new weapon identity: `incendiary_rifle` spreads `referenceRifleEntity.components` and its `weapon` block (`content/dev/scripts/status-hazards.ts`). So an NPC cannot fire `reference_rifle` with a per-use change, such as a projectile, without a second weapon. The owner left shared weapons to `drafts/descriptor-instance-overrides/` (2026-11-23).

## Prior commitments touched
- `done/E16--enemy-ranged-projectile/`: enemies carry no `Inventory`; a per-enemy weapon instance entity is a non-goal. Kept: a set is not an inventory.
- `done/E10--enemy-stances-and-telegraphs/`: the exhaustive `attacks` list is the load-time half of weapon-gated attacks.
- `done/E21--bone-sockets-attachments/`: deferred a runtime attach/detach API to replicated weapon identity. This brief's replicated NPC mount list delivers it for NPCs.
- `done/E18--spawner-and-closet-containment/`: no per-fire spawn arguments. The `equipment` key is a placement-time spawner key, not a per-fire argument.
- `entity_model.md` §4: maps select, never tune.
- `networking.md` randomness posture: host-only, load-time, consequences-only. Enemy behavior draws are a second carved exception, "decided, not yet built" in the doc, though `behavior_draws.rs` exists. The set draw rides it and runs at spawn, so the doc amendment says so.
- `done/E16--wieldable-switching-inventory/`: a live loadout edit applies at the next install, not to a live inventory. Held NPC equipment follows that rule on hot reload.
- `done/E10--enemy-stances-and-telegraphs/`: rejects a reached rest with no way out. This brief runs that check once per declared set.
- Roadmap "Patrol routes": per-instance variants chosen by map or script. It may cite this brief's selection rule but decides its own unknown-name behavior.
- `done/E21--character-weapon-mounts/`: "Gameplay never reads a mount". This brief reverses it for NPCs, whose shots only the host computes, by launching them from the mounted muzzle. That brief's loud missing-entry rule for a declared table extends to set weapons. It deferred the rifle squads' move to `mesh.weaponMounts` here; the limitator mounts the same rifle the same way, so it moves too.

## Not verified
- That the host has an NPC's posed skeleton at fire time. The host animates NPCs for hit zones, and `HitZoneStore::posed_socket_world` (`crates/sim/src/scripting_systems/hit_zones.rs`) samples a body socket's world position on demand from `MeshComponent.animation` and an `anim_time`. Not checked: which clock the AI fire step could pass as `anim_time` (AI clip time is game time, `entity_model.md` §7c), and how an attachment's own `muzzle` socket composes on the sampled holder joint.

## Validation history input
The first `/validate-plan` returned Reshape: set identity as runtime state forecloses single-weapon changes. The owner chose held weapons as the state (2026-11-22). The second `/validate-plan` returned Reshape on layers: replacing the whole stack diverged from `grantLayer`'s existing-layers-only rule and the reload path. The owner chose literal capacity overrides on declared layers, and kept the selection rule specific to equipment rather than a general variant pattern (2026-11-22).

The third `/validate-plan` returned Reshape: weapon mounts in sets ignored per-rig bakes. The owner chose archetype-owned weapon mounts, noting that the player avatar mount did not seat weapons correctly then, so this is a new NPC path rather than a copy of it. The owner also allowed weight-0 sets, renamed the state "carried" with the guard `carries(weapon)`, and required weapons referenced by identity rather than by string (2026-11-22). The owner will personally review the API and the brief before promotion.

Owner rulings, 2026-11-23: NPC shots launch from the carried weapon's muzzle, a built-in weapon's from a body socket; one rifle for players and NPCs waits for `drafts/descriptor-instance-overrides/`; a missing mount entry is loud, as in E21; carried weapons never share a mount socket.

Later owner rulings, 2026-11-23: two carried weapons may share a socket when their entries name the same model, for now; consolidating such a pair into one modal weapon goes to `drafts/E16--persistent-firing-modes/`, widened the same day for enemy weapons. A built-in weapon's muzzle is a `mesh.weaponMounts` entry naming `muzzle`, without `attachments`. The owner's reason: it avoids a precedent mutually exclusive with melee weapons, which later briefs expand on.
