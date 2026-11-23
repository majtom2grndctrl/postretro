# E10--npc-weapon-mounts — research

Read at bb7323fc5, split from `drafts/E10--npc-equipment-sets/research.md`. Read at 73b072de3 (no source change since bb7323fc5): §Muzzle pose. Read at f29bc577d (no source change since 73b072de3): §Blocked muzzle. Read at e57fcf5ea (no source change since f29bc577d): §Rename consumers. Findings that inform the brief; the brief carries the conclusions.

## NPC shot origin
Owner-observed 2026-11-23 on `movement-feel`: the `rifle_trooper` (white armor, `rodin_sci-fi_trooper_m`) fires from above its head. Pre-existing: `enemy_eye` dates from `3d45c494e` (2026-08-23, enemy static LOS fire gate), and E21 did not touch it.

Today:
- An NPC shot launches from the AI eye. `WeaponAim.origin` is `perception.enemy_eye` (`crates/ai/src/compute.rs`, the fire proposal). The AI weapon controller launches the projectile at `aim.origin` (`AiWeaponControllers::advance_actor`, `weapon_controller.rs`).
- The eye is the top of the hitbox. `enemy_eye` (`crates/ai/src/perception.rs`) returns `position + hitbox.offset + Y * hitbox.half_extents.y` when the health block declares a hitbox. Without one, it falls back to `EYE_FACTOR` (0.9) × the baked nav agent height. The branches disagree: the hitbox branch applies no eye factor.
- Line of sight reads the same point (`perception.rs`).
- The AI path never calls `resolve_muzzle_launch_origin` or `muzzle_world_origin` (`crates/sim/src/weapon/mod.rs`). Those compose a player's shot from first-person placement plus `muzzleOffset`, fall back to the eye when the muzzle is obstructed, and converge on the aim target. No enemy rifle declares `muzzleOffset` (`content/dev/scripts/enemy-rifle.ts`), and adding one would change nothing.
- `resolve_muzzle_launch_origin` takes the placement and a model-local muzzle and composes the world muzzle itself. Its obstruction test is the eye-to-muzzle sweep at the projectile's radius, plus the eye ray reaching world at or before the muzzle's forward plane. An NPC caller already holds a world muzzle point.

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

Directions weighed (the owner chose the held weapon's muzzle on 2026-11-23, with a built-in weapon's muzzle as a body socket; a later ruling dropped the eye fallback, §Blocked muzzle):
- A separate NPC launch point. The eye stays for line of sight. The launch point is one of:
  - an authored chest or shoulder offset in the facing frame;
  - the held weapon's muzzle: hand joint × rig bake × the bake's rigid `muzzle` socket.
- Separately, make the eye an authored height or a factor-scaled one, so a hitbox edit stops moving it.

Costs of the held-muzzle source:
- Reading the held mount for gameplay reverses E21's Decision "Gameplay never reads a mount" (`networking.md` §Weapon placement is content) and `resource_management.md` §7's presentation-only sockets, for host-only NPCs.
- Aim drift with clip pose: a muzzle read from the posed hand moves with every clip. For scale, Mixamo rifle clips hold the hand-to-hand line 24° or more off the +Z mount (measured 2026-11-05: `idle_aiming` 24°, `firing_rifle` 38°).

