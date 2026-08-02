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
- Two acquisition modes: `auto` (a touch fires on the enter edge) and `press` (a touch fires on a `use`
  press while overlapping).
- Per-touch facts, computed at every evaluation, in the number/bool vocabulary the IR admits.
- One decision seam: facts in, an ordered list of effects out. The engine default policy is its only
  implementation here.
- A closed touch-effect set, holding `Acquire` alone in this spec.
- Prompt-eligible pairs, returned from the pass as an in-memory per-tick value.
- Dropping the active wieldable back into the world, including the inhibit that stops the drop point's
  occupants from immediately acquiring it.
- One inventory-growth chokepoint, with a source-scan test restricting its call sites.
- Host-authoritative acquisition and drop in co-op, with world items replicated to clients.
- Client-side inventory growth and release over the existing tuning channel.

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
- **An edge-recovery lane for the drop press.** A drop press discarded by the input catch-up trim is lost,
  matching `use`. Reload's dedicated pre-trim edge lane is not extended here.

## Evaluation model

Three distinct things happen per overlapping pair, and conflating them is the defect this section exists
to prevent.

| Step | When | Effect |
|---|---|---|
| **Overlap** | Geometric state, recomputed every tick | Maintains the per-item occupancy set; produces enter and exit edges |
| **Evaluation** | Every tick, for every overlapping `(player, item)` pair | Builds `TouchFacts` and runs the policy. Result drives prompt-eligibility only |
| **Touch** | Enter edge in `auto`; `use_pressed` edge while overlapping in `press` | Re-evaluates, then **applies** the returned effects |

A pair is prompt-eligible when its evaluation returns a non-empty effect list and no effects were applied
for it this tick. So an `auto` item is never left prompt-eligible: if its policy returns effects, the
enter edge already applied them.

Facts are built at evaluation, not only at touch. That is what lets a `press` item report eligibility
before any press occurs, and it is why the policy is required to be pure — it runs on ticks where nothing
is applied.

`pressed` carries the player's real `use_pressed` state at each evaluation, never a synthesized value. A
policy gating on it therefore reports eligibility only on the tick the player is actually pressing, which
is correct: such a policy is describing an effect that only a press produces.

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
a client needs no mode — it never runs the touch pass and attaches presentation only.

**Per-tick input command.** `drop_pressed` joins `use_pressed` on the per-tick input levels. This widens
the transport wire, whose version gate is independent of the tuning epoch above; bump it on its own.
`drop_pressed` is an **edge bit** and follows `use_pressed`'s treatment exactly: `held_gap_sim_command`
clears it on every held tick and `neutral_sim_command` synthesizes it false, so a packet gap replays no
drop. It is **not** carried forward like `reload`, which is a level bit deduplicated by weapon-owned
state. Carrying an edge bit across a gap would drop one weapon per held tick from a single press.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| An item is acquired by at most one player, ever | Task 3 (acquisition removes the item from the world in the same pass that decides it) | Two players' enter edges on one tick; a player entering after another's empty effect list | AC 5 |
| Effects apply only on a touch; evaluation alone never mutates | Task 3 (evaluation and application are separate steps) | A later effect arm applied from the eligibility path | AC 7, AC 10 |
| A touch fires on an edge, never on sustained overlap | Task 3 (per-item occupancy set, enter transition in `auto`; press edge in `press`) | Drop seeds the drop point's occupants (Task 6); an empty effect list must not clear occupancy, or the next tick re-fires | AC 4, AC 10 |
| Facts are computed at every evaluation, whatever the policy reads | Task 3 (`TouchFacts` built before the policy runs) | A later short-circuit that skips fact computation when the default ignores a field | AC 7 |
| The policy decides; the chokepoint mutates | Task 2 (chokepoint holds no eligibility rule), Task 3 (policy holds no registry write) | Any refusal rule migrating back into the chokepoint | AC 6, AC 7 |
| Prompt-eligible means the policy returned effects that were not applied | Task 3 | A later effect arm filtered by an acquisition-specific check | AC 10 |
| Pickup and drop never write `AmmoReserve` | Task 2 (the chokepoint takes no reserve argument) | The existing source-scan test allowlists exactly two non-test `credit`/`set_exact` call sites; a new one fails it | AC 9 |
| Inventory slot **fill** happens in exactly one place | Task 2 (chokepoint + its own source-scan test) | Task 3, Task 6, and Task 7 all mutate inventories and must route through it | AC 8 |
| A world item is never ticked by the weapon stage | Already true — both weapon-stage entry points reach weapons through `Inventory` or `RemotePawnCommand.weapon`, and no production code scans the weapon column | A future scan-based stage would break it silently | AC 11 |
| A descriptor authoring no touchable block is not map-placeable by virtue of its weapon | Task 1 (the placeability predicate gains a `touchable` term and no `weapon` term) | Any later addition of a `weapon` term | AC 2 |
| Acquisition and drop are host decisions | Task 5, Task 6 | A client must not predict either; its inventory changes only on host word | AC 13 |
| A picked-up wieldable carries across a level transition | Inherited — carry harvests each slot's `DescriptorProvenance.canonical_name`, which a map-placed item carries like any descriptor spawn | Task 2 must not clear or rewrite provenance when a slot is filled | AC 12 |

