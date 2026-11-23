# E10--npc-equipment-sets — research

Read at e761fb13. Read at 07c46bddd: facts E21 changed. Read at bb7323fc5: source corrections throughout. Read at 73b072de3 (no source change since bb7323fc5): §Replication. Read at f29bc577d (no source change since 73b072de3): §Set names, §Gear, §Example weapons. Read at e57fcf5ea (no source change since f29bc577d): §Gear. Read at 55d245fd1 (no source change since e57fcf5ea): §Today's binding (cancellation, weapon references), §Spawn and composition (draw salts, spawner KVPs). Mount, shot-origin and NPC-materialize facts moved to `drafts/E10--npc-weapon-mounts/research.md` at the split. Findings that inform the brief; the brief carries the conclusions.

## Today's binding
- Attack rows name weapons statically: `AttackParams.weapon: Option<String>` (`crates/foundation/src/data_descriptors/types/behavior.rs`). The SDK passes the string through (`sdk/lib/data_script/attacks.ts`, `.luau`). `validate_attacks` checks shape only, not that the name exists. Stance lowering never reads `weapon`. Every `content/` call site that names a weapon string is in `content/dev/scripts/limitator.ts`. Identity-lowering precedent: `lowerWeaponMountReferences` (`sdk/lib/data_script.ts`, `.luau`).
- `resolve_projectile_attacks` (`crates/ai/src/brain_programs.rs`) builds per-entity `BrainEntityPrograms` entries, but their content is a pure function of the shared graph and the descriptors. It is refreshed when the descriptor generation changes. Projectile weapons only: a hitscan weapon disables the attack with a warn-once. The rule is enforced at runtime only, by that filter and by `advance_actor`'s `.expect("AI sync admits projectile weapons")`; no load check exists.
- Fire: the fire proposal in `compute.rs` builds `WeaponAttackRequest { canonical_weapon, attack_name }`, and `AiWeaponControllers::advance_actor` runs it.
- Row behaviour toward an unresolved attack: nothing on the row side checks resolution. A marked or queued row still takes and holds a turn and whiffs at fire. Only an unmarked pending attack never requests a turn.
- Row skip seam: `RowSelection::skips` (`crates/ai/src/graph_eval.rs`), used by `first_enabled_target`, sees the row and its direct target activity (`envelope.activities.get(&row.to)`). It does not see nested descendants.
- Cancellation: `arbitrate_attack_turns` (`crates/ai/src/attack_turns/lifecycle.rs`) cancels an execution whose turn ended, any execution on a reseat, and one whose attack no longer resolves or whose enemy holds no turn for it. A weapon that stops resolving is dropped in `AiWeaponControllers::sync` (`retain`, `active` cleared).
- Pre-existing defect: an attack whose weapon does not resolve whiffs and sets no cooldown, because the cooldown lookup in `compute.rs` goes through `resolved_projectile_attack`, which returns `None`. The attack can re-queue at once.
- `AiWeaponControllers` / `ActorWeapons` (`weapon_controller.rs`): weapons are keyed by canonical name, each with a full `WeaponComponent` (resource, activation cursor, recovery). `sync` keeps live resources and is derived from resolved attacks. It rebuilds only when the graph `Arc` (`Arc::ptr_eq`) or `descriptor_generation` changes; its early exit sees no other input.
- Other weapon references: `inventory.loadout` and E21's mount entries already take weapon handles (`sdk/types/postretro.d.ts`). `AttackParams.weapon` is the SDK's only string weapon reference. The projectile trap's FGD `weapon` key is a map string (`sdk/TrenchBroom/postretro.fgd`).
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
- Placement keys: attach reads them from `MapEntity.key_values` (`always_active`, `enabled_on_spawn`). A spawner copy's synthetic `MapEntity` carries only `enabled_on_spawn` in its KVP bag. The spawner's own KVPs are readable with `registry.get_map_kvp(spawner_id, key)`. Spawner archetypes resolve at install (`crates/postretro/src/startup/lifecycle_spawners.rs`).
- FGD: `entity_spawner` is `base(EntitySpawn)`, not `AiEnemy`.
- Diagnostics: the trap's unknown-weapon warning names an `EntityId`; `MapEntity::diagnostic_origin()` gives a placement's map origin.
- Per-instance seed: `behavior_draw_seed` (`crates/foundation/src/behavior_draws.rs`) is pure, an FNV hash of the descriptor name and the salt, so it is independent of spawn order. Draws read `behavior_draw_unit(seed, stream, counter)`; a one-off draw can pass counter 0 and store nothing. Adding a `BehaviorDrawStream` variant grows the serde'd `counters` array. Placement salt: `placement_draw_salt`, `BehaviorDrawSalt::Placement(map_ordinal)`. A spawn with no authored identity (a synthetic placement, a spawner with no map ordinal) salts on `BehaviorDrawSalt::Entity`, the registry index, which is replay-stable only while spawn order is.
- `MeshComponent.attachments` is per entity, copied at spawn by `descriptor_mesh_component`. Bindings resolve via `resolve_mesh_entity_bindings[_for_entities]` at load, at spawner spawn and on client materialize. A changed list must rebind, and its models must be in the upload sweep.
- `DefenseLayersComponent::from_descriptor` holds per-entity values and parameters. Expression parameters (`DefenseParameters`) are keyed by the descriptor's canonical name.
- Hot reload rebuilds the stack from the descriptor (`DefenseLayersComponent::refreshed`, called from `plan_defense_layers_refresh` in `refresh_plan.rs`), which would drop set overrides unless the refresh reapplies them. `refreshed` clamps each kept layer's `value` to its new capacity before any caller could reapply an override. No per-instance override storage exists. Mesh refresh is unsupported, so live attachments are kept.
- An expression-capacity layer re-evaluates its capacity every tick (`DefenseParameterSource::evaluate`), which would overwrite a literal override.
- A capacity-0 layer is skipped by every hit: `drain_layers` skips a layer whose `value <= 0`.
- Hot reload deferral: `defer_visual_asset_descriptor_refreshes` (`crates/scripting-core/src/runtime/core.rs`) defers `mesh` whole, `mesh.weaponMounts` included. It does not cover an `equipment` block, and it does not retain a removed descriptor's models.
- `HealthComponent.max` is per entity and reset from the descriptor on reload.
- Mount facts (entry shape, the E21 writer, the install check, the preload sweep, the AR_4 bakes): `drafts/E10--npc-weapon-mounts/research.md` §Mounts today. The preload sweep covers every table model, so gear entries need no new sweep.

