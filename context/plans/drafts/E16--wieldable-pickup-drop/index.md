# Wieldable Pickup and Drop

## Goal

Place a wieldable instance in the world and let a player take it into their inventory, or put one back.
The descriptor decides whether an item is taken by walking over it or by pressing the action key.
This is the first path that grows a live inventory; every prior path composes one at spawn and replaces
it wholesale.

A player's capsule meeting a takeable thing is a **touch**. The engine owns the touch and computes what
is true at it; what a touch *means* is a policy over those facts. This spec ships one engine default
policy — take it if you can — and charts the seam a later authoring surface plugs into.

## Scope

### In scope

- A `components.touchable` descriptor block that makes an otherwise-unplaceable wieldable map-placeable
  and carries its acquisition mode and radius.
- A world item that **is** the wieldable instance — same entity kind held or dropped.
- An entity-vs-player overlap pass: sphere volume per item against each player capsule, enter/exit edges.
- Two acquisition modes: `auto` (touch fires on the enter edge) and `press` (touch fires on a `use` press
  while overlapping).
- Per-touch facts, computed at every touch, in the number/bool vocabulary the IR admits.
- One decision seam: facts in, an ordered list of effects out. The engine default policy is its only
  implementation here.
- A closed touch-effect set, holding `Acquire` alone in this spec.
- Prompt-eligible pairs, returned from the pass as an in-memory per-tick value.
- Dropping the active wieldable back into the world, including the inhibit that stops the dropper from
  immediately re-acquiring it.
- One inventory-growth chokepoint, with a source-scan test restricting its call sites.
- Host-authoritative acquisition and drop in co-op, with world items replicated to clients.
- Client-side inventory growth over the existing tuning channel.

### Out of scope

- **The touch dispatch source and its authoring surface.** No `defineTouchEvent`, no IR-bound policies, no
  base+override. The facts and the decision seam land here; publishing them as a §12 dispatch scope and
  letting mods author over them is a later spec, on the `E16--impact-policy-substrate` pattern.
- **Every touch effect but `Acquire`.** Granting a duplicate's reserve, swapping the held wieldable,
  dismantling for materials — each is an arm added to the closed set by the spec that needs it.
- **Cross-slot ranking.** Replacing the lowest-stat wieldable drawing an ammo type is a scan over a
  collection, which the IR substrate forbids (`scripting.md` §11). It needs a fact family that pre-reduces
  the inventory, or an engine-owned ranking word. Neither is designed.
- **Rendering pickup prompts.** Roadmap `E16 › Combat Feedback & Economy › combat presentation substrate`
  owns floating text and pickup prompts. This spec produces the facts and stops.
- **Any state-store projection.** Inventory contents reach no slot. `SlotValue` carries no collection
  shape, and designing one belongs to the spec that needs an inventory screen.
- **Authored carry policy.** Carry across a level transition is unconditional and inherited from the
  shipped seat substrate; `E15--seat-session-identity-roster` §Out-of-scope owns the per-level-reset and
  pistol-start knobs.
- **Non-weapon touchables** — keys, currency, health and ammo items. `grantHealth`/`grantAmmo` on a
  trigger volume already covers the resource case; per-player currency is `drafts/E16--per-player-currency`.
- **Item respawn.** A taken item is gone for the level.
- **Client prediction of acquisition.** Acquisition is host-only; a client sees the item disappear after
  the round trip.
- **AI or enemy touch.** Only player pawns generate touches.
- **Automatic switch to a newly acquired wieldable**, except when the inventory held nothing.
- **Throwing.** Drop places the item; it has no velocity and no physics.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| touchable block | `EntityTypeDescriptor::touchable` | `touchable` (under `components`) | `touchable` | `touchable` | n/a — presence is authored in the data script, not the map |
| descriptor type | `TouchableDescriptor` | — | `TouchableDescriptor` | `TouchableDescriptor` | n/a |
| acquisition mode | `TouchableDescriptor::mode` | `mode` | `mode` | `mode` | n/a |
| mode: walk over | `TouchMode::Auto` | `"auto"` | `"auto"` | `"auto"` | n/a |
| mode: press to take | `TouchMode::Press` | `"press"` | `"press"` | `"press"` | n/a |
| touch radius | `TouchableDescriptor::radius` | `radius` | `radius` | `radius` | n/a — gameplay tuning is descriptor-owned (`entity_model.md` §4) |
| runtime component | `TouchableComponent` | not serialized to the wire | n/a | n/a | n/a |
| component column | `ComponentKind::Touchable` | — | — | — | — |
| provenance kind | `DescriptorComponentKind::Touchable` | `"touchable"` (snake_case, matching its siblings) | n/a | n/a | n/a |
| drop action | `Action::Drop` | — | n/a | n/a | n/a |
| map placement | existing `classname` == descriptor `canonicalName` | — | — | — | author's own `@PointClass` in their FGD |