## Ordering matrix

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Two players enter one item's radius on the same tick | Both enter edges evaluated in one pass | The player earlier in the stable `PlayerId` order acquires; the second finds the item already taken and does not acquire. Deterministic, not first-mover-by-float-distance. |
| One player, two `auto` items entered on one tick, one free slot | Items iterated in ascending `EntityId` order | The lower `EntityId` is acquired; the second evaluates with `freeSlots = 0` and returns no effects |
| Player drops an item and stands still | Drop seeds every player overlapping the drop point into the item's occupancy | No enter edge, so no touch in `auto`. A `press` player pressing again re-acquires — a deliberate press defeats the inhibit. |
| Player drops, walks out of radius, walks back (auto) | Exit edge clears occupancy; later enter edge fires | Re-acquired |
| Player B already overlaps the point where A drops | Drop runs before touch edges; the seed covers B as well as A | B gets no enter edge on the drop tick. B acquires only after leaving and returning, or by pressing a `press` item. |
| Two adjacent players drop on the same tick | Each drop seeds every player overlapping *that* item's point, so each is seeded into both | Neither gets an enter edge next tick; no mutual weapon swap |
| Player overlaps a `press` item and never presses | Enter edge, then sustained overlap; evaluation runs every tick | Prompt-eligible every tick while the policy returns effects; no touch fires |
| Player overlaps an item they already own (either mode) | Evaluation runs; facts carry `ownedCount = 1`; default policy returns no effects | No acquisition, not prompt-eligible. A later policy reading the same facts returns effects here, which makes it prompt-eligible with no change to the pass. |
| Inventory full | Evaluation runs; facts carry `freeSlots = 0`; default policy returns no effects | Same as duplicate. Freeing a slot while standing on an `auto` item does not acquire it — there is no new enter edge — so the player steps off and back on. |
| `use` press lands on the same tick as a trigger-volume Use activation | Trigger stage runs first, touch pass second, both read the same `use_pressed` map | Both fire. The press is not consumed by either. |
| A script queues `despawn(item)` on the tick the item is acquired | The deferred effect rides the item's own queue and would execute next tick against a now-held weapon | Acquisition purges the acquired item's pending deferred effects. The weapon survives. |
| An item already marked for end-of-frame removal | Removal pass runs at end of frame | The item generates no overlap and no evaluation |
| Frame renders zero fixed ticks | Pass lives inside the tick loop | No evaluation, no prompt refresh; last published prompt state persists for that frame |
| Frame renders two fixed ticks | Pass runs twice | The enter edge fires on the first; the second sees sustained overlap and applies nothing |
| Radius authored zero or negative | Validation at descriptor load clamps to zero; the pass skips zero-radius items | Warn once naming the descriptor. The item spawns, is never evaluated, and never generates a touch — the capsule radius alone must not make it acquirable. |
| Drop press followed by a packet gap | `held_gap_sim_command` clears the edge on every held tick | Exactly one drop |
| Drop press inside a backlog that trips the catch-up trim | The trim discards the command carrying the edge | The drop is lost, matching `use`. Accepted; no edge-recovery lane. |
| Drop with an empty inventory | No active wieldable | No-op, no warning — pressing drop with nothing held is ordinary input |
| Drop of a wieldable whose descriptor has no touchable block | Drop refused at the chokepoint | Nothing leaves the inventory; warn once per descriptor. An item with no touchable block could never be recovered. |
| Player dies or disconnects while overlapping an item | The capsule lookup fails on the next tick | Treated as an exit: occupancy and any drop inhibit for that `PlayerId` clear, and eligibility ends that tick. A dropper who dies and respawns onto the item acquires it, since the inhibit died with the occupancy entry. |
| Host acquires an item a client is standing on | Host decides, despawn tombstone replicates | The client's item disappears one round trip later; the client never predicts the acquisition |
| Client receives tuning growth before the item's despawn tombstone | Control is reliable-ordered, Snapshot is unreliable; no cross-channel ordering exists | Accepted transient: the weapon is in the inventory while the item still stands in the world. Converges when the tombstone or the next baseline arrives. |
| Client receives a drop baseline before the tuning slot-`None` | Same two channels, drop direction | Accepted transient: the item stands in the world while the client still holds it. The freed local instance is despawned when the tuning apply lands. |
| Host acquisition on the final tick before a level change | Tuning publish races demotion and client unload | A payload reaching a demoted or unloading pawn is discarded. The next level's inventory comes solely from the re-promotion payload and carried state. |
| Level unloads while a picked-up weapon is held | Carry harvest runs before the registry clears | The weapon's canonical name and magazine carry; the instance does not |

