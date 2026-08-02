# Wieldable Pickup and Drop

## Goal

Place a wieldable instance in the world and let a player take it into their inventory, or put one back.
The descriptor decides whether an item is taken by walking over it or by pressing the action key.
This is the first path that grows a live inventory; every prior path composes one at spawn and replaces
it wholesale.

## Scope

### In scope

- A `components.pickup` descriptor block that makes an otherwise-unplaceable wieldable map-placeable and
  carries its acquisition mode and radius.
- A world item that **is** the wieldable instance — same entity kind held or dropped.
- An entity-vs-player overlap pass: sphere volume per item against each player capsule, enter/exit edges.
- Two acquisition modes: `auto` (taken on the enter edge) and `press` (taken on a `use` press while
  overlapping).
- Prompt-eligible pairs reported out of the pickup pass as an in-memory per-tick result. Not a published
  script or UI surface — the intended reader is the unbuilt combat presentation substrate, so the shape
  stays internal until that spec names what it needs.
- Dropping the active wieldable back into the world, including the inhibit that stops the dropper from
  immediately re-acquiring it.
- One inventory-growth chokepoint, with a source-scan test restricting its call sites.
- Host-authoritative acquisition and drop in co-op, with world items replicated to clients.
- Client-side inventory growth over the existing tuning channel.

### Out of scope

- **Authored acquisition policy.** A player who already owns a wieldable of the same canonical name, or
  whose inventory is full, cannot take the item. Swapping the held wieldable for the world one, and
  replacing the lowest-stat wieldable drawing the same ammo type, are both out: expressing either needs
  inventory state as IR-readable facts plus a policy vocabulary word, and the ranking arm needs iteration
  the IR substrate forbids (`scripting.md` §11). A later spec owns them. That argument does **not** reach
  the duplicate case: granting the item's authored reserve instead of refusing needs neither IR facts nor
  iteration, only the item's own descriptor and the shipped `grant_ammo` chokepoint. It is out of scope
  here by owner decision, not by cost — recorded in Open questions so the price is not misread later.
- **Rendering pickup prompts.** Roadmap `E16 › Combat Feedback & Economy › combat presentation substrate`
  owns floating text and pickup prompts. This spec publishes the facts and stops.
- **Authored carry policy.** Carry across a level transition is unconditional and inherited from the
  shipped seat substrate; `E15--seat-session-identity-roster` §Out-of-scope owns the per-level-reset and
  pistol-start knobs.
- **Non-weapon pickups** — keys, currency, health and ammo items. `grantHealth`/`grantAmmo` on a trigger
  volume already covers the resource case; per-player currency is `drafts/E16--per-player-currency`.
- **Item respawn.** A taken item is gone for the level.
- **Client prediction of acquisition.** Acquisition is host-only; a client sees the item disappear after
  the round trip.
- **AI or enemy pickup.** Only player pawns acquire.
- **Automatic switch to a newly acquired wieldable**, except when the inventory held nothing.
- **Throwing.** Drop places the item; it has no velocity and no physics.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| pickup block | `EntityTypeDescriptor::pickup` | `pickup` (under `components`) | `pickup` | `pickup` | n/a — presence is authored in the data script, not the map |
| descriptor type | `PickupDescriptor` | — | `PickupDescriptor` | `PickupDescriptor` | n/a |
| acquisition mode | `PickupDescriptor::mode` | `mode` | `mode` | `mode` | n/a |
| mode: walk over | `PickupMode::Auto` | `"auto"` | `"auto"` | `"auto"` | n/a |
| mode: press to take | `PickupMode::Press` | `"press"` | `"press"` | `"press"` | n/a |
| pickup radius | `PickupDescriptor::radius` | `radius` | `radius` | `radius` | n/a — gameplay tuning is descriptor-owned (`entity_model.md` §4) |
| runtime component | `PickupComponent` | not serialized to the wire | n/a | n/a | n/a |
| component column | `ComponentKind::Pickup` | — | — | — | — |
| provenance kind | `DescriptorComponentKind::Pickup` | `"pickup"` (snake_case, matching its siblings) | n/a | n/a | n/a |
| drop action | `Action::Drop` | — | n/a | n/a | n/a |
| map placement | existing `classname` == descriptor `canonicalName` | — | — | — | author's own `@PointClass` in their FGD |