`WeaponDescriptor` carries `#[serde(rename_all = "camelCase")]`; `TouchableDescriptor` follows it. Both
new field names are single words, so the two casings coincide. The attribute still goes on — the sibling
convention requires it, and the first multi-word field will need it.

The FGD is hand-authored and committed (`sdk/TrenchBroom/postretro.fgd`), not generated from the
descriptor registry. A modder placing a touchable adds a `@PointClass` whose classname literal equals the
descriptor's `canonicalName`, as for any other placeable archetype. This spec adds no FGD keys: mode and
radius are gameplay tuning, and tuning is descriptor-owned.

`TouchFacts` and `TouchEffect` are engine-internal in this spec. Their names become a cross-boundary
commitment when the dispatch source publishes them, so the fact spellings below are chosen for that.

## Wire format

No new binary or PRL section. Three existing wire surfaces change, gated separately — the tuning-payload
epoch and the transport wire version are different compatibility promises and must not be bumped as one.

**Tuning payload.** `TuningPayload` carries `wieldables: [Option<WieldableTuningPayload>; 10]` at epoch 2.
The layout does not change; its *merge semantics* do, so the epoch increments to 3 and peers at the old
epoch are refused by the existing gate. Field order, integer widths, and the empty-slot encoding
(`None`) are unchanged — an added slot is an existing `Some(...)` in a position that previously could
only be skipped.

**Entity replication.** A world item replicates through the existing `FullBaseline` / `Delta` / `Despawn`
records with a `Transform` payload and an `entity_class` descriptor name. `entity_class` is already valid
on any non-despawn record carrying a finite `Transform` (`networking.md` §Snapshot apply ordering), so
no metadata gate widens. No new `ComponentKind` reaches the wire: `TouchableComponent` is host-local, and
the client re-derives an item's mode from the descriptor its `entity_class` names.

**Per-tick input command.** `drop_pressed` joins `use_pressed` on the per-tick input levels. This widens
the transport wire, whose version gate is independent of the tuning epoch above; bump it on its own. The
field follows `use_pressed` exactly — gap-held, neutralized, and catch-up-trimmed by the command queue —
so it inherits the existing gap policy rather than defining one.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| An item is acquired by at most one player, ever | Task 3 (acquisition removes the item from the world in the same pass that decides it) | Two players' enter edges on one tick; a second enter edge after an empty effect list | AC 5, AC 6 |
| A touch fires on an enter edge, never on sustained overlap | Task 3 (per-item occupancy set, enter transition only) | Drop seeds the dropper into the occupancy set (Task 6); an empty effect list must not clear occupancy, or the next tick re-fires | AC 4, AC 10 |
| Facts are computed at every touch, whatever the policy reads | Task 3 (`TouchFacts` built before the policy runs) | A later short-circuit that skips fact computation when the default ignores a field | AC 7 |
| The policy decides; the chokepoint mutates | Task 2 (chokepoint holds no eligibility rule), Task 3 (policy holds no registry write) | Any refusal rule migrating back into `acquire_wieldable` | AC 6, AC 7 |
| Prompt-eligible means the policy returns at least one effect | Task 3 | A later effect arm that should prompt but is filtered by an acquisition-specific check | AC 10 |
| Pickup and drop never write `AmmoReserve` | Task 2 (the chokepoint takes no reserve argument) | The existing source-scan test allowlists exactly two non-test `credit`/`set_exact` call sites; a new one fails it | AC 9 |
| Inventory slot growth happens in exactly one place | Task 2 (chokepoint + its own source-scan test) | Task 3 and Task 6 both mutate inventories and must route through it; Task 7's client path must too | AC 8 |
| A world item is never ticked by the weapon stage | Already true — both weapon-stage entry points reach weapons through `Inventory` or `RemotePawnCommand.weapon`, and no production code scans the weapon column | A future scan-based stage would break it silently | AC 11 |
| A descriptor authoring no touchable block is not map-placeable by virtue of its weapon | Task 1 (the placeability arm keys on `touchable`, not `weapon`) | Any later widening of the weapon arm | AC 2 |
| Acquisition and drop are host decisions | Task 5, Task 6 | A client must not predict either; its inventory changes only on host word | AC 13 |
| A picked-up wieldable carries across a level transition | Inherited — carry harvests each slot's `DescriptorProvenance.canonical_name`, which a map-placed item carries like any descriptor spawn | Task 2 must not clear or rewrite provenance when a slot is filled | AC 12 |