## Replication
- An NPC's per-instance payloads are `Transform`, `MeshAnimationState`, `StatusPresence` and `presentation_binding`. No KVPs, no tags. Its class and client materialize path: `drafts/E10--npc-weapon-mounts/research.md` §Mounts today. No client path updates an NPC's attachments after materialize.
- Pawn precedent: `active_weapon_archetype` (presence flag plus string on `RawEntityRecord`). It is filled by `replication.rs::active_weapon_archetype` and validated by `validate_active_weapon_metadata` (`crates/net/src/wire.rs`), which gates on the movement component, not the class. The net crate is registry-blind; class prefixes live in `crates/netcode/src/descriptor_class.rs`. A client applies it through `update_weapon_mount_attachments`, against the pawn's class and its own descriptors (`snapshot_apply.rs`). The host-side writers, `synchronize_weapon_owner_attachments` and `synchronize_weapon_attachment_for_pawn`, call the same function.
- AI weapon cues carry resolved sound keys (`freeze_observer_weapon_cue`). Projectiles carry `projectile:<weapon>`. Clients need no NPC weapon name for either.
- `WIRE_VERSION` (`crates/net/src/handshake.rs`, 43 at 73b072de3) and `SNAPSHOT_VERSION` (`crates/net/src/wire.rs`, 27) both bump for a new record field.
- `validate_active_weapon_metadata` rejects a false presence flag with a non-empty string (`MalformedActiveWeaponMetadata`) and a present value on a record without movement state (`ActiveWeaponMetadataWithoutMovement`).
- The compatibility digest (`mod_digest.rs`) excludes entity descriptors.