`WeaponDescriptor` carries `#[serde(rename_all = "camelCase")]`; `PickupDescriptor` follows it. Both new
field names are single words, so the two casings coincide — this is not a reason to skip the rename
attribute, which the sibling convention requires and which a later multi-word field will need.

The FGD is hand-authored and committed (`sdk/TrenchBroom/postretro.fgd`), not generated from the
descriptor registry. A modder placing a pickup adds a `@PointClass` whose classname literal equals the
descriptor's `canonicalName`, exactly as they do for any other placeable archetype. This spec adds no
FGD keys — the acquisition mode and radius are gameplay tuning and are descriptor-owned.

## Wire format

No new binary or PRL section. Three existing wire surfaces change, gated separately — the tuning-payload
epoch and the transport wire version are different compatibility promises and must not be bumped as one.

**Tuning payload.** `TuningPayload` carries `wieldables: [Option<WieldableTuningPayload>; 10]` at epoch 2.
The layout does not change; its *merge semantics* do, so the epoch increments to 3 and peers at the old
epoch are refused by the existing gate. Field order, integer widths, and the empty-slot encoding
(`None`) are unchanged — an added slot is an existing `Some(...)` in a position that previously could
only be skipped.

**Entity replication.** A world item replicates through the existing `FullBaseline` / `Delta` / `Despawn`
records with a `Transform` payload and a `entity_class` descriptor name. `entity_class` is already valid
on any non-despawn record carrying a finite `Transform` (`networking.md` §Snapshot apply ordering), so
no metadata gate widens. No new `ComponentKind` reaches the wire: `PickupComponent` is host-local, and
the client re-derives an item's acquisition mode from the descriptor its `entity_class` names.

**Per-tick input command.** `drop_pressed` joins `use_pressed` on the per-tick input levels. This widens
the transport wire, whose version gate is independent of the tuning epoch above; bump it on its own. The
field follows `use_pressed` exactly — gap-held, neutralized, and catch-up-trimmed by the command queue —
so it inherits the existing gap policy rather than defining one.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| An item is acquired by at most one player, ever | Task 3 (acquisition removes the item from the world in the same pass that decides it) | Two players' enter edges on one tick; a second enter edge after a refused attempt | AC 5, AC 6 |
| Acquisition fires on an enter edge, never on sustained overlap | Task 3 (per-item occupancy set, enter transition only) | Drop seeds the dropper into the occupancy set (Task 6); a refused attempt must not clear occupancy, or the next tick re-fires | AC 4, AC 9 |
| Pickup and drop never write `AmmoReserve` | Task 2 (the chokepoint takes no reserve argument) | The existing source-scan test allowlists exactly two non-test `credit`/`set_exact` call sites; a new one fails it | AC 8 |
| Inventory slot growth happens in exactly one place | Task 2 (chokepoint + its own source-scan test) | Task 3 and Task 6 both mutate inventories and must route through it; Task 7's client path must too | AC 7 |
| A world item is never ticked by the weapon stage | Already true — both weapon-stage entry points reach weapons through `Inventory` or `RemotePawnCommand.weapon`, and no production code scans the weapon column | A future scan-based stage would break it silently | AC 10 |
| A descriptor authoring no pickup block is not map-placeable by virtue of its weapon | Task 1 (the placeability arm keys on `pickup`, not `weapon`) | Any later widening of the weapon arm | AC 2 |
| Acquisition and drop are host decisions | Task 5, Task 6 | A client must not predict either; its inventory changes only on host word | AC 12 |
| A picked-up wieldable carries across a level transition | Inherited — carry harvests each slot's `DescriptorProvenance.canonical_name`, which a map-placed item carries like any descriptor spawn | Task 2 must not clear or rewrite provenance when a slot is filled | AC 11 |