## Ordering matrix

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Two players enter one item's radius on the same tick | Both enter edges evaluated in one pass | The player earlier in the stable `PlayerId` order acquires; the second finds the item already taken and does not acquire. Deterministic, not first-mover-by-float-distance. |
| Player drops an item and stands still | Drop seeds the dropper into the item's occupancy set; next tick sees sustained overlap | No touch fires, in either mode |
| Player drops, walks out of radius, walks back (auto) | Exit edge clears occupancy; later enter edge fires | Re-acquired |
| Player overlaps two press-mode items and presses once | One pass, one `use_pressed` edge, two overlapping items | The nearer item by squared center distance is acquired; the other is untouched. Ties break on the lower `EntityId`. |
| Player overlaps a press-mode item and never presses | Enter edge, then sustained overlap | Prompt-eligible every tick while overlapping; no touch fires |
| Player overlaps an item they already own (either mode) | Touch fires; facts carry `ownedCount = 1`; default policy returns no effects | No acquisition, and not prompt-eligible. A later policy reading the same facts may return effects here, which makes it prompt-eligible with no change to the pass. |
| Inventory full | Touch fires; facts carry `freeSlots = 0`; default policy returns no effects | Same as duplicate |
| `use` press lands on the same tick as a trigger-volume Use activation | Trigger stage runs first, touch pass second, both read the same `use_pressed` map | Both fire. The press is not consumed by either. |
| A script despawns an item on the tick a player enters its radius | Touch pass runs inside the fixed tick; the removal pass runs at end of frame | Acquisition wins that tick. An item already marked for end-of-frame removal generates no touch. |
| Frame renders zero fixed ticks | Pass lives inside the tick loop | No touch evaluation, no prompt refresh; last published prompt state persists for that frame |
| Frame renders two fixed ticks | Pass runs twice | The enter edge fires on the first; the second sees sustained overlap and does nothing |
| Radius authored zero or negative | Validation at descriptor load | Warn once naming the descriptor, clamp to zero; the item spawns and never generates a touch |
| Drop with an empty inventory | No active wieldable | No-op, no warning — pressing drop with nothing held is ordinary input |
| Drop of a wieldable whose descriptor has no touchable block | Drop refused at the chokepoint | Nothing leaves the inventory; warn once per descriptor. An item with no touchable block could never be recovered. |
| Host acquires an item a client is standing on | Host decides, despawn tombstone replicates | The client's item disappears one round trip later; the client never predicts the acquisition |
| Client's inventory grows from empty | Tuning payload arrives with a slot the client has as `None` | The client materializes that slot rather than skipping it |
| Level unloads while a picked-up weapon is held | Carry harvest runs before the registry clears | The weapon's canonical name and magazine carry; the instance does not |

## Direction

**Problem.** A weapon instance cannot exist in the world. The map-placement path never attaches a weapon
component — pinned by two tests — so there is nothing to pick up, and no code path anywhere grows a live
inventory: every composition site builds a fresh `Inventory` and replaces the component wholesale.