## Direction

**Problem.** A weapon instance cannot exist in the world. The map-placement path never attaches a weapon
component — pinned by two tests — so there is nothing to pick up, and no code path anywhere grows a live
inventory: every composition site builds a fresh `Inventory` and replaces the component wholesale.

Pickup is roadmap-demanded. **Drop is not.** Its demand is co-op: one player handing a weapon to another
in a shared session. Drop also shapes the design — a touch is an edge over per-item occupancy rather than
a sustained-overlap test, and only drop forces that. It is the better shape either way, and it is what
rules out the cheaper alternative below.

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
| `pressed` | bool | the player's `use_pressed` state at this evaluation |

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

Numbered; the Invariants table references these numbers.

1. [ ] A descriptor authoring both a weapon and a touchable block, placed in a map, spawns a live entity
   carrying a weapon component and a touchable component at the placement's position.
2. [ ] A descriptor authoring a weapon and no touchable block is still skipped by the map sweep when it
   is weapon-only, and still spawns without a weapon component when another component makes it placeable.
3. [ ] An `auto` item is taken the first tick a player's capsule overlaps its sphere, and the item leaves
   the world in that same tick.
4. [ ] An item is not re-taken by a player standing still after dropping it, nor by another player who
   was already standing at the drop point; walking out of the radius and back in takes it.
5. [ ] Two players overlapping one item on the same tick produce exactly one acquisition, and which
   player acquires is stable across runs with identical inputs.
6. [ ] A player who already owns a wieldable of the same canonical name, or whose ten slots are full,
   does not acquire the item, and the item is not reported prompt-eligible for that player.
7. [ ] Every evaluation carries facts matching the world it ran in — owned count, free slots, magazine,
   reserve, and press state — whether or not the acting policy reads them, and on ticks where no effects
   apply. Substituting a policy that acquires unconditionally acquires a duplicate, with no other change.
8. [ ] Filling an inventory slot outside the single chokepoint fails a source-scanning test that names its
   allowed call sites, in the shape of the existing ammo-reserve call-site test.
9. [ ] Picking up or dropping a weapon leaves every ammo-reserve balance on the pawn unchanged, including
   when the weapon's ammo type is one the pawn has never carried.
10. [ ] A `press` item overlapping a player is reported prompt-eligible each tick until the player
    presses, leaves, or the item is taken; pressing while overlapping two items takes exactly one. An
    `auto` item is never reported prompt-eligible.
11. [ ] A weapon instance sitting in the world advances no cooldown, no reload, and no state timer over
    any number of ticks.
12. [ ] A weapon picked up during a level is present in the inventory after a level transition, with its
    magazine preserved.
