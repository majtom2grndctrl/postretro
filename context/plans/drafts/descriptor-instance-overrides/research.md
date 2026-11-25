# descriptor-instance-overrides — research

Read at bb7323fc5. Findings that inform the brief; the brief carries the conclusions.

## What exists today

- A spread derives a new weapon. `incendiaryRifleEntity` (`content/dev/scripts/status-hazards.ts`) spreads `referenceRifleEntity.components` and overrides `damage` and `damageType` under a new `canonicalName`, `incendiary_rifle`. Composition happens in script; the engine sees a full descriptor.
- Spreading the reference rifle into a projectile weapon validates today. `WeaponDescriptor` validation (`combat/weapon_validation.rs`) requires `projectile` with `resolution: "projectile"` and rejects `splash` on hitscan. It does not reject bloom, `muzzleOffset`, `placement` or `viewmodel` on a projectile weapon.
- So overriding works. What a spread cannot keep is identity: the result is a different weapon to everything keyed on the name (below). Its visible cost today is in `content/dev/scripts/player.ts`: the incendiary rifle needs its own `mesh.weaponMounts` entry pointing at the rifle's bake ("Spreads the reference rifle, so it shares the rifle's bake").
- Commitment: `done/weapon-placement/` decided "Sparse override is authored in script via object spread, not an engine merge". The placement resolver (`resolve_weapon_placement`, `combat.rs`) reserves a per-instance tier, passed `None` everywhere; its home is `WeaponComponent`, and it lands with dual-wield.
- Design intent, not a contract: `context/research/weapon-model.md` routes per-instance stat changes (rolls, augments, even a resolution change) through the `effective()` seam on `WeaponComponent`. Roadmap Epic 16 "augments / attachments" owns that.

## Map overrides are not the vehicle

- `DescriptorMapOverride` (`crates/entities/src/provenance.rs`) lists light and emitter `initial_*` fields only. `DescriptorProvenance.map_overrides` records which landed. On hot reload, `apply_light_overrides` / `apply_emitter_overrides` (`crates/scripting-core/src/refresh_plan.rs`) re-read each recorded KVP through `override_raw`; a KVP that went away fails the refresh.
- `entity_model.md` §4: maps select, never tune; weapon damage, range and timing stay descriptor-owned. `WeaponDescriptor`'s doc comment repeats it: "maps do not override these params".
- Weapon refresh (`plan_weapon_replace`, `refresh_plan.rs`) replaces a live weapon from its descriptor by canonical name. It reapplies no override.

## Where the NPC resolves its weapon

- `AttackParams.weapon` is a canonical-name string (`behavior.rs`). `validate_attacks` (`behavior/recursive.rs`) rejects `damage`, `maxRange`, `queueRange`, `cooldownMs` and `damageType` beside `weapon`: a weapon attack takes its stats from the weapon. Positioning fields (`engagementRadius`, `standoffDistance`) stay on the attack. Origin: `done/E16--enemy-ranged-projectile/`.
- `resolve_projectile_attacks` (`crates/ai/src/brain_programs.rs`) resolves each attack's weapon with `find_descriptor`, keeps it only if `resolution == Projectile` with a `projectile` block, and clones it into one `Arc<WeaponDescriptor>` per weapon name. Otherwise: warn once, attack disabled.
- `AiWeaponControllers` (`crates/ai/src/weapon_controller.rs`): `ActorWeapons.weapons` is keyed by canonical name, each a `WeaponComponent::from_descriptor_with_canonical`. `sync` rebuilds on a graph or descriptor-generation change.
- `advance_actor` only ever initiates the primary lane (`ActivationLane::Primary`; secondary is never pressed). The AI crate issues no reload. So a secondary action is unreachable for an NPC, and an ammo resource would run dry. The reference rifle's three-shot burst is its `secondary`, and it carries a 30-round magazine.
- The AI shot reads only frozen shot values and the projectile block from that `Arc`. `ProjectileLaunch` takes `credit_source`, `damage`, `range`, speed and the projectile descriptor from the shot; `ProjectileSource.weapon` is the canonical name. A composed descriptor needs no fire-path change.
- Credit: `resolve_credit_source` (`crates/entities/src/components/weapon.rs`) uses `creditSource`, else the canonical name. The enemy rifles author `creditSource: "enemy.rifle"`; the reference rifle authors none.

## What keys on the weapon's canonical name