Pickup is roadmap-demanded. **Drop is not.** Its demand is co-op: one player handing a weapon to another
in a shared session. Drop also shapes the design — a touch is an enter edge over per-item occupancy
rather than a sustained-overlap test, and only drop forces that. It is the better shape either way, and
it is what rules out the cheaper alternative below.

**Touch is a chokepoint, and this spec charts it.** `E16--impact-policy-substrate` established the
pattern: the engine owns the single point where damage lands, publishes structured number/bool facts and
opaque command-target tokens, and holds no opinion about what a hit means. Death is a policy layered on
top, not an engine concept. Acquisition sits in the same position — walking onto a weapon means *take it*
in Quake, *take its ammo* in Doom, *swap for the held one* in Halo, and *dismantle it for parts* in a game
with crafting. Building the refusal rule into the mutation would make each of those a rewrite.

The full substrate is out of scope. What lands here is the part that is expensive to retrofit: the facts
themselves, and a decision site that takes facts and returns effects. Charting a seam and stubbing its
mechanism is the same move the impact substrate made for its app-drain producer, and for the same reason —
to avoid an API footgun when the real consumer arrives.

The facts are per-touch, not store slots. Cardinality forces it: two players touching two different items
on one tick need two different `ownedCount` values, resolved before either acts, and a slot holds one
value. `@impact.healthBefore` is a dispatch fact even though `player.health` is a store slot, for exactly
this reason.

Each fact is computed relative to the item being touched, which is what keeps them inside the IR's
number/bool vocabulary — the engine resolves *which* wieldable before publishing, so no policy ever needs
a per-weapon name namespace or an entity-typed leaf.

| Fact | Type | Meaning |
|---|---|---|
| `ownedCount` | number | inventory slots holding this item's canonical name |
| `freeSlots` | number | empty inventory slots |
| `magazine` | number | the touched instance's loaded rounds |
| `reserve` | number | the taker's current balance of this item's ammo type, zero when it has none |
| `pressed` | bool | the touch came from an action press rather than a walkover |

`reserve` is a balance, not headroom: `AmmoReserve` has no per-type cap, so remaining capacity is not a
value the engine can compute.

The default policy is the whole of this spec's behavior: acquire when `ownedCount` is zero and
`freeSlots` is positive, otherwise do nothing. Later specs add arms to the effect set and read the same
facts — `ownedCount > 0 → grantAmmo, despawn` is Doom, `freeSlots == 0 → drop active, acquire` is Halo,
`ownedCount > 0 → slot.add(materials), despawn` is dismantle-for-crafting.

**Prior commitments.** `weapon-model.md` invariant 6 pins held and dropped wieldables as the same
instance kind, reachable by one spawn path; §6 describes pickup as "the *same* instance, but placed in
the world with a transform and a trigger." §6 also pins that inventory does not own the ammunition
reserve — reserves pool on the pawn — so a wieldable leaving the inventory never takes its reserve with
it. This spec follows both: the item is the instance, and neither pickup nor drop touches `AmmoReserve`.

`entity_model.md` §7 already specifies the overlap machinery this needs — bounding sphere for pickups,
direct geometric checks, no spatial partitioning, a separate pass after entity updates — and §9 makes
spatial partitioning for entity-entity queries a non-goal. §4 pins gameplay tuning as descriptor-owned
and never an FGD KVP. That is why radius and mode are authored in the data script while the item's
position is authored in the map.

Two deliberate divergences, both from `entity_model.md` §7, and Task 3 amends the doc for both. First,
the overlap pass runs after the trigger stage rather than after *all* entity updates. Only player pawns
and items can open or close an overlap, and items do not move; running before the AI tick keeps the touch
in the tick that produced it. Second, §7 fixes volume size per entity *type*. The radius here is per
descriptor — the same rule in this codebase's vocabulary, since a descriptor is the type.

**Alternatives rejected.** *A pickup proxy that spawns a fresh instance on acquisition.* A lightweight
world entity naming an archetype, materializing a new wieldable when taken. It keeps map placement away
from the weapon column and leaves no live `WeaponComponent` unowned in the world.