## Ordering matrix

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Two players enter one item's radius on the same tick | Both enter edges evaluated in one pass | The player earlier in the stable `PlayerId` order acquires; the second finds the item already taken and does not acquire. Deterministic, not first-mover-by-float-distance. |
| Player drops an item and stands still | Drop seeds the dropper into the item's occupancy set; next tick sees sustained overlap | No re-acquisition, in either mode |
| Player drops, walks out of radius, walks back (auto) | Exit edge clears occupancy; later enter edge fires | Re-acquired |
| Player overlaps two press-mode items and presses once | One pass, one `use_pressed` edge, two overlapping items | The nearer item by squared centre distance is acquired; the other is untouched. Ties break on the lower `EntityId`. |
| Player overlaps a press-mode item and never presses | Enter edge, then sustained overlap | Prompt-eligible every tick while overlapping; no acquisition |
| Player overlaps an item they already own (either mode) | Enter edge, refusal | No acquisition, and **not** prompt-eligible — a prompt that cannot succeed is worse than none |
| Inventory full | Enter edge, refusal | Same as duplicate: no acquisition, not prompt-eligible |
| `use` press lands on the same tick as a trigger-volume Use activation | Trigger stage runs first, pickup pass second, both read the same `use_pressed` map | Both fire. The press is not consumed by either. |
| A script despawns an item on the tick a player enters its radius | Pickup pass runs inside the fixed tick; the removal pass runs at end of frame | Acquisition wins that tick. An item already marked for end-of-frame removal is skipped by the pickup pass. |
| Frame renders zero fixed ticks | Pass lives inside the tick loop | No pickup evaluation, no prompt refresh; last published prompt state persists for that frame |
| Frame renders two fixed ticks | Pass runs twice | The enter edge fires on the first; the second sees sustained overlap and does nothing |
| Radius authored zero or negative | Validation at descriptor load | Warn once naming the descriptor, clamp to zero; the item spawns and is never acquirable |
| Drop with an empty inventory | No active wieldable | No-op, no warning — pressing drop with nothing held is ordinary input |
| Drop of a wieldable whose descriptor has no pickup block | Drop refused at the chokepoint | Nothing leaves the inventory; warn once per descriptor. An item with no pickup block could never be recovered. |
| Host acquires an item a client is standing on | Host decides, despawn tombstone replicates | The client's item disappears one round trip later; the client never predicts the acquisition |
| Client's inventory grows from empty | Tuning payload arrives with a slot the client has as `None` | The client materializes that slot rather than skipping it |
| Level unloads while a picked-up weapon is held | Carry harvest runs before the registry clears | The weapon's canonical name and magazine carry; the instance does not |

## Direction

**Problem.** A weapon instance cannot exist in the world. The map-placement path never attaches a weapon
component — pinned by two tests — so there is nothing to pick up, and no code path anywhere grows a live
inventory: every composition site builds a fresh `Inventory` and replaces the component wholesale.

Pickup is roadmap-demanded. **Drop is not**, and its demand is co-op: a shared session where one player
can hand a weapon to another is the reason to build the inverse now rather than later. It also pays for
itself structurally — designing acquisition as an enter edge over per-item occupancy, rather than a
sustained-overlap test, is only forced by drop, and it is the better design either way. Stating this
matters because drop is what rules out the cheaper alternative below.

**Prior commitments.** `weapon-model.md` invariant 6 pins that held and dropped wieldables are the same
instance kind reachable by one spawn path, and §6 describes pickup as "the *same* instance, but placed
in the world with a transform and a trigger." §6 also pins that inventory does not own the ammunition
reserve — reserves pool on the pawn — so a wieldable leaving the inventory never takes its reserve with
it. This spec follows both: the item is the instance, and neither pickup nor drop touches `AmmoReserve`.
`entity_model.md` §7 already specifies the overlap machinery this needs — bounding sphere for pickups,
direct geometric checks, no spatial partitioning, a separate pass after entity updates — and §9 makes
spatial partitioning for entity-entity queries a non-goal. `entity_model.md` §4 pins that gameplay
tuning is descriptor-owned and never an FGD KVP, which is why the radius and mode are authored in the
data script even though the item's position is authored in the map.

Two deliberate divergences. First, the overlap pass runs after the trigger stage rather than after *all*
entity updates as §7 states, because the only entities whose motion opens or closes a pickup overlap are
player pawns and items, and items do not move — running before the AI tick keeps the acquisition edge in
the same tick as the movement that produced it. Second, `entity_model.md` §7 says volume size is fixed
per entity *type*; the radius here is per descriptor, which is the same statement in this codebase's
vocabulary since a descriptor is the type.