## Set names
- `scripting.md` §2, Ghost style declarations: "The name is an explicit field, not binding-name sugar, because map files reference it as a string."
- `scripting.md` §2, Damage type and defense layer declarations: binding-name sugar, though a mover's FGD `crush_damage_type` references a damage type by that name. An unknown name there warns once per level load and deals the default type. So §2 already holds one map-referenced name under sugar.
- `scripting.md` §1, Handles over strings: "Strings remain where a map or content resolves the name". A set is both: a handle in script, a string in the map.
- Luau has no build step: `defineDefenseLayer("armor")` passes the name (`content/dev/scripts/defenses.luau`), and the SDK's sugar diagnostic covers `defineStore`, `defineImpactEvent`, `defineDefenseLayer`, `defineDamageType` and `defineStatus` (`sdk/lib/data_script/impact_events.luau`).
- Cost of sugar: a TS binding rename changes the set's name with no error; maps still naming the old one warn and draw.
- The script compiler has no descriptor knowledge, so a map key's set name is checked at load, not at compile.

## Gear
- No gear exists today. E21's `weaponMounts` entry is the per-rig precedent: `weapon` plus an `attachments` map baked for that character's rig, with an empty map rejected and paths checked (`validate_weapon_mounts`, `crates/entities/src/data_descriptors/types/weapon_mounts.rs`; `resource_management.md` §7). `drafts/E10--npc-weapon-mounts/` renames it `mesh.mounts`, keyed by `item`, so a gear item takes an entry the way a weapon does.
- A rig-independent identity keeps gear out of the set state problem. The superseded shape put gear on the archetype's set entry (`equipment: [{ set, weight, attachments }]`). A client could resolve that gear only through the entry's identity, so the host had to keep the worn set's name past spawn and the record had to carry it, against the first Decision. A gear item named on the record resolves through the client's own `mesh.mounts`, as a weapon does.
- Names: TS handles of the `defineDefenseLayer` kind carry `{ kind, name }` (`sdk/lib/data_script.ts`); `defineDefenseLayer()` takes no argument in TS and the name in Luau (`content/dev/scripts/defenses.ts`, `.luau`). A gear item has no map reference, so it needs none of the set-name divergence.
- `item` lowers to a name, as `weapon` does today. A weapon's canonical name and a gear name share that slot, so the brief rejects a collision at load rather than resolve by kind.
- Gear on a socket where a set weapon mounts hides that gun. The owner rules that gear wins: the displaced weapon has no drawn muzzle and fires from the eye, as any missing muzzle does under `drafts/E10--npc-weapon-mounts/`, and load warns. Two weapons on one socket still reject there, since every carried weapon must draw.

## Example weapons
- Projectile weapons in `content/dev/scripts/`: the enemy rifles (`enemy-rifle.ts`), `reference_plasma_bolt` and `reference_rocket` (`reference-projectiles.ts`). The arc, acid and incendiary rifles spread the hitscan reference rifle.
- `drafts/E16--persistent-firing-modes/` merges `enemy_rifle_burst` and `enemy_rifle_long` into one modal rifle; `enemy_rifle`, the traps' single shot, is named by no attack and stays.
- `reference_rocket` has a four-round magazine and the AI never reloads, so an NPC would dry-fire after four shots. `reference_plasma_bolt` spends a cell that refills after a pause; its charged secondary is never driven, since enemies drive primary only.