Rejected because it makes drop a second mechanism rather than the inverse of pickup. A dropped instance
carries per-instance state — magazine now, augments and charge later — that a name-only proxy cannot
represent. Drop would either lose that state or grow its own instance-bearing world entity, which is the
instance model arriving through a worse door. It also contradicts `weapon-model.md` invariant 6 directly.
The instance model's cost is the unowned `WeaponComponent`, and that cost is bounded: no weapon-stage
entry point reaches a weapon except through its owner, pinned here as an invariant.

*Reusing brush trigger volumes.* Author a `trigger_volume` around each item and fire a reaction. No new
overlap code. Rejected because brush volumes are level-load-only — the sole AABB registration site
populates from the PRL trigger-volume section — so a dropped item can never have one.

This diverges from the roadmap, which describes pickup as the instance "placed in the world **with a
trigger**." That line predates drop entering scope; a per-item sphere serves both.

*Shipping the refusal rule inside the chokepoint.* Fewer moving parts, one less type, and the default
policy is the only caller today. Rejected because the refusal rule is the one part of this spec that
every influential shooter answers differently, and burying it in the mutation means the second answer is
a rewrite of the first. The seam costs one function and one enum.

**Foreclosures and one-way doors.** The `touchable` descriptor block is append-only surface. The fact
names are the durable commitment: they become wire spellings when the dispatch source publishes them, and
`E16--impact-policy-substrate` records what a rename costs — `resolve_input` fails silently on an unknown
name. The effect set is closed and extensible, so a later spec adds an arm rather than widening a type.
The tuning-payload epoch bump is not reversible within a session: peers at epoch 2 are refused, which is
the existing and intended behavior for a semantics change.

## Acceptance criteria

- [ ] A descriptor authoring both a weapon and a touchable block, placed in a map, spawns a live entity
      carrying a weapon component and a touchable component at the placement's position.
- [ ] A descriptor authoring a weapon and no touchable block is still skipped by the map sweep when it is
      weapon-only, and still spawns without a weapon component when another component makes it placeable.
- [ ] An `auto` item is taken the first tick a player's capsule overlaps its sphere, and the item leaves
      the world in that same tick.
- [ ] An item is not re-taken by a player standing still after dropping it; walking out of the radius and
      back in takes it.
- [ ] Two players overlapping one item on the same tick produce exactly one acquisition, and which
      player acquires is stable across runs with identical inputs.
- [ ] A player who already owns a wieldable of the same canonical name, or whose ten slots are full, does
      not acquire the item, and the item is not reported prompt-eligible for that player.
- [ ] Every touch carries facts matching the world it fired in — owned count, free slots, magazine,
      reserve, and press origin — whether or not the acting policy reads them. Substituting a policy that
      acquires unconditionally acquires a duplicate, with no other change.
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
- [ ] Pressing drop with an empty inventory, and dropping a wieldable whose descriptor authors no
      touchable block, both leave the inventory unchanged.
- [ ] A touch radius authored as zero or a negative number warns once naming the descriptor, and that
      item never generates a touch.

## Tasks

### Task 1: Touchable descriptor, component, and map placement

Add a `TouchableDescriptor { mode: TouchMode, radius: f32 }` beside `WeaponDescriptor` in
`crates/foundation/src/data_descriptors/types/combat.rs`, carrying `#[serde(rename_all = "camelCase")]`
to match its sibling. `TouchMode` is `Auto | Press` with `rename_all = "camelCase"`, wire spellings
`"auto"` and `"press"`; `mode` defaults to `Auto` and `radius` has no default. Add
`touchable: Option<TouchableDescriptor>` to `EntityTypeDescriptor` in
`crates/entities/src/data_descriptors/types/entity.rs`. Read the `touchable` key under `components` in
both FFI readers — `crates/scripting-core/src/data_descriptors/lua/entity.rs` and the JS mirror in the
sibling `js/entity.rs` — following the shape the existing `weapon` arm uses in each. Register
`TouchableDescriptor` and `TouchMode` in the typedef schema in
`crates/postretro/src/scripting/primitives/mod.rs` beside `WeaponDescriptor`, add `touchable` to the
`EntityTypeComponents` type, and regenerate the SDK types with
`cargo run -p postretro --bin gen-script-types`; the committed-typedef drift test fails until you do.
Validate in `TouchableDescriptor::validate` that `radius` is finite and non-negative, warning once naming
the descriptor and clamping to zero otherwise. Add a runtime `TouchableComponent { mode, radius }` in
`crates/entities/src/components/touchable.rs` with a `ComponentKind::Touchable` column, and a
`DescriptorComponentKind::Touchable` variant — that enum has an `ALL` const, a `component_kind()`
mapping, and a `label()` arm, all three of which are exhaustive matches that will not compile until
extended. Then make the placement change: add `|| descriptor.touchable.is_some()` to
`is_directly_map_placeable` in `crates/postretro/src/scripting/builtins/data_archetype.rs`, and change
the map-sweep call site's `attach_weapon` argument from the literal `false` to
`descriptor.touchable.is_some()`. Attach the touchable component in `attach_descriptor_components` under
the same condition, recording the provenance kind. Do not touch the `weapon` arm of
`is_directly_map_placeable`: the two existing regression tests place descriptors that author no touchable
block, so both must still pass unmodified, and a test asserting that is part of this task.