**Alternatives rejected.** *A pickup proxy that spawns a fresh instance on acquisition.* A separate
lightweight world entity naming an archetype, materializing a new wieldable when taken. It avoids a live
`WeaponComponent` sitting unowned in the world and keeps map placement away from the weapon column. It
was rejected because it makes drop a second mechanism rather than the inverse of pickup — a dropped
instance carries per-instance state (magazine, and later augments and charge) that a name-only proxy
cannot represent, so drop would either lose that state or need its own instance-bearing world entity,
which is the instance model arriving anyway through a worse door. It also contradicts `weapon-model.md`
invariant 6 directly. The instance model's real cost is that an unowned `WeaponComponent` exists in the
world; that cost is bounded because no weapon-stage entry point reaches weapons except through an owner,
and this spec pins that as an invariant rather than leaving it as an accident.

*Reusing brush trigger volumes.* Author a `trigger_volume` around each item and fire a reaction. It needs
no new overlap code. Rejected because brush volumes are level-load-only — the sole AABB registration site
populates from the PRL trigger-volume section — so a dropped item could never have one, and drop is in
scope. This is a divergence from the roadmap's own wording, which describes pickup as the instance
"placed in the world **with a trigger**," and it is deliberate: the roadmap line predates drop being in
scope, and a per-item sphere is what serves both. Naming it so the divergence is not read as an oversight.

**Foreclosures and one-way doors.** The `pickup` descriptor block is append-only surface; adding fields
later is cheap. The acquisition chokepoint is the one-way door: once Task 3, Task 6, and the client path
all route through it, its signature is load-bearing across three subsystems, and a later policy spec that
needs to return "refused, here is why" will widen its outcome type. That widening is anticipated — the
outcome type is an enum with named refusal reasons from the start, so a policy layer adds variants rather
than changing a boolean. The tuning-payload epoch bump is not reversible within a session: peers at
epoch 2 are refused, which is the existing and intended behavior for a semantics change.

## Acceptance criteria

- [ ] A descriptor authoring both a weapon and a pickup block, placed in a map, spawns a live entity
      carrying a weapon component and a pickup component at the placement's position.
- [ ] A descriptor authoring a weapon and no pickup block is still skipped by the map sweep when it is
      weapon-only, and still spawns without a weapon component when another component makes it placeable.
- [ ] An `auto` item is taken the first tick a player's capsule overlaps its sphere, and the item leaves
      the world in that same tick.
- [ ] An item taken from a player standing still after dropping it is not re-taken; walking out of the
      radius and back in takes it.
- [ ] Two players overlapping one item on the same tick produce exactly one acquisition, and which
      player acquires is stable across runs with identical inputs.
- [ ] A player who already owns a wieldable of the same canonical name, or whose ten slots are full, does
      not acquire the item, and the item is not reported prompt-eligible for that player.
- [ ] Growing an inventory slot outside the single chokepoint fails a source-scanning test that names its
      allowed call sites, in the shape of the existing ammo-reserve call-site test.
- [ ] Picking up or dropping a weapon leaves every ammo-reserve balance on the pawn unchanged, including
      when the weapon's ammo type is one the pawn has never carried.
- [ ] A `press` item overlapping a player is reported prompt-eligible each tick until the player presses,
      leaves, or the item is taken; pressing while overlapping two items takes exactly one.
- [ ] A weapon instance sitting in the world advances no cooldown, no reload, and no state timer over any
      number of ticks.
- [ ] A weapon picked up during a level is present in the inventory after a level transition, with its
      magazine preserved.
- [ ] In a host-plus-client session, a client standing on an item does not remove it locally; the item
      disappears only after the host's despawn arrives, and the client's inventory shows the new weapon
      only after the host reports it.
- [ ] A client whose inventory has an empty slot materializes a wieldable in that slot when the host's
      tuning payload names one, rather than skipping it.
- [ ] Pressing drop with an empty inventory, and dropping a wieldable whose descriptor authors no pickup
      block, both leave the inventory unchanged.
- [ ] A pickup radius authored as zero or a negative number warns once naming the descriptor, and that
      item is never acquirable.

## Tasks

### Task 1: Pickup descriptor, component, and map placement