13. [ ] In a host-plus-client session, a client standing on an item does not remove it locally; the item
    disappears only after the host's despawn arrives, and the client's inventory shows the new weapon
    only after the host reports it.
14. [ ] A client whose inventory has an empty slot materializes a wieldable in that slot when the host's
    tuning payload names one, at the slot index the host named. A tuning slot that is `None` where the
    client holds a wieldable releases and despawns the local instance.
15. [ ] A dropped weapon lands at a reachable point in front of the pawn, never inside world geometry,
    and lands idle — a weapon dropped mid-reload or mid-raise shows no residual timed state.
16. [ ] Pressing drop with an empty inventory, and dropping a wieldable whose descriptor authors no
    touchable block, both leave the inventory unchanged.
17. [ ] A touch radius authored as zero or a negative number warns once naming the descriptor, and that
    item is never evaluated and never acquirable, including by a player standing on it.
18. [ ] A single drop press produces exactly one drop across a packet gap that holds the command for
    several ticks.

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
the descriptor and clamping to zero otherwise.

Add a runtime `TouchableComponent { mode, radius }` in `crates/entities/src/components/touchable.rs` with
a `ComponentKind::Touchable` column, and a `DescriptorComponentKind::Touchable` variant. That enum's
`component_kind()` and `label()` are exhaustive matches and will not compile until extended; its
`ALL: [Self; N]` const is a fixed-length array that compiles fine while silently stale, so extend it by
hand and widen its length.

Then make the placement change in `crates/postretro/src/scripting/builtins/data_archetype.rs`: add
`|| descriptor.touchable.is_some()` to `is_directly_map_placeable`, and change the map-sweep call site's
`attach_weapon` argument from the literal `false` to `descriptor.touchable.is_some()`. Attach the
touchable component in `attach_descriptor_components` under the same condition, recording the provenance
kind. `is_directly_map_placeable` lists `light || emitter || movement || mesh || health` and excludes
weapon **by omission** — do not add a `weapon` term; that exclusion is the shipped guarantee. The two
existing regression tests place descriptors that author no touchable block, so both must still pass
unmodified, and a test asserting that is part of this task.

### Task 2: Inventory fill and release chokepoint

Add the first live-inventory read-modify-write to
`crates/postretro/src/scripting/builtins/wieldable_inventory.rs`. The "this is the only composition path"
sentence is the doc comment on `compose_wieldable_inventory`, not the module doc — update that function's
doc in the same change rather than leaving it false.

Three functions, all pure effects that hold no eligibility rule: whether an acquisition *should* happen is
decided in Task 3 and is not this layer's concern.

`acquire_wieldable_at(registry, pawn, slot, item) -> bool` is the primitive. It writes the item id into
the named slot and returns whether it did, returning `false` when the slot is occupied, out of range, or
the item carries no weapon component. `acquire_wieldable(registry, pawn, item) -> Option<usize>` is the
lowest-index-free-slot wrapper over it, returning the filled slot. Task 7's client path needs the
slot-targeted form: the host names a slot index, and picking the client's own lowest free slot would
desync the two inventories by index.

Neither must modify `active_slot` unless the inventory held no wieldable at all, in which case
`active_slot` becomes the filled slot. Neither must touch `AmmoReserve`, `switch_target`, or
`switch_origin`, and neither must rewrite the item's `DescriptorProvenance` — cross-level carry reads the
canonical name from it. Neither may reject a duplicate: a policy that wants a second shotgun in slot 4
gets one.

`release_wieldable(registry, pawn, slot) -> Option<EntityId>` clears the slot, returns the freed entity,
and re-picks `active_slot` as the lowest occupied slot (or leaves it at zero when the inventory is now
empty), clearing `switch_target` and `switch_origin` if either referenced the released slot. It returns
`None` and warns once per descriptor when the released wieldable's descriptor authors no touchable block,
leaving the inventory untouched.

Add a source-scanning test in the same shape as
`ammo_reserve_writes_have_only_grant_seed_and_carry_restore_non_test_call_sites` in
`crates/entities/src/components/grant.rs`: walk `crates/`, mask `#[cfg(test)]` blocks and test files, and
count **slot-fill** writes to `Inventory::wieldables` — assignments of `Some(...)` — outside them. Scan
fills only, not clears: `normalize_inventory_liveness` in
`crates/postretro/src/sim/weapon_stage/commands.rs` nulls slots in production and is not a growth path.
The allowlist is exactly the composition site and the two acquire functions.