### Task 2: Inventory acquisition and release chokepoint

Add the first live-inventory read-modify-write to
`crates/postretro/src/scripting/builtins/wieldable_inventory.rs`, whose module doc currently states that
composition is the only path — update that statement in the same change rather than leaving it false.
Two functions, both pure effects that hold no eligibility rule: whether an acquisition *should* happen is
decided in Task 3 and is not this layer's concern.

`acquire_wieldable(registry, pawn, item) -> Option<usize>` writes the item id into the lowest-index free
slot and returns that slot, or returns `None` when no slot is free or the item carries no weapon
component. It must not modify `active_slot` unless the inventory held no wieldable at all, in which case
`active_slot` becomes the filled slot. It must not touch `AmmoReserve`, must not touch `switch_target` or
`switch_origin`, and must not rewrite the item's `DescriptorProvenance` — cross-level carry reads the
canonical name from it. It must not reject a duplicate: a policy that wants a second shotgun in slot 4
gets one.

`release_wieldable(registry, pawn, slot) -> Option<EntityId>` clears the slot, returns the freed entity,
and re-picks `active_slot` as the lowest occupied slot (or leaves it at zero when the inventory is now
empty), clearing `switch_target` and `switch_origin` if either referenced the released slot. It returns
`None` and warns once per descriptor when the released wieldable's descriptor authors no touchable block,
leaving the inventory untouched.

Add a source-scanning test in the same shape as
`ammo_reserve_writes_have_only_grant_seed_and_carry_restore_non_test_call_sites` in
`crates/entities/src/components/grant.rs`: it walks `crates/`, masks `#[cfg(test)]` blocks and test
files, counts writes to `Inventory::wieldables` outside them, and asserts the allowlist is exactly the
composition site and these two functions.

### Task 3: Touch pass, facts, and default policy

Add `crates/postretro/src/sim/touch.rs` with a `TouchSystem` holding
`occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>` — the same shape `TriggerSystem` uses, and for the
same reason: sorted keys make edge emission stable across equivalent input orderings. Its per-tick entry
point takes the registry, the same `players: &[AuthoritativePlayer]` slice and
`use_pressed: &HashMap<PlayerId, bool>` map that `TriggerTickInputs` already carries, and returns a
report of prompt-eligible `(PlayerId, EntityId)` pairs. Call it from `crates/postretro/src/sim/mod.rs` in
`simulate_tick_with_presentation_aim`, immediately after the trigger stage's
`run_authoritative_tick_with_dispatch` call and before the AI tick, threading the same inputs; `App` owns
the `TouchSystem` beside its `TriggerSystem` and clears it on level unload.

Resolve player capsules with the existing `canonical_player_capsules` helper in
`crates/postretro/src/trigger_system.rs`, which returns `(pawn, position, radius, half_height)` per
`PlayerId` — make it `pub(crate)` if it is not already. Overlap is sphere-vs-capsule: the item's
`Transform.position` against the capsule segment, true when the center-to-segment distance is at most the
sum of the touch radius and the capsule radius; `segment_range_distance` beside `capsule_overlaps_aabb`
is the existing distance helper to reuse or mirror. Iterate items via
`registry.iter_with_kind(ComponentKind::Touchable)`, skipping any entity already marked for end-of-frame
removal. For each item, compute the current overlapping set and diff against `occupants` to get enter and
exit edges. A touch fires on an enter edge in `Auto` mode, and on a `use_pressed` edge while overlapping
in `Press` mode.