Add a `PickupDescriptor { mode: PickupMode, radius: f32 }` beside `WeaponDescriptor` in
`crates/foundation/src/data_descriptors/types/combat.rs`, carrying `#[serde(rename_all = "camelCase")]`
to match its sibling. `PickupMode` is `Auto | Press` with `rename_all = "camelCase"`, wire spellings
`"auto"` and `"press"`; `mode` defaults to `Auto` and `radius` has no default. Add
`pickup: Option<PickupDescriptor>` to `EntityTypeDescriptor` in
`crates/entities/src/data_descriptors/types/entity.rs`. Read the `pickup` key under `components` in both
FFI readers — `crates/scripting-core/src/data_descriptors/lua/entity.rs` and the JS mirror in the
sibling `js/entity.rs` — following the shape the existing `weapon` arm uses in each. Register
`PickupDescriptor` and `PickupMode` in the typedef schema in
`crates/postretro/src/scripting/primitives/mod.rs` beside `WeaponDescriptor`, add `pickup` to the
`EntityTypeComponents` type, and regenerate the SDK types with
`cargo run -p postretro --bin gen-script-types`; the committed-typedef drift test fails until you do.
Validate in `PickupDescriptor::validate` that `radius` is finite and non-negative, warning once naming the
descriptor and clamping to zero otherwise. Add a runtime `PickupComponent { mode, radius }` in
`crates/entities/src/components/pickup.rs` with a `ComponentKind::Pickup` column, and a
`DescriptorComponentKind::Pickup` variant — that enum has an `ALL` const, a `component_kind()` mapping,
and a `label()` arm, all three of which are exhaustive matches that will not compile until extended.
Then make the placement change: add `|| descriptor.pickup.is_some()` to `is_directly_map_placeable` in
`crates/postretro/src/scripting/builtins/data_archetype.rs`, and change the map-sweep call site's
`attach_weapon` argument from the literal `false` to `descriptor.pickup.is_some()`. Attach the pickup
component in `attach_descriptor_components` under the same condition, recording the provenance kind.
Do not touch the `weapon` arm of `is_directly_map_placeable`: the two existing regression tests place
descriptors that author no pickup block, so both must still pass unmodified, and a test asserting that
is part of this task.

### Task 2: Inventory acquisition and release chokepoint

Add the first live-inventory read-modify-write to
`crates/postretro/src/scripting/builtins/wieldable_inventory.rs`, whose module doc currently states that
composition is the only path — update that statement in the same change rather than leaving it false.
Two functions. `acquire_wieldable(registry, pawn, item) -> AcquireOutcome` reads the pawn's `Inventory`,
resolves the item's canonical name from its `DescriptorProvenance`, and returns
`Refused(RefusalReason::AlreadyOwned)` when any occupied slot holds a wieldable whose provenance
canonical name matches, `Refused(RefusalReason::InventoryFull)` when no slot is `None`,
`Refused(RefusalReason::NotWieldable)` when the item carries no weapon component, and otherwise writes
the item id into the lowest-index free slot and returns `Acquired { slot }`. It must not modify
`active_slot` unless the inventory held no wieldable at all, in which case `active_slot` becomes the
filled slot. It must not touch `AmmoReserve`, must not touch `switch_target` or `switch_origin`, and must
not rewrite the item's `DescriptorProvenance` — cross-level carry reads the canonical name from it.
`release_wieldable(registry, pawn, slot) -> Option<EntityId>` clears the slot, returns the freed entity,
and re-picks `active_slot` as the lowest occupied slot (or leaves it at zero when the inventory is now
empty), clearing `switch_target` and `switch_origin` if either referenced the released slot. It returns
`None` and warns once per descriptor when the released wieldable's descriptor authors no pickup block,
leaving the inventory untouched. `RefusalReason` is a named enum, not a boolean, so a later policy spec
adds variants rather than changing the signature. Add a source-scanning test in the same shape as
`ammo_reserve_writes_have_only_grant_seed_and_carry_restore_non_test_call_sites` in
`crates/entities/src/components/grant.rs`: it walks `crates/`, masks `#[cfg(test)]` blocks and test
files, counts writes to `Inventory::wieldables` outside them, and asserts the allowlist is exactly the
composition site and these two functions.

### Task 3: Pickup overlap pass