### Task 3: Touch pass, facts, and default policy

Add `crates/postretro/src/sim/touch.rs` with a `TouchSystem` holding
`occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>` and its own
`warned_duplicate_players: HashSet<PlayerId>` — the same shape `TriggerSystem` uses, and for the same
reason: sorted keys make edge emission stable across equivalent input orderings.

Its per-tick entry point takes `&mut EntityRegistry`, `&CollisionWorld` (Task 6's drop placement needs
it), the same `players: &[AuthoritativePlayer]` slice and `use_pressed: &HashMap<PlayerId, bool>` map
that `TriggerTickInputs` already carries, and a `drop_pressed: &HashMap<PlayerId, bool>` map from Task 4.
It returns a `TouchReport { prompts: Vec<(PlayerId, EntityId)>, acquired: Vec<EntityId>, dropped:
Vec<EntityId> }`. The pass performs no replication bookkeeping itself — it has no netcode access by
design; the caller in `crates/postretro/src/sim/mod.rs` forwards `acquired` and `dropped` to Task 5's
register/unregister path. Call it from `simulate_tick_with_presentation_aim` immediately after the trigger
stage's `run_authoritative_tick_with_dispatch` call and before the AI tick; `App` owns the `TouchSystem`
beside its `TriggerSystem` and clears it on level unload.

Resolve player capsules with the existing `canonical_player_capsules` helper in
`crates/postretro/src/trigger_system.rs` — make it `pub(crate)` if it is not already. It takes a third
argument, `warned_duplicate_players: &mut HashSet<PlayerId>`, which is why `TouchSystem` carries its own.
It returns `(pawn, position, radius, half_height)` per `PlayerId`; a `PlayerId` absent from the returned
map is treated as exited, clearing its occupancy entries, exactly as the trigger stage treats a vanished
player.

Overlap is sphere-vs-capsule: horizontal distance from the item's `Transform.position` to the capsule
axis, and `range_distance(item.y, capsule_min_y, capsule_max_y)` on the vertical — the point-vs-range
helper beside `capsule_overlaps_aabb`, not `segment_range_distance`, which is segment-vs-range and the
wrong shape here. Overlap holds when the squared sum is at most the squared sum of the touch radius and
the capsule radius. Iterate items via `registry.iter_with_kind(ComponentKind::Touchable)` collected and
sorted ascending by `EntityId`, skipping any entity marked for end-of-frame removal and any whose
`TouchableComponent.radius` is zero — a zero-radius item must not be acquirable by the capsule radius
alone.

Follow the Evaluation model section exactly. Every tick, for every overlapping `(player, item)` pair,
build `TouchFacts { owned_count, free_slots, magazine, reserve, pressed }` and run the policy —
`owned_count` counts occupied slots whose wieldable's `DescriptorProvenance.canonical_name` matches the
item's, `reserve` reads the pawn's `AmmoReserve` balance for the item's ammo type and is zero when the
weapon authors no ammo resource, `pressed` is that player's live `use_pressed` entry. Build the facts
whether or not the acting policy reads them, and on ticks where nothing is applied.

`default_touch_policy(facts) -> Vec<TouchEffect>` is the single decision site. It returns
`vec![TouchEffect::Acquire]` when `owned_count == 0 && free_slots > 0`, and an empty vector otherwise.
`TouchEffect` is a closed enum holding `Acquire` alone. Apply the returned effects only when a touch
fires — an enter edge in `Auto`, a `use_pressed` edge while overlapping in `Press` — and apply them in
order; `Acquire` calls `acquire_wieldable` from Task 2. An empty list leaves the occupancy entry in place
so the next tick does not re-fire. A pair is prompt-eligible when its evaluation returned a non-empty list
and no effects were applied for it this tick — never by an acquisition-specific test, so an added effect
arm prompts without touching the pass.