Every touch builds a `TouchFacts { owned_count, free_slots, magazine, reserve, pressed }` before any
decision runs — `owned_count` counts occupied slots whose wieldable's `DescriptorProvenance.canonical_name`
matches the item's, `reserve` reads the pawn's `AmmoReserve` balance for the item's ammo type and is zero
when the weapon authors no ammo resource. Build the facts whether or not the acting policy reads them.

Pass the facts to `default_touch_policy(facts) -> Vec<TouchEffect>`, the single decision site. It returns
`vec![TouchEffect::Acquire]` when `owned_count == 0 && free_slots > 0`, and an empty vector otherwise.
`TouchEffect` is a closed enum holding `Acquire` alone. Apply the returned effects in order; `Acquire`
calls `acquire_wieldable` from Task 2. An empty list leaves the occupancy entry in place so the next tick
does not re-fire. A pair is prompt-eligible when the policy returns a non-empty list and the item was not
acquired this tick — never by an acquisition-specific test, so an added effect arm prompts without
touching the pass.

A player pressing while overlapping several items acquires only the nearest by squared center distance,
breaking ties on the lower `EntityId`. On a successful acquisition, remove the item's `TouchableComponent`,
drop its occupancy entry, and hand the item to the netcode unregistration path from Task 5. Process
players in `PlayerId` order so two simultaneous enter edges resolve deterministically. Return the prompt
report as a plain per-tick value and let the caller hold it; it is not a published script or UI surface.

Amend `context/lib/entity_model.md` §7 in this task, in two places. Collision Timing states that
entity-entity overlap runs after all entity updates complete; this pass runs between the trigger and AI
stages, so record the narrower placement and why it is sufficient. §7 also fixes volume size per entity
type; restate it as per descriptor. Both sentences are false once this ships, and the next
entity-overlap spec reads them.

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
`descriptor_materializes_world_item(descriptor) -> bool`, returning `descriptor.touchable.is_some()`,
beside `descriptor_materializes_ai_enemy` in
`crates/postretro/src/scripting/builtins/data_archetype.rs`, and add it to the client-side map-sweep
filter that already suppresses AI enemies there, so a connected client does not spawn its own copy of a
map-placed item. Add a host registration sweep beside `host_register_map_enemies` in
`crates/postretro/src/netcode/replication.rs` that stamps a `NetworkId` and registers every entity
carrying a `TouchableComponent`, with the same stale-id unregister-and-forget prologue so a level reload
is idempotent. Call it from the host's level-install path beside the existing enemy and mover
registrations. On acquisition (Task 3) and on drop (Task 6), the item's replication membership changes:
acquiring unregisters it and forgets its allocator mapping so the client receives a despawn tombstone,
and dropping stamps and registers it so the client receives a baseline. Held wieldables are not
replicated today — the only `replicable.register` calls naming a weapon are in `netcode/lifecycle.rs`
test fixtures — so acquisition ends an item's replicated life rather than transferring it, and the client
learns the pawn's new weapon through the tuning path in Task 7. Confirm the client's apply path
materializes an item baseline from `entity_class` plus `Transform` without further metadata; that
combination is already valid on non-despawn records.

### Task 6: Drop