## Prior commitments touched
- `done/E16--enemy-ranged-projectile/`: enemies carry no `Inventory`; a per-enemy weapon instance entity is a non-goal. It rejected `Inventory` because enemies needed no switching. This brief gives them equipment that changes, and the rejection still holds: Inventory has one active weapon, while an NPC's melee and ranged attacks can overlap; and the registry holds only what must be addressed one by one (`entity_model.md` §1, Registry versus pool), while nothing addresses an NPC's carried weapon, so weapon entities on every NPC in a horde would add registry load and serve no one. Weapon state stays in the AI weapon controllers, keyed by carried weapon name. A later NPC pickup copies the picked weapon's state in; it never holds the weapon.
- `done/E10--enemy-stances-and-telegraphs/`: the exhaustive `attacks` list is the load-time half of weapon-gated attacks.
- `done/E21--bone-sockets-attachments/`: deferred a runtime attach/detach API to replicated weapon identity. This brief's replicated carried weapons deliver it for NPCs.
- `done/E18--spawner-and-closet-containment/`: no per-fire spawn arguments. The `equipment` key is a placement-time spawner key, not a per-fire argument.
- `entity_model.md` §4: maps select, never tune.
- `networking.md` randomness posture: host-only, load-time, consequences-only. Enemy behavior draws are a second carved exception, "decided, not yet built" in the doc, though `behavior_draws.rs` exists. The set draw rides it and runs at spawn, so the doc amendment says so.
- `done/E16--wieldable-switching-inventory/`: a live loadout edit applies at the next install, not to a live inventory. Held NPC equipment follows that rule on hot reload.
- `done/E10--enemy-stances-and-telegraphs/`: rejects a reached rest with no way out. This brief runs that check once per declared set.
- Roadmap "Patrol routes": per-instance variants chosen by map or script. It may cite this brief's selection rule but decides its own unknown-name behavior.
- `drafts/E16--persistent-firing-modes/`: one modal weapon is one identity, so one `carries` guard and one set entry serve every mode. Sets choose weapons; attacks choose modes.
- `drafts/descriptor-instance-overrides/`: delegates to whichever of it and this brief builds second the rule that a base weapon's mount entry serves the variants a set lists; `carries` keys on the variant.
- `CarriedLoadoutWire` (`crates/net/src/wire/control.rs`): the player's carried loadout across level changes. An NPC's carried equipment is a different thing and never crosses a level.
- `done/E21--character-weapon-mounts/`: its loud missing-entry rule for a declared table extends to set weapons and set gear through `drafts/E10--npc-weapon-mounts/`, which owns NPC mount consumption, the shot origin and the dev content move.

## Ordering pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| E-1 | Equip drops the weapon of the NPC's active attack activity, under a mark, a hold or a release | equip apply → next decide → arbitration | the activity reads as left on that decide, through its first enabled row, else the graph's initial activity; its turn ends; under an equip that keeps the weapon, the activity and its turn continue |
| E-2 | Equip and a hit on one NPC, same tick | equip applies at one fixed point in the tick relative to impact settle | the hit drains the whole pre-equip stack or the whole post-equip stack, never a mix, the same under every frame split |
| E-3 | Equip whose subjects include a zero-HP NPC and a held NPC | group resolve, then per-subject apply | the zero-HP subject is skipped silently with state unchanged (status precedent: zero health accepts no new instance); the held subject is equipped like the others, and once released it fires and mounts the new set |
| E-4 | Equip, then the next snapshot; a late join; relevance re-entry | equip apply → snapshot capture on the same tick | the snapshot stamped with the equip tick carries post-equip lists; a later materialize composes from them |
| E-5 | Two equips on one tick, A→B→A | reaction order, sequential | the result matches two equips on separate ticks: what B dropped arrives fresh, and layers B changed arrive full |
| E-6 | Runtime equip, then a same-map restart | restart respawns from placement or spawner | keyed or drawn set again; the equip does not survive |
| E-7 | Equip that keeps a weapon whose burst is in flight | equip sync, then the next fire step | the kept weapon's burst fires its remaining shots; only a dropped weapon's burst cancels |

## Validation history input
The first `/validate-plan` returned Reshape: set identity as runtime state forecloses single-weapon changes. The owner chose held weapons as the state (2026-11-22). The second `/validate-plan` returned Reshape on layers: replacing the whole stack diverged from `grantLayer`'s existing-layers-only rule and the reload path. The owner chose literal capacity overrides on declared layers, and kept the selection rule specific to equipment rather than a general variant pattern (2026-11-22).

The third `/validate-plan` returned Reshape: weapon mounts in sets ignored per-rig bakes. The owner chose archetype-owned weapon mounts, noting that the player avatar mount did not seat weapons correctly then, so this is a new NPC path rather than a copy of it. The owner also allowed weight-0 sets, renamed the state "carried" with the guard `carries(weapon)`, and required weapons referenced by identity rather than by string (2026-11-22). The owner will personally review the API and the brief before promotion.

Owner rulings of 2026-11-23 on shot origin, mount sharing and built-in muzzles now live in `drafts/E10--npc-weapon-mounts/research.md` §Validation history input.

A later `/validate-plan` returned Reshape (2026-11-23). The owner split the brief: `drafts/E10--npc-weapon-mounts/` first, this brief on top. Rulings for this brief: the wire carries carried weapon identities, not resolved `(socket, model)` mounts; an unused mount entry warns rather than rejects, and only on an archetype that declares `equipment`.