Add `crates/postretro/src/sim/pickup.rs` with a `PickupSystem` holding
`occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>` — the same shape `TriggerSystem` uses, and for the
same reason: sorted keys make edge emission stable across equivalent input orderings. Its per-tick entry
point takes the registry, the same `players: &[AuthoritativePlayer]` slice and
`use_pressed: &HashMap<PlayerId, bool>` map that `TriggerTickInputs` already carries, and returns a
report of prompt-eligible `(PlayerId, EntityId)` pairs. Call it from
`crates/postretro/src/sim/mod.rs` in `simulate_tick_with_presentation_aim`, immediately after the trigger
stage's `run_authoritative_tick_with_dispatch` call and before the AI tick, threading the same inputs;
`App` owns the `PickupSystem` beside its `TriggerSystem` and clears it on level unload. Resolve player
capsules with the existing `canonical_player_capsules` helper in
`crates/postretro/src/trigger_system.rs`, which returns `(pawn, position, radius, half_height)` per
`PlayerId` — make it `pub(crate)` if it is not already. Overlap is sphere-vs-capsule: the item's
`Transform.position` against the capsule segment, true when the centre-to-segment distance is at most the
sum of the pickup radius and the capsule radius; `segment_range_distance` beside `capsule_overlaps_aabb`
is the existing distance helper to reuse or mirror. Iterate items via
`registry.iter_with_kind(ComponentKind::Pickup)`, skipping any entity already marked for end-of-frame
removal. For each item, compute the current overlapping set, diff against `occupants` to get enter and
exit edges, then act only on enter edges: in `Auto` mode call `acquire_wieldable` immediately; in `Press`
mode acquire only when that player's `use_pressed` entry is true, and otherwise report the pair as
prompt-eligible. A player pressing while overlapping several eligible items acquires only the nearest by
squared centre distance, breaking ties on the lower `EntityId`. A refusal must leave the occupancy entry
in place so the next tick does not re-fire, and must not report the pair prompt-eligible. On a successful
acquisition, remove the item's `PickupComponent`, drop its occupancy entry, and hand the item to the
netcode unregistration path from Task 5. Process players in `PlayerId` order so two simultaneous enter
edges resolve deterministically. The prompt report is an in-memory per-tick value, not a published
script or UI surface; return it and let the caller hold it. Finally, amend `context/lib/entity_model.md`
§7 in this task: Collision Timing currently states entity-entity overlap runs after all entity updates
complete, and this pass runs between the trigger and AI stages, so record the narrower placement and the
reason. In the same pass, §7 says a volume's size is fixed per entity type — restate it as per
descriptor, which is that sentence in this codebase's vocabulary. Leaving both unamended makes §7 false
after ship and misroutes the next entity-overlap spec.

### Task 4: Drop action and command plumbing

Add `Action::Drop` to `crates/postretro/src/input/types.rs` and bind it in
`crates/postretro/src/input/defaults.rs` — a keyboard key, a gamepad button, and the defaults table
entry, following the three sites `Action::Use` occupies. Read it as a rising edge at tick index zero in
`crates/postretro/src/main.rs`, in the same block that builds `use_pressed` from `Action::Use`, producing
a `HashMap<PlayerId, bool>` of drop edges for the local pawn. Add a `drop_pressed` field beside
`use_pressed` on the per-tick movement command in `crates/postretro/src/movement/mod.rs` and on the
network input command so remote players' drop presses reach the host, mirroring how `use_pressed` travels
today: it rides the per-tick input levels, is gap-held and neutralized by the command queue, and is
merged into a `PlayerId::Remote` map in `main.rs` beside the existing remote use-edge construction.
Enumerate and update every struct-literal construction site — the field is not `Option`, so the compiler
lists them. This widens the transport wire, so bump the transport's own version gate here. Do not fold it
into Task 7's `TUNING_PAYLOAD_EPOCH` change: the two gates answer different questions and a peer can fail
one while satisfying the other.

### Task 5: World-item replication