| Consumer | Key | Where |
|---|---|---|
| Player loadout | canonical name, lowered from the handle | SDK `defineEntity` lowering (`sdk/lib/data_script.ts`); `compose_wieldable_inventory` |
| Third-person mount | `MeshDescriptor::weapon_mount(weapon)` | `compose_mounted_attachments` (`crates/netcode/src/weapon_mounts.rs`), both peers |
| Pawn active weapon on the wire | `active_weapon_archetype` string | `RawEntityRecord` (`crates/net/src/wire.rs`) |
| Player tuning row | `WieldableTuningPayload.canonical_name` | `crates/combat-model/src/carried_loadout.rs` |
| Client wieldable | local descriptor by `canonical_name`, then host gameplay fields overlaid; sounds and viewmodel stay local | `materialize_net_local_wieldable_at_slot`, `net_descriptor.rs` |
| Projectile presentation | class `projectile:<name>` | `descriptor_entity_class` (`crates/netcode/src/descriptor_class.rs`) |
| Client projectile visual | `find_descriptor(name).weapon.projectile`; absent → nothing drawn | `materialize_armed_remote_projectile` (`remote_materialize.rs`) |
| Projectile ghost opt-in | weapon name | `ghost_links/eligibility.rs` |
| AI weapon state | canonical name | `ActorWeapons.weapons` |
| Kill credit (default) | canonical name | `resolve_credit_source` |
| Pickup ownership | `DescriptorProvenance.canonical_name` equality | `owned_count`, `crates/sim/src/sim/touch.rs` |
| Carried loadout, saves | `CarriedState.wieldables: [Option<String>]` | `carried_loadout.rs`; `save_files/loadout.rs` |
| Projectile trap | map KVP `weapon` string | `content/dev/maps/trap-*.map` name `enemy_rifle` |
| Weapon hot reload | canonical name | `plan_weapon_replace` |
| E10 draft guard | `@carries.<weapon>` | `drafts/E10--npc-equipment-sets/` |

Not keyed on the name: AI observer weapon cues carry frozen sound keys (`freeze_observer_weapon_cue`).

## Peer resolution

- Entity descriptors sit outside the mod digest (`crates/postretro/src/mod_digest.rs` header). Each peer resolves a weapon from its own descriptors by name. Admission gates on mod id, not version.
- Player gameplay tuning is host-resolved: `WieldableTuningPayload` carries resolution, projectile, damage, range, primary, secondary, resource, splash, knockback, spread and placement. `strip_local_presentation` clears sound keys and `projectile.ghost`. A composed descriptor on the host therefore reaches player prediction and host shot validation with no change. Sounds and viewmodel still resolve locally by the row's name.
- An NPC shot reaches clients only as the `projectile:<name>` class. A client resolving the reference rifle's name finds no projectile block and draws nothing. So a projectile override with no name of its own is invisible on clients.

## The three scopes

**Per reference** (an attack, set or loadout entry says "this weapon, with these fields"):
- Needs a minted identity anyway. The projectile class, AI state key, tuning row, carried loadout and saves all carry one name. A minted key such as `<weapon>@<archetype>.<attack>` changes the class grammar and needs a client resolver that recomposes from the archetype.
- A player loadout entry's override is lost when the weapon is carried, saved or picked up, because those keep only the name.
- A trap map KVP cannot name it.
- Two archetypes sharing one NPC rifle (trooper, warrior and limitator all use the burst rifle) repeat the override per site.

**Per instance** (a live entity carries an override on its weapon):
- Maps cannot author it (§4). A script step would need tuning replication for client presentation.
- It is the per-instance stat path that augments and rolls own (`weapon-model.md`; roadmap "augments / attachments").

**Derived identity with lineage** (a named variant composed at load that records its base):
- Every name-keyed path in the table works unchanged: wire, saves, AI state, projectile class, traps, hot reload.
- Only consumers that mean the physical weapon consult the base. The mount lookup is the one built today.
- No wire version, snapshot version, tuning epoch or digest change.

## Surface constraints

- Handles over strings (`scripting.md` §1): a variant shared across files is a value, as weapon handles are in `inventory.loadout` and `mesh.weaponMounts`.
- Canonical names are explicit strings because maps (trap KVP), saves and the wire hold them. Binding-name sugar would make a binding rename break saves; defense layers accept that hazard, weapons have not.
- `token.with({ ... })` is the precedent for a handle carrying per-use fields (`AttackTokenRef`). It mints no identity.
- Luau tables cannot hold `nil`, so "clear this optional block" (`resource`, `secondary`) needs an explicit form that works in both runtimes. Omission means "inherit".
- `WeaponEntityDescriptor` requires a full `components.weapon`. A variant returned already composed satisfies every existing handle consumer's type.

## Not verified

- That a dry-fired AI weapon never fires again. The brief needs only that the AI crate never reloads.
- The full client presentation path for a pawn's projectile shot. It uses `local_projectile_presentation_descriptor_class`, which reads the owner weapon's provenance name; that fits named variants, but I did not trace it end to end.
- Every reader of credit source beyond the shot and kill credit.
- Hot reload for a variant whose base changes. Composed in script, it re-derives at staged mod init by construction; not exercised.
- Whether the incendiary rifle's mount entry can be dropped without other content depending on it.