Add the drop path to `crates/postretro/src/sim/touch.rs`, driven by the drop edges Task 4 produces and
evaluated in the same pass, before any touch edges so a drop and an acquisition on one tick cannot
interact. For each player with a drop edge, call `release_wieldable` from Task 2 for that pawn's active
slot. On success: write the freed item's `Transform.position` to a point in front of the pawn — derived
from the pawn's transform and capsule, at ground level, and clamped back to the pawn's own position when
the target point is not reachable through the collision world, so an item is never dropped inside
geometry. Attach a `TouchableComponent` built from the descriptor's touchable block. Seed the item's
occupancy entry in `TouchSystem` with the dropping player: without it, the dropper's next tick is an
enter edge and the item comes straight back. Register the item for replication through Task 5's path.
Force the released wieldable out of any timed state and reset the state timer fields alongside the state
— a weapon dropped mid-reload or mid-raise must land in the world idle, because nothing will tick it. The
existing `normalize_inventory_liveness` in `crates/postretro/src/sim/weapon_stage/commands.rs` handles
the pawn side of a vanished slot; releasing is not a vanish, so drop must leave the inventory consistent
itself rather than relying on that reconciliation.

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
mid-level acquisition touches. That trigger escalates the channel: a config push at transitions becomes
an event-driven state channel with no delta and no ack, so a dropped payload leaves a stale client
inventory until the next publication. Accept it here — the payload is small, fixed-size, and
reliable-ordered — and hold the constraint `E15` set, that it stays opaque to `crates/net`. Route the
client's local `Inventory` writes through Task 2's chokepoint so the source-scan test stays satisfied.

### Task 8: Ordering and edge coverage

Build the test coverage for the Ordering matrix rows in this plan that no single task above owns end to
end, and cite the row rather than restating it. Specifically: two players entering one item's radius on
one tick producing exactly one acquisition with a stable winner; a frame rendering zero fixed ticks and a
frame rendering two; a `use` press landing on the same tick as a trigger-volume Use activation, with both
firing and neither consuming the press; a script despawn racing an acquisition on one tick; a press-mode
player overlapping two items; drop followed by standing still, then by leaving and returning; a weapon
instance sitting unowned in the world across many ticks advancing no cooldown, reload, or state timer;
and a picked-up weapon surviving a level transition with its magazine.

Two tests carry more than their row. Write the unowned-inertness test against observable component state
after N ticks, never against the call sites — the invariant it pins is currently held only by two call
sites choosing not to scan the weapon column, and a test naming them would move with them. Cover the
facts by substituting a policy that acquires unconditionally and asserting a duplicate lands in the next
free slot: it proves the facts are built independently of what the default reads, which no test of the
default alone can show. Host-and-client cases belong in the existing netcode harness style rather than a
new fixture.

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

The decision seam is three declarations:

```rust
// Proposed design — engine-internal until a dispatch source publishes them.
struct TouchFacts { owned_count: u32, free_slots: u32, magazine: u32, reserve: u32, pressed: bool }
enum TouchEffect { Acquire }
fn default_touch_policy(facts: &TouchFacts) -> Vec<TouchEffect>;
```

The list return is load-bearing. Every policy past the first emits more than one effect — Doom grants
ammo *and* despawns, Halo drops *and* acquires — so an `Option<TouchEffect>` would change shape at the
second consumer.

Duplicate detection compares `DescriptorProvenance.canonical_name` between the item and each occupied
slot's wieldable. That field is what cross-level carry already harvests, so the two agree by construction
on what "the same weapon" means.

The sphere-vs-capsule test is the same segment-distance computation `capsule_overlaps_aabb` already needs
for its AABB case; `segment_range_distance` in `crates/postretro/src/trigger_system.rs` is the helper to
lift or mirror into the touch module.

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
    touchable: { mode: "auto", radius: 40 },
  },
});

// A rocket launcher the player must deliberately take.
export const rocketLauncher: EntityTypeDescriptor = defineEntity({
  canonicalName: "weapon_rocket",
  components: {
    weapon: { /* … */ },
    mesh: { model: "weapons/rocket" },
    touchable: { mode: "press", radius: 48 },
  },
});
```

A descriptor authoring `touchable` without `mesh` spawns an invisible but acquirable item. Legal, and
useful for testing. The engine requires no visual; that is the author's call.

## Open questions

- **Drop's default key binding.** `G` is the genre convention and is unbound today, but the binding table
  is the owner's call rather than the implementer's.
- **Whether a dropped item should be reachable through the `world.query` component vocabulary.** Nothing
  in this spec needs it, and adding a component to that enum is a scripting-surface commitment. Left out;
  a mod wanting to script over world items would raise it as its own change.