Give world items the AI-enemy replication discipline. Add
`descriptor_materializes_world_item(descriptor) -> bool`, returning `descriptor.pickup.is_some()`, beside
`descriptor_materializes_ai_enemy` in `crates/postretro/src/scripting/builtins/data_archetype.rs`, and add
it to the client-side map-sweep filter that already suppresses AI enemies there, so a connected client
does not spawn its own copy of a map-placed item. Add a host registration sweep beside
`host_register_map_enemies` in `crates/postretro/src/netcode/replication.rs` that stamps a `NetworkId` and
registers every entity carrying a `PickupComponent`, with the same stale-id unregister-and-forget prologue
so a level reload is idempotent. Call it from the host's level-install path beside the existing enemy and
mover registrations. On acquisition (Task 3) and on drop (Task 6), the item's replication membership
changes: acquiring unregisters it and forgets its allocator mapping so the client receives a despawn
tombstone, and dropping stamps and registers it so the client receives a baseline. Held wieldables are not
replicated today — the only `replicable.register` calls naming a weapon are in `netcode/lifecycle.rs` test
fixtures — so acquisition ends an item's replicated life rather than transferring it, and the client
learns the pawn's new weapon through the tuning path in Task 7. Confirm the client's apply path
materializes an item baseline from `entity_class` plus `Transform` without further metadata; that
combination is already valid on non-despawn records.

### Task 6: Drop

Add the drop path to `crates/postretro/src/sim/pickup.rs`, driven by the drop edges Task 4 produces and
evaluated in the same pass, before acquisition edges so a drop and a pickup on one tick cannot interact.
For each player with a drop edge, call `release_wieldable` from Task 2 for that pawn's active slot. On
success: write the freed item's `Transform.position` to a point in front of the pawn — derived from the
pawn's transform and capsule, at ground level, and clamped back to the pawn's own position when the
target point is not reachable through the collision world, so an item is never dropped inside geometry.
Attach a `PickupComponent` built from the descriptor's pickup block. Seed the item's occupancy entry in
`PickupSystem` with the dropping player, which is what stops the dropper from re-acquiring on the next
tick while standing on it; this is the reason acquisition is an enter edge rather than an overlap test.
Register the item for replication through Task 5's path. Force the released wieldable out of any timed
state — a weapon dropped mid-reload or mid-raise must land in the world idle, since nothing will tick it
— and reset the state timer fields alongside the state itself. The existing
`normalize_inventory_liveness` in `crates/postretro/src/sim/weapon_stage/commands.rs` handles the pawn
side of a vanished slot; releasing is not a vanish, so drop must leave the inventory consistent itself
rather than relying on that reconciliation.

### Task 7: Client inventory growth over the tuning channel

`materialize_net_local_wieldable_inventory_from_tuning` in
`crates/postretro/src/scripting/builtins/net_descriptor.rs` builds a client's local inventory only when
the pawn has none, and its slot-merge loop copies tuning fields onto slots the client has already filled,
skipping any slot the client holds as `None`. That skip is what makes a host-side acquisition invisible.
Change the merge loop so a tuning slot naming a canonical name the client's slot lacks materializes that
wieldable locally, routed through Task 2's `acquire_wieldable` at the named slot rather than through a
second composition path, and so a tuning slot that is `None` where the client holds a wieldable releases
it. Increment `TUNING_PAYLOAD_EPOCH` in `crates/postretro/src/netcode/tuning_payload.rs` from 2 to 3:
the layout is unchanged but the merge semantics are not, and the epoch gate is what stops a peer from
applying the old reading. Add a send trigger so the host publishes a fresh tuning payload when a pawn's
inventory changes; today publication fires on slot-accept and manifest refresh only, neither of which a
mid-level acquisition touches. That trigger is an escalation worth naming: it turns a
config-push-at-transition channel into an event-driven state channel with no delta and no ack, so a
dropped payload means a stale client inventory until the next publication. Accept it for this spec — the
payload is small, fixed-size, and reliable-ordered — and keep the constraint `E15` established, that the
payload stays opaque to `crates/net`. Route the client's local `Inventory` writes through Task 2's
chokepoint so the source-scan test stays satisfied.

### Task 8: Ordering and edge coverage