## Blocked muzzle
Owner ruling, 2026-11-23, after `/validate-plan` returned Under-scoped: a muzzle whose path to the target is blocked whiffs; the shot never launches from the eye.
- The eye gate: `line_of_sight_combined` from `enemy_eye` to `target_aim`, world and movers at the tick's pose (`perception.rs`). `perceive_target` debounces it through the LOS grace (`debounce_los`), and `fire_gate` reads the debounced verdict.
- The fire proposal builds `WeaponAim` whenever target perception exists, visible or not (`compute.rs`). `advance_actor` cancels an execution only when the aim is absent, and each due shot launches at that tick's aim. So a running burst keeps firing after sight is lost.
- `enemy_eye_height_clears_low_cover_but_full_cover_blocks_fire` (`crates/ai/src/ai_tests.rs`): a meshless NPC with a hitbox half-height 1.5 at offset 0, so its eye is 1.5 m; the pawn 2 m away, eye 0.5 m; walls at x = 1, 0.9 m (the eye ray crosses at 1.0 m, so it fires) and 1.25 m (blocked). Its attack is contact damage (8 HP, applied in-tick), not a projectile, so it proves the eye gate only.
- The band the ruling accepts: cover between gun height and the hitbox top, near the NPC, passes the eye gate and blocks the muzzle. The measured eyes sit 0.08–0.12 m above the head top, and the trooper's 0.4–0.5 m above its gun (§NPC shot origin). An eye at head height would narrow it; the eye's derivation stays out of scope.
- Rivals weighed: launch from the muzzle into the cover (the NPC shoots the lip in front of it); fall back to the eye as a pawn does (restores the defect); the chest or shoulder offset (the owner's earlier rejection: off the drawn gun).
- Foreclosure flagged by validation: once the muzzle socket is gameplay, art edits are gameplay edits. Re-baking an AR_4 (its scale, seat or solve) or moving its `muzzle` node moves the shot start and changes which cover blocks the NPC. `solve-weapon-mount --check` measures orientation only (`resource_management.md` §7).

## Muzzle pose
- The clip clock is `ClipClock` (`crates/entities/src/components/brain/clip_clock.rs`; rules in `crates/ai/src/clip_clock.rs`). `state` is the drawn animation state with its travel-variant suffix; `time` is unwrapped clip time, each tick's `dt × rate` from the state's start or drawn phase. `begin_tick` advances it for every brain on every tick the NPC acts, not only for clip-timed attacks. `state` is `None` when it tracks nothing.
- `HitZoneStore::posed_socket_world` (`crates/sim/src/scripting_systems/hit_zones.rs`) samples a body socket on demand through `sample_world_pose_for_probe`. That resolves `MeshAnimation` stamps at an animation-clock `anim_time`, and returns `None` while the current state's clip is unresolved. Its callers (damaged-enemy overlay, client markers, netcode presentation) pass presentation time. So a pose at clip time samples the drawn clip at `ClipClock.time` directly: a new entry, with no crossfade blend.
- It does not compose an attachment's own socket. A bake's rigid `muzzle` socket is a rest transform in the bake's mesh frame (`resource_management.md` §7), composed on the posed holder joint as the renderer places the attachment.
- Gameplay capsules and probes omit presentation-only pose modifiers (`entity_model.md` §7, Skeletal hit zones). The muzzle pose follows that rule.

## Mounts today
- Third-person mounts are character-owned (E21). A character's `mesh.weaponMounts` maps each weapon to socket → model attachments baked for its rig (`resource_management.md` §7). `WeaponDescriptor.third_person_model` is retired.
- The entry type, `WeaponMountDescriptor` (`crates/entities/src/data_descriptors/types/weapon_mounts.rs`), holds `weapon` and an `attachments` map, not optional. `validate_weapon_mounts` rejects an empty map ("an entry mounts at least one socket"), and `check_weapon_mount_key` rejects any key outside `WEAPON_MOUNT_KEYS` (`weapon`, `attachments`). No entry field names a muzzle today.
- The mount writer, `update_weapon_mount_attachments` (`crates/netcode/src/weapon_mounts.rs`), composes base `mesh.attachments`, then the one active weapon's entry, through `compose_mounted_attachments`. It runs for pawns only. NPCs mount their rifle through `mesh.attachments`: the limitator directly, and the rifle squads by spreading its `mesh` and replacing `hand_r` (`rifle-squads.ts`).
- The limitator's attacks fire two weapons: `enemy_rifle_burst` (`shoot`, `spray`) and `enemy_rifle_long` (`longshot`, `content/dev/scripts/limitator.ts`). One `hand_r` attachment, `models/ar_4/limitator.gltf`, shows both. The rifle squads spread the limitator, keeping its attacks, and replace that attachment with their own bake.
- E21's install check, `check_weapon_mount_tables` (`crates/sim/src/scripting_systems/weapon_mounts.rs`), already walks NPC tables and warns for an entry whose weapon name resolves to no weapon.
- The level-install sweep and model prediction already collect every descriptor's `weaponMounts` models, NPCs included (`weapon_presentation_models`, `data_archetype.rs`).
- `descriptor_mesh_component` (`data_archetype.rs`) builds an NPC's `MeshComponent` both on host attach and on client materialize (`materialize_armed_remote_enemy` → `materialize_net_descriptor_presentation`, `net_descriptor.rs`). An NPC's snapshot identity is `entity_class = "descriptor:<canonical_name>"` (`crates/netcode/src/descriptor_class.rs`), so a client resolves the same descriptor by name.
- E21 re-seated the AR_4 trooper and warrior bakes (`content/dev/models/ar_4/`). The warrior's rifle moved out of its torso. The trooper's bake scaled 1.41×, to 0.859 m, so its support hand reaches the handguard: the rig's hands sit ~0.54 m apart, wider than the stock AR_4's 0.29 m from grip to muzzle. Content scale standards are on the roadmap under Epic 21.
- No AR_4 bake carries a node socket (read at 73b072de3: no node `extras` in any of the three). The cyberpunk viewmodels do: the rifle's `model.gltf` has a `muzzle` node with `extras.socket = "muzzle"`.

## Rename consumers
Every consumer of `mesh.weaponMounts`, read at e57fcf5ea. The owner moved the rename into this brief (2026-11-23).

| Layer | Symbols | Where |
|---|---|---|
| Descriptor type | `MeshDescriptor.weapon_mounts`, `RawMeshDescriptor.weapon_mounts`, `WeaponMountDescriptor` (`weapon`, `attachments`), `MeshDescriptor::weapon_mount`, `MeshDescriptor::weapon_mount_models` | `crates/entities/src/data_descriptors/types/weapon_mounts.rs`, `entity.rs` (validation calls `validate_weapon_mounts` from the `RawMeshDescriptor` conversion) |
| Key vocabulary | `MESH_DESCRIPTOR_KEYS` (lists `"weaponMounts"`), `WEAPON_MOUNT_KEYS` (`"weapon"`, `"attachments"`), `check_mesh_key`, `check_weapon_mount_key`, `weapon_mount_shape_error`, `validate_weapon_mounts`; error text names `components.mesh.weaponMounts[i]` | `weapon_mounts.rs` |
| Runtime readers | `weapon_mounts_from_js`, `reject_unknown_mesh_keys`, Luau twins; each `entity.rs` reads the `"weaponMounts"` key | `crates/scripting-core/src/data_descriptors/{js,lua}/mesh_weapon_mounts.rs`, `{js,lua}/entity.rs` |
| SDK type registration | `MeshDescriptor` field `weaponMounts?`, type `WeaponMountDescriptor`; placement field docs name `mesh.weaponMounts` | `crates/sim/src/scripting/primitives/mod.rs`, `primitives/manifest.rs` |
| Drift guard | `mesh_sdk_fields_match_the_keys_the_descriptor_readers_accept` compares the SDK fields with `MESH_DESCRIPTOR_KEYS` and `WEAPON_MOUNT_KEYS` | `crates/sim/src/scripting/typedef/tests/surface.rs`; fixtures `expected.d.ts`, `expected.d.luau` |
| Typedef templates | weapon-handle docs naming `components.mesh.weaponMounts[].weapon` | `crates/scripting-core/src/typedef/templates/sdk_lib.d.ts`, `sdk_lib.luau` |
| SDK | `lowerWeaponMountReferences`; `MeshDescriptor.weaponMounts`, `WeaponMountDescriptor` | `sdk/lib/data_script.ts`, `.luau`; `sdk/types/postretro.d.ts`, `.d.luau` |
| Pawn writer | `update_weapon_mount_attachments` → `compose_mounted_attachments` (reads `weapon_mounts`, `weapon_mount`), reached from `synchronize_weapon_owner_attachments`, `synchronize_weapon_attachment_for_pawn`, `apply_local_weapon_attachments`, `snapshot_apply.rs` | `crates/netcode/src/weapon_mounts.rs` |
| Install check | `check_weapon_mount_tables` (reads `weapon_mounts`, `weapon_mount`), called from `lifecycle_world_cpu.rs` | `crates/sim/src/scripting_systems/weapon_mounts.rs` |
| Preload | `weapon_presentation_models` (reads `weapon_mount_models`), with `level_load/model_prediction.rs` | `crates/sim/src/scripting/builtins/data_archetype.rs` |
| Struct literals | `weapon_mounts: None` in `MeshDescriptor` builders | `netcode/src/lib.rs`, `remote_materialize.rs`, `postretro/src/main.rs`, `mod_digest.rs`, `mesh_render.rs`, `scripting-core/src/refresh_plan.rs`, `runtime/core.rs`, `net_descriptor.rs`, `wieldable_inventory.rs`, `sim/src/sim/touch.rs`, `data_archetype_test_fixtures.rs` |
| Tests | mount tables built or asserted | `netcode/src/weapon_mounts_{tests,fire_tests,path_tests,reload_tests}.rs`, `enemy_replication_harness_test.rs`; `postretro/src/client_weapon/tests.rs`, `late_baseline_tests.rs`, `level_load/{model_tests,weapon_mount_content_tests}.rs`, `startup/lifecycle_{beside,load}_tests.rs`; `scripting-core/src/data_descriptors/tests/weapon_mounts.rs`, `runtime/staged_preflight/tests.rs`, `runtime/weapon_mount_reload_tests.rs`, `staged_manifest/weapon_mount_manifest_tests.rs`; `sim/src/scripting_systems/weapon_mounts_tests.rs`, `data_archetype.rs` tests |
| Content | `mesh.weaponMounts` on the player; a comment pointing at it | `content/dev/scripts/player.ts`, `coop-presentation-entities.ts` (the `.js` beside them are ignored build output) |
| Human docs | `docs/weapon-mounts.md`, `docs/scripting-reference.md`, `docs/character-animation.md` | |
| Context library | `entity_model.md` (Components, Weapon vocabulary, Weapon mounts), `resource_management.md` §7, `networking.md` §Weapon placement is content, `index.md` router | |

The `weapon_mount` names in `crates/model/src/mount*` and `crates/tool/src/mount/` belong to the mount solver (`solve-weapon-mount`), not the table; they stay.

## One rifle for players and NPCs
- The dev enemy rifles (`enemy-rifle.ts`) all fire projectiles at 20 m/s: `enemy_rifle` (10 damage, 12 m), `enemy_rifle_long` (10 damage, 20 m) and `enemy_rifle_burst` (6 damage, three shots 120 ms apart, 12 m). The owner treats the slow, dodgeable shot as balance.
- `reference_rifle` (`reference-rifle.ts`) is hitscan, 9 damage, 80 m, with `muzzleOffset` and a first-person placement. The AI admits projectile weapons only, so NPCs cannot wield it as authored.
- Its exo_red bake already sits at `models/cyberpunk_weapons/rifle/exo_red.gltf`. An NPC rig bake would sit beside it as `<rig>.gltf`.
- The SDK has no per-use descriptor override. Spreading a weapon's components derives a new weapon identity: `incendiary_rifle` spreads `referenceRifleEntity.components` and its `weapon` block (`content/dev/scripts/status-hazards.ts`). The owner left shared weapons to `drafts/descriptor-instance-overrides/` (2026-11-23).

## Prior commitments touched
- `done/E16--enemy-ranged-projectile/`: named the posed weapon-socket muzzle the decided fire origin and deferred it as a presentation follow-on, because the socket transform was not sampled at the fixed AI tick; its first cut fired from `enemy_eye`. This brief builds that origin, as a host gameplay query on clip time rather than presentation, and supersedes the first cut.
- `index.md` §2, Designers over structure: load errors are for placements that cannot work. Two different models on one carried socket cannot: one would not draw while its shots left it.
- `entity_model.md` §7c, Sight sees movers: the muzzle-path test joins the NPC line-of-sight tests that see movers.
- `done/E21--character-weapon-mounts/`: "Gameplay never reads a mount", reversed here for host-only NPC shots. Its loud missing-entry rule extends to NPC tables. It deferred NPC consumption and the rifle squads' move to `mesh.weaponMounts` to E10; the limitator mounts the same rifle the same way, so it moves too.
- `done/E21--bone-sockets-attachments/`: sockets are presentation-only. Reversed here for one query, host NPC shot origin.
- `entity_model.md` §2: no branch on actor kind. Admitted here in launch origin only.
- `entity_model.md` §7c: clip time is the one exception to a single animation clock. The muzzle pose joins it.

## Not verified
- That the host can pose the drawn state at the brain's clip time at the AI fire step: the model's joint data on the host, the drawn clip resolved, and the bake's `muzzle` composed on the posed holder joint. The first slice tests it.

## Validation history input
`/validate-plan` on `drafts/E10--npc-equipment-sets/` returned Reshape (2026-11-23). The owner split the brief: this one first, equipment sets on top.

Owner rulings, 2026-11-23: NPC shots launch from the carried weapon's muzzle, a built-in weapon's from a body socket named by a `mesh.weaponMounts` entry without `attachments` (the owner's reason: it avoids a precedent mutually exclusive with melee weapons, which later briefs expand on). A missing mount entry is loud, as in E21. Two carried weapons may share a socket when their entries name the same model, for now; consolidating such a pair into one modal weapon goes to `drafts/E16--persistent-firing-modes/`. One rifle for players and NPCs waits for `drafts/descriptor-instance-overrides/`. After the split: the muzzle pose uses game-tick clip time; an entry names `attachments` or `muzzle`, never both, and neither rejects; missing-muzzle behavior is delegated.

A second `/validate-plan` returned Under-scoped (2026-11-23). Owner rulings: a blocked muzzle whiffs with no eye fallback, and sight, perception and target choice stay on the eye; the blocked-muzzle rivals and the superseded E16 first cut are recorded; the one-model-per-socket rule carries its reason; the art-is-gameplay foreclosure is noted. Order of the enemy-weapon briefs: this one, then `drafts/E16--persistent-firing-modes/`, then `drafts/E10--npc-equipment-sets/`.

Later owner rulings, 2026-11-23, made while settling gear in `drafts/E10--npc-equipment-sets/`:
- One `mesh.mounts` table, owned by this brief: E21's `weaponMounts` renamed and keyed by `item`, for every item a character carries or wears. The reason matches the `muzzle` entry's: no precedent that excludes melee. This brief's items are weapons; the equipment-sets brief adds gear.
- A missing muzzle fires from the eye: a bake without a `muzzle` socket, a missing body socket, an unresolved clip, or (in the equipment-sets brief) a gun displaced by gear. Authoring errors warn once at load. A blocked muzzle still whiffs.