A later `/validate-plan` returned Direction sound (2026-11-23). Owner rulings: gear is per rig on the archetype's set entry, and travels as identity; build order is `drafts/E10--npc-weapon-mounts/`, then `drafts/E16--persistent-firing-modes/`, then this brief, so sets choose weapons and attacks choose modes; set names stay binding-name sugar over an explicit name field; the `drafts/descriptor-instance-overrides/` reconciliation is acknowledged; give-and-take-first and gear-in-the-set join the rivals. The owner reviews the changed Scripting surface. Gear as identity keeps the worn set's name past spawn, which the first Decision says does not survive: an owner item.

Owner rulings of 2026-11-23 on that item, asked for the approach that sets PostRetro up best long term:
- Gear is a first-class carried item, a `defineGear` handle named by binding (Luau passes the name). Sets list weapons and gear. An instance's state is its carried weapons and worn gear, never the set, and the wire carries both identities. This supersedes gear on the set entry, and set entries return to `{ set, weight }`. The owner's framing: a set's gear is a default loadout that can be overridden piece by piece later; gear give and take are additive, like weapons.
- One `mesh.mounts` table keyed by `item`, owned and renamed by `drafts/E10--npc-weapon-mounts/`; this brief extends `item` to gear.
- Gear wins its socket: base attachments, then carried weapons' mounts, then worn gear's. "A gear set is a default loadout but it can be overridden as needed." A gun displaced by gear fires from the eye, and load warns where a set's gear covers its own weapon's socket. This replaces the rejection of gear on a set weapon's socket.
- A missing muzzle fires from the eye (`drafts/E10--npc-weapon-mounts/`).

Owner rulings of 2026-11-23, after detail review:
- The gate applies to the active path: an activity whose attack becomes unavailable reads as left on the next decide, through its first enabled row, else the graph's initial activity, and its turn ends, mark, hold or release included.
- A reload's kept layer value clamps to the instance's override capacity, never to the base.
- `carries` on an archetype without `equipment` reads true for every attack weapon; load rejects a weapon no attack names.
- A reload that adds or removes `equipment` applies the new rule at once, chosen over "until respawn": a deliberate exception to "Hot reload never re-equips".
- A set listing an item twice, overriding a layer twice, or carrying an unknown key rejects load.
- An `equipment` key on an archetype without `equipment` warns once and carries every attack weapon. A zero-HP equip subject is skipped silently; a held one is equipped (decided by the orchestrator at the owner's request, for the smoothest player and authoring experience).
- Hitscan set weapons, in the owner's words: "I personally prefer enemies don't have hitscan weapons but some devs may want it, so it's technically valid, but I'd rather avoid it in playable test content." A set weapon AI cannot fire warns at load and its attacks stay unavailable.

## Rivals weighed
- Set identity as the state. Rejected: later single-weapon changes would need an authored set for every combination, and graphs would branch on set names.
- Give and take first: single-weapon `npcs().give` / `take` as the first runtime step. Rejected: sets bound every carried state an NPC can reach, so load checks each for a stuck rest; give and take reach states no declaration lists.
- Per-set graph lowering with graph swap. Rejected: graph identity becomes the state, which forecloses single-weapon changes further. Its reachability check is borrowed.
- Per-instance archetype selection: a placement or spawner draws from a weighted list of factory-built archetypes (the `rifleEnemy` precedent). Rejected: runtime equip still needs carried state, which leaves two mechanisms where this brief has one.
- Weapon mounts inside sets. Rejected: mounts are per rig, so sets would multiply per rig, a mixed-rig `equip` would fail, and a dropped weapon would have no mount rule.
- Gear per rig on the archetype's set entry (`equipment: [{ set, weight, attachments }]`). Superseded by the owner: worn gear then needs its set to survive spawn as state, against the first Decision, and gear cannot be given or taken apart from its set.
- Gear models inside sets. Rejected: a gear model is baked for one rig, as a weapon bake is, so a set shared across rigs would seat it wrong on every other rig or multiply per rig.
- No gear. Rejected: an armored variant would look like the rest, so a player could not read which grunt a set armored.
- Slot attacks, where one row fires whatever is carried. Rejected: weapons differ in reach and cadence, so behavior differs by weapon.
- Replicating resolved `(socket, model)` mounts or gear. Rejected: it ties every client to the host's model paths, where every other mount composes on each peer from its own descriptors.