A player pressing while overlapping several items acquires only the nearest by squared center distance,
breaking ties on the lower `EntityId`. On a successful acquisition, remove the item's
`TouchableComponent`, drop its occupancy entry, purge the item's pending `DeferredEffect` queue so a
same-tick scripted despawn cannot execute next tick against the now-held weapon, and add it to
`TouchReport.acquired`. Process players in `PlayerId` order so two simultaneous enter edges resolve
deterministically.

Amend `context/lib/entity_model.md` §7 in this task, in two places. Collision Timing states that
entity-entity overlap runs after all entity updates complete; this pass runs between the trigger and AI
stages, so record the narrower placement and why it is sufficient. §7 also fixes volume size per entity
type; restate it as per descriptor. Both sentences are false once this ships, and the next
entity-overlap spec reads them.

### Task 4: Drop action and command plumbing

Add `Action::Drop` to `crates/postretro/src/input/types.rs` and bind it in
`crates/postretro/src/input/defaults.rs` — `KeyG`, a gamepad button, and the defaults table entry,
following the three sites `Action::Use` occupies. `G` is a provisional default and the owner may revise
it; nothing else depends on the choice. Read it as a rising edge at tick index zero in
`crates/postretro/src/main.rs`, in the same block that builds `use_pressed` from `Action::Use`, producing
a `HashMap<PlayerId, bool>` of drop edges for the local pawn, and thread it into the touch pass beside
`use_pressed`.

Add a `drop_pressed` field beside `use_pressed` on the per-tick movement command in
`crates/postretro/src/movement/mod.rs` and on the network input command so remote players' drop presses
reach the host, merged into a `PlayerId::Remote` map in `main.rs` beside the existing remote use-edge
construction. `drop_pressed` is an **edge bit**: clear it in `held_gap_sim_command` and synthesize it
false in `neutral_sim_command` in `crates/postretro/src/netcode/command_queue.rs`, exactly as both
already treat `use_pressed`. Do not carry it forward the way `reload` is carried — `reload` is a level
bit deduplicated by weapon-owned `reload_press_consumed`, and an edge bit held across a gap would drop one
weapon per held tick from a single press. A drop press discarded by the catch-up trim is lost, matching
`use`; do not extend reload's pre-trim edge lane.

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
registrations.

Expose register and unregister entry points the tick caller drives from Task 3's `TouchReport`: the
`acquired` list unregisters and forgets each item's allocator mapping so the client receives a despawn
tombstone, and the `dropped` list stamps and registers so the client receives a baseline. Held wieldables
are not replicated today — the only `replicable.register` calls naming a weapon are in
`netcode/lifecycle.rs` test fixtures — so acquisition ends an item's replicated life rather than
transferring it, and the client learns the pawn's new weapon through the tuning path in Task 7. Confirm
the client's apply path materializes an item baseline from `entity_class` plus `Transform` without
further metadata; that combination is already valid on non-despawn records.

### Task 6: Drop

Add the drop path to `crates/postretro/src/sim/touch.rs`, driven by the `drop_pressed` map Task 4
produces and evaluated at the top of the same pass, before any overlap or touch edges, so a drop and an
acquisition on one tick cannot interact. For each player with a drop edge, call `release_wieldable` from
Task 2 for that pawn's active slot. On success:

Write the freed item's `Transform.position` to a point in front of the pawn, derived from the pawn's
transform and capsule at ground level, clamped back to the pawn's own position when the target point is
not reachable through the `&CollisionWorld` the pass takes, so an item is never dropped inside geometry.
Attach a `TouchableComponent` built from the descriptor's touchable block. Force the released wieldable
out of any timed state and reset the state timer fields alongside the state — a weapon dropped mid-reload
or mid-raise must land in the world idle, because nothing will tick it. Add the item to
`TouchReport.dropped` so the caller registers it for replication.

Seed the item's occupancy entry with **every player whose capsule overlaps the drop point**, not only the
dropper. Seeding the dropper alone leaves two holes: a second player already standing there gets an enter
edge on the drop tick and steals the item, and two adjacent players dropping on the same tick each get an
enter edge on the other's item next tick and swap weapons. Seeding all current occupants closes both with
one rule. The seed suppresses enter edges only, so a `press`-mode item can still be re-acquired by a
deliberate press — that is intended.