Build the test coverage for the Ordering matrix rows in this plan that no single task above owns end to
end, and cite the row rather than restating it. Specifically: two players entering one item's radius on
one tick producing exactly one acquisition with a stable winner; a frame rendering zero fixed ticks and a
frame rendering two; a `use` press landing on the same tick as a trigger-volume Use activation, with both
firing and neither consuming the press; a script despawn racing an acquisition on one tick; a press-mode
player overlapping two eligible items; drop followed by standing still, then by leaving and returning; a
weapon instance sitting unowned in the world across many ticks advancing no cooldown, reload, or state
timer; and a picked-up weapon surviving a level transition with its magazine. The unowned-inertness test
is the one that pins an invariant currently held only by two call sites choosing not to scan the weapon
column, so write it against observable component state after N ticks rather than against those call
sites. Host-and-client cases belong in the existing netcode harness style rather than a new fixture.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice through authoring, both FFI readers, the typedef schema, and
the map sweep. It falsifies the boundary assumptions (casing, reader mirroring, typedef drift, the
placeability arm) before anything is built on them.
**Phase 2 (concurrent):** Task 2, Task 4 — the chokepoint and the input plumbing are independent; both
consume only Task 1's component and descriptor.
**Phase 3 (sequential):** Task 3 — consumes the chokepoint from Task 2 and the drop edges from Task 4.
**Phase 4 (sequential):** Task 6 — extends the pass Task 3 creates and shares its file.
**Phase 5 (concurrent):** Task 5, Task 7 — the two netcode halves touch different files and different
wire surfaces.
**Phase 6 (sequential):** Task 8 — exercises every seam above.

## Rough sketch

`AcquireOutcome` is `Acquired { slot: usize } | Refused(RefusalReason)`, with `RefusalReason` covering
`AlreadyOwned`, `InventoryFull`, and `NotWieldable`. Keeping the refusal named rather than a `bool` or an
`Option` is what lets a later policy spec add `SwappedLowest` or `GrantedAmmoInstead` without touching
three call sites.

Duplicate detection compares `DescriptorProvenance.canonical_name` between the item and each occupied
slot's wieldable. That field is what cross-level carry already harvests, so the two agree by construction
on what "the same weapon" means.

The sphere-vs-capsule test is the same segment-distance computation `capsule_overlaps_aabb` already needs
for its AABB case; `segment_range_distance` in `crates/postretro/src/trigger_system.rs` is the helper to
lift or mirror into the pickup module.

`trigger_system.rs` is 2400 lines but only ~700 are production code — the rest is its test module. It
needs no split before this work, and this spec adds nothing to it beyond making one helper visible.
`sim/weapon_stage.rs` is likewise a 26-line facade over submodules. Neither triggers the split-first rule.

## Script syntax examples

```ts
// Proposed design — a shotgun that can be walked over.
export const shotgun: EntityTypeDescriptor = defineEntity({
  canonicalName: "weapon_shotgun",
  components: {
    weapon: {
      damage: 8, range: 1200, fireRateMs: 900,
      fireMode: "semi", resolution: "hitscan",
      resource: { kind: "ammo", type: "shells", magazine: 8, reserve: 24, reloadMs: 600, reloadStyle: "perShell" },
    },
    mesh: { model: "weapons/shotgun" },
    pickup: { mode: "auto", radius: 40 },
  },
});

// A rocket launcher the player must deliberately take.
export const rocketLauncher: EntityTypeDescriptor = defineEntity({
  canonicalName: "weapon_rocket",
  components: {
    weapon: { /* … */ },
    mesh: { model: "weapons/rocket" },
    pickup: { mode: "press", radius: 48 },
  },
});
```

A descriptor authoring `pickup` without `mesh` spawns an invisible but acquirable item. That is legal and
useful for testing, and it is an authoring concern rather than an engine rule — the engine does not
require a visual.

## Open questions

- **Drop's default key binding.** `G` is the genre convention and is unbound today, but the binding table
  is the owner's call rather than the implementer's.
- **Whether a dropped item should be reachable through the `world.query` component vocabulary.** Nothing
  in this spec needs it, and adding a component to that enum is a scripting-surface commitment. Left out;
  a mod wanting to script over world items would raise it as its own change.
- **Whether the duplicate case should grant the item's reserve rather than refuse.** Out of scope by owner
  decision. Recorded because the deferral is not a cost argument: the item's authored reserve is already
  read by the spawn path's reserve seeding, `grant_ammo` is shipped and public, and
  `E16--resource-grant-chokepoint` banked its reaction arm naming pickups as the case it serves. One
  `pickup` field — `onDuplicate: "refuse" | "grantReserve"` — covers it with no IR facts and no iteration.
  The cost of waiting is that AC 6 pins the null behavior as correct, so reversing it later means
  re-litigating an acceptance criterion that has become a regression test and changing what
  `RefusalReason::AlreadyOwned` means at three call sites.