The existing `normalize_inventory_liveness` in `crates/postretro/src/sim/weapon_stage/commands.rs`
handles the pawn side of a vanished slot; releasing is not a vanish, so drop must leave the inventory
consistent itself rather than relying on that reconciliation.

### Task 7: Client inventory growth and release over the tuning channel

`materialize_net_local_wieldable_inventory_from_tuning` in
`crates/postretro/src/scripting/builtins/net_descriptor.rs` builds a client's local inventory only when
the pawn has none, and its slot-merge loop copies tuning fields onto slots the client has already filled,
skipping any slot the client holds as `None`. That skip is what makes a host-side acquisition invisible.

Change the merge loop in both directions. Where a tuning slot names a canonical name and the client's
slot is `None`, spawn the wieldable locally through `spawn_descriptor_instance` with the canonical name
the payload carries — the same call `compose_wieldable_inventory_slots` uses — and place it with Task 2's
`acquire_wieldable_at` at the index the host named, so the two inventories agree by index. Where a tuning
slot is `None` and the client holds a wieldable, call `release_wieldable` and despawn the freed local
instance; the client owns that entity outright, and leaving it unreferenced leaks it for the level.

Increment `TUNING_PAYLOAD_EPOCH` in `crates/postretro/src/netcode/tuning_payload.rs` from 2 to 3: the
layout is unchanged but the merge semantics are not, and the epoch gate is what stops a peer from
applying the old reading.

For the send trigger, `host_send_tuning_if_changed` already dedupes by payload equality and has two call
sites. Call it once per host poll for each participating slot rather than adding a dirty-flag protocol —
the equality check makes an unchanged inventory free, and an inventory change then publishes on the next
poll with no new bookkeeping. That trigger still escalates the channel: a config push at transitions
becomes an event-driven state channel with no delta and no ack, so a dropped payload leaves a stale
client inventory until the next publication. Accept it here — the payload is small, fixed-size, and
reliable-ordered — and hold the constraint `E15` set, that it stays opaque to `crates/net`. Route the
client's local `Inventory` writes through Task 2's chokepoint so the source-scan test stays satisfied.

### Task 8: Ordering and edge coverage

Build the test coverage for the Ordering matrix rows in this plan that no single task above owns end to
end, and cite the row rather than restating it. Specifically: two players entering one item's radius on
one tick producing exactly one acquisition with a stable winner; one player entering two `auto` items on
one tick with one free slot; a frame rendering zero fixed ticks and a frame rendering two; a `use` press
landing on the same tick as a trigger-volume Use activation, with both firing and neither consuming the
press; a scripted despawn queued on the acquisition tick; a `press` player overlapping two items; drop
followed by standing still, by another player already at the drop point, by two adjacent players dropping
together, and by leaving and returning; a player dying while overlapping; a drop press across a packet
gap; a weapon instance sitting unowned in the world across many ticks advancing no cooldown, reload, or
state timer; and a picked-up weapon surviving a level transition with its magazine.

Three tests carry more than their row. Write the unowned-inertness test against observable component
state after N ticks, never against the call sites — the invariant it pins is currently held only by two
call sites choosing not to scan the weapon column, and a test naming them would move with them. Cover the
facts by substituting a policy that acquires unconditionally and asserting a duplicate lands in the next
free slot: it proves the facts are built independently of what the default reads, which no test of the
default alone can show. Cover the no-re-fire threat by counting policy invocations against an item whose
evaluation returns an empty list, since with the default policy a spurious re-fire is otherwise
behaviorally invisible. Host-and-client cases belong in the existing netcode harness style rather than a
new fixture.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice through authoring, both FFI readers, the typedef schema, and
the map sweep. It falsifies the boundary assumptions (casing, reader mirroring, typedef drift, the
placeability predicate) before anything is built on them.
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

`trigger_system.rs` is 2400 lines but only ~700 are production code — the rest is its test module. It
needs no split before this work, and this spec adds nothing to it beyond making one helper visible.
`sim/weapon_stage.rs` is likewise a small facade over submodules. Neither triggers the split-first rule.

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

- **Whether a dropped item should be reachable through the `world.query` component vocabulary.** Nothing
  in this spec needs it, and adding a component to that enum is a scripting-surface commitment. Left out;
  a mod wanting to script over world items would raise it as its own change.
