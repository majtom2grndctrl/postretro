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
- Prompt-eligible pairs, held as in-memory per-tick state on the touch system.
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
  trigger volume covers the **map-authored** resource case and only that: brush volumes are
  level-load-only (see *Alternatives rejected*), so no existing mechanism covers a resource item that
  appears at runtime. Per-player currency is `drafts/E16--per-player-currency`.
- **Item respawn.** A taken item is gone for the level.
- **Client prediction of acquisition.** Acquisition is host-only; a client sees the item disappear after
  the round trip.
- **AI or enemy touch.** Only player pawns generate touches.
- **Automatic switch to a newly acquired wieldable**, except when the inventory held nothing.
- **Throwing.** Drop places the item; it has no velocity and no physics.
- **An edge-recovery lane for the drop press.** A drop press discarded by the input catch-up trim is lost,
  matching `use`. Reload's dedicated pre-trim edge lane is not extended here.

## Evaluation model

Overlap, evaluation, and touch are three separate steps.

| Step | When | Effect |
|---|---|---|
| **Overlap** | Geometric state, recomputed every tick | Maintains the per-item occupancy set; produces enter and exit edges |
| **Evaluation** | Every tick, for every overlapping `(player, item)` pair | Builds `TouchFacts` and runs the policy. Result drives prompt-eligibility only |
| **Touch** | Enter edge in `auto`; `use_pressed` edge while overlapping in `press` | Marks the player a **contestant** for that item; the item's nearest contestant wins and its effects **apply** |

Items resolve one at a time, in ascending `EntityId` order. Every overlapping `(player, item)` pair is
evaluated when its item resolves, against the world already mutated by lower-`EntityId` items this tick —
so a player who filled their last slot on an earlier item evaluates this one with `free_slots = 0`. A
pair's policy runs once, at that moment, and that run drives both the pair's prompt-eligibility and, if the
pair wins, its application. There is no global pre-pass.

An item is won by one **contestant**. On a press edge a player's contest is confined to a single
**claim** — the nearest press item they overlap and do not already own, ties on the lower `EntityId` — so
one `use_pressed` edge acquires at most one item; an `auto` enter edge contests the item it lands on. The
item reduces its contestants to one winner: the nearest by squared centre distance, ties on the lower
`PlayerId`; the losers acquire nothing. One winner per item bounds acquisition to a single player, so no
separate liveness re-check is needed. A press pair that is not the player's claim is still evaluated for
prompt-eligibility but never acquires. When one free slot is contested by both an `auto` and a `press`
item, the lower-`EntityId` item resolves first and takes it; the other takes nothing.

A `press` pair is prompt-eligible when its evaluation returns a non-empty effect list and no effects were
applied for it this tick. **Applied** means an effect that *succeeded*, not merely one the policy returned:
an item whose `Acquire` the chokepoint refuses counts as nothing applied and stays prompt-eligible. `auto`
items are never prompt-eligible: their affordance is walking, so there is nothing to prompt for.
Eligibility filters on **mode**, which is the affordance, never on which effect the policy returned — an
added effect arm must prompt without touching the pass.

The policy runs on ticks where nothing is applied. It must be pure.

`pressed` is an **edge**, not a level. The `use_pressed` map is built as
`tick_index == 0 && ButtonState::Pressed` and holds only `true` entries, so the fact is true on exactly
one tick per press and false while the button is held. A policy gating on it describes an effect that only
a fresh press produces.

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
| drop edge | `MovementInput::drop_pressed` / `SimCommand::drop_pressed` | `WireMovementInput::drop_pressed` | n/a | n/a | n/a |
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

**Per-tick input command.** `drop_pressed` joins `use_pressed` on the per-tick input levels. `InputCommand`
rides `ClientMessage` on the Input channel, so its layout is gated by `WIRE_VERSION`, not
`SNAPSHOT_VERSION`. That gate is independent of the tuning epoch above; bump it on its own.
`drop_pressed` is an **edge bit** and follows `use_pressed`'s treatment exactly: `held_gap_sim_command`
clears it on every held tick and `neutral_sim_command` synthesizes it false, so a packet gap replays no
drop. It is **not** carried forward like `reload`, which is a level bit deduplicated by weapon-owned
state. Carrying an edge bit across a gap would drop one weapon per held tick from a single press.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| An item is acquired by at most one player, ever | Task 3 (each item's contesting touches reduce to one winner, so at most one acquisition per item per tick) | A reducer returning more than one winner; an item resolved in more than one reduction per tick | AC 5 |
| Effects apply only on a touch; evaluation alone never mutates | Task 3 (evaluation and application are separate steps) | A later effect arm applied from the eligibility path | AC 7, AC 10 |
| A touch fires on an edge, never on sustained overlap | Task 3 (per-item occupancy set, enter transition in `auto`; press edge in `press`) | Drop seeds the drop point's occupants (Task 6); an empty effect list must not clear occupancy, or the next tick re-fires | AC 4, AC 10, AC 19 |
| Facts are computed at every evaluation, whatever the policy reads | Task 3 (`TouchFacts` built before the policy runs) | A later short-circuit that skips fact computation when the default ignores a field | AC 7 |
| The policy decides; the chokepoint mutates | Task 2 (chokepoint holds no eligibility rule), Task 3 (policy holds no registry write) | Task 6's drop refusal, which belongs in the drop path and must not migrate into the chokepoint | AC 6, AC 7 |
| Prompt-eligible means the policy returned effects that were not applied | Task 3 | A later effect arm filtered by an acquisition-specific check | AC 10 |
| Pickup and drop never write `AmmoReserve` | Task 2 (the chokepoint takes no reserve argument) | The existing source-scan test allowlists exactly two non-test `credit` call sites and one non-test `set_exact` call site, across two assertions; a new one fails it | AC 9 |
| Inventory slot **fill** happens in exactly one place | Task 2 (chokepoint + its own source-scan test) | Task 3 and Task 7 both fill slots and must route through it; Task 6 releases through the same module | AC 8 |
| A world item is never ticked by the weapon stage | Already true — no weapon-stage path reaches a weapon except through its owner: `run_local_weapon_command` via `Inventory::active_wieldable`, `run_remote_weapon_commands` via `RemotePawnCommand.weapon`, and `normalize_all_inventory_liveness` via the `Inventory` column; no production code scans the weapon column | A future scan-based stage would break it silently | AC 11 |
| A descriptor authoring no touchable block is not map-placeable by virtue of its weapon | Task 1 (the placeability predicate gains a `touchable` term and no `weapon` term) | Any later addition of a `weapon` term | AC 2 |
| Acquisition and drop are host decisions | Task 3 (the pass sits inside the tick a connected client never runs), Task 5, Task 6 | A client must not predict either; its inventory changes only on host word | AC 13 |
| A picked-up wieldable carries across a level transition | Inherited — carry harvests each slot's `DescriptorProvenance.canonical_name`, which a map-placed item carries like any descriptor spawn | Task 2 must not clear or rewrite provenance when a slot is filled | AC 12 |

## Ordering matrix

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Two players enter one item's radius on the same tick | Both are contestants; the item reduces to one winner | The nearest by squared centre distance acquires, ties on the lower `PlayerId`; the other acquires nothing. One acquisition, stable across runs with identical inputs. |
| Two players contest one item, one already holding its canonical name | The owner's evaluation returns no effects, so it never contests | The non-owner acquires regardless of distance — `ownedCount` removes the owner before the nearest-winner reduction runs. |
| A press player overlaps two press items and presses once, two free slots | Press-eligible items reduced to the nearest before application | Exactly one taken — nearest by squared centre distance, ties on the lower `EntityId`. The un-taken item stays a world item and stays prompt-eligible next tick. |
| One player, two `auto` items entered on one tick, one free slot | Items iterated in ascending `EntityId` order | The lower `EntityId` is acquired; the second evaluates with `freeSlots = 0` and returns no effects. `EntityId` packs generation above index, so ascending order is deterministic but is not spawn order; the guarantee is stability across runs with identical inputs, matching the `trigger_ids` sort in `run_authoritative_tick_with_dispatch`. |
| One player, one free slot, an `auto` item entered and a `press` item claimed on one tick | Both resolve in ascending `EntityId` order | The lower-`EntityId` item takes the slot; the other evaluates with `freeSlots = 0` and takes nothing. The cross-mode winner is the `EntityId` order, not auto-over-press or press-over-auto. |
| Player drops an item and stands still | Drop seeds every player overlapping the drop point into the item's occupancy | No enter edge, so no touch in `auto`. A `press` player pressing again re-acquires — a deliberate press defeats the inhibit. |
| Player drops, walks out of radius, walks back (auto) | Exit edge clears occupancy; later enter edge fires | Re-acquired |
| Player B already overlaps the point where A drops | Drop runs before touch edges; the seed covers B as well as A | B gets no enter edge on the drop tick. B acquires only after leaving and returning, or by pressing a `press` item. |
| Two adjacent players drop on the same tick | Each drop seeds every player overlapping *that* item's point, so each is seeded into both | Neither gets an enter edge next tick; no mutual weapon swap |
| Sphere-cast fallback relocates the drop point; a second player overlaps the resolved point but not the front point | Occupancy is seeded against the final resolved position, after the fallback | That player is seeded and gets no enter edge on the drop tick |
| Player overlaps a `press` item and never presses | Enter edge, then sustained overlap; evaluation runs every tick | Prompt-eligible every tick while the policy returns effects; no touch fires |
| Player overlaps an item they already own (either mode) | Evaluation runs; facts carry `ownedCount = 1`; default policy returns no effects | No acquisition, not prompt-eligible. A later policy reading the same facts returns effects here, which makes it prompt-eligible with no change to the pass. |
| Inventory full | Evaluation runs; facts carry `freeSlots = 0`; default policy returns no effects | Same as duplicate. Freeing a slot while standing on an `auto` item does not acquire it — there is no new enter edge — so the player steps off and back on. |
| `use` press lands on the same tick as a trigger-volume Use activation | Trigger stage runs first, touch pass second, both read the same `use_pressed` map | Both fire. The press is not consumed by either. |
| A script queues `despawn(item)` on the tick the item is acquired | The deferred effect rides the item's own queue and would execute next tick against a now-held weapon | Acquisition purges the acquired item's pending deferred effects. The weapon survives. |
| A script queues a deferred effect against a weapon picked up and dropped earlier in the level | Acquisition cleared the queue but left `inert` false | The effect is admitted and runs normally. Only a terminal despawn sets `inert`, and acquisition is not one. |
| Drop and same-tick re-acquire of one item | Drop runs at the top of the pass; a deliberate press defeating the inhibit re-acquires after | The item ends the tick held, with no `TouchableComponent`. It was unregistered at tick start (held, not a tracked world item), so the end-of-tick sweep emits nothing and the client sees nothing — there is no transform delta and no `NetworkId` to hold unchanged. Across two ticks — acquire on one tick, drop on a later one — the sweep unregisters and forgets, then re-stamps, so the item's `NetworkId` does rotate and the client sees a despawn followed by a fresh baseline. |
| A dropped item's mesh reaches clip resolution | Task 6 sets the `MeshComponent` directly, which enqueues no spawn-context resolve | The pass reports the dropped item id on a `TickEvents` vector; the per-tick clip-binding resolve `main.rs` already runs for runtime-spawned host enemies drains it in the same call. Resolving an id whose `MeshComponent` was removed later that frame is a harmless no-op. Without the report a dropped animated model renders unbound. |
| An item already marked for end-of-frame removal | Removal pass runs at end of frame | The pass drops the item's occupancy entry, then skips it — no overlap, no evaluation. Dropping the entry before skipping mirrors the vanished-item rule, so nothing stale survives the removal. |
| A deferred-despawn timer expires the same tick a player's enter edge would acquire | `tick_deferred_effects` runs at the top of the tick, before the pass | The effect marks the item for end-of-frame removal; the pass skips the now-marked item. Not acquired; removed at end of frame. |
| A drop press edge lands on tick 0 while the overlap only begins on tick 1 | `pressed` is an edge true on exactly one tick | The `press` item is not taken: on tick 0 there is no overlapping pair, and on tick 1 the pair overlaps with `pressed = false`. The player presses again. |
| Frame renders zero fixed ticks | Pass lives inside the tick loop | No evaluation, no prompt refresh; last published prompt state persists for that frame |
| Frame renders two fixed ticks | Pass runs twice | The enter edge fires on the first; the second sees sustained overlap and applies nothing |
| Radius authored zero or negative | `TouchableDescriptor::validate` rejects at descriptor load | The mod fails to load with a shape error. No entity spawns, so the capsule radius alone can never make a zero-radius item acquirable. |
| Drop press followed by a packet gap | `held_gap_sim_command` clears the edge on every held tick | Exactly one drop |
| Drop press inside a backlog that trips the catch-up trim | The trim discards the command carrying the edge | The drop is lost, matching `use`. Accepted; no edge-recovery lane. |
| Drop with an empty inventory | No active wieldable | No-op, no warning — pressing drop with nothing held is ordinary input |
| Drop of a wieldable whose descriptor has no touchable block | Drop path refuses before it calls the chokepoint | Nothing leaves the inventory; warn once per descriptor. An item with no touchable block could never be recovered. |
| Player disconnects while overlapping an item | The slot close despawns the pawn, so the capsule lookup fails next tick | Treated as an exit: occupancy and any drop inhibit for that `PlayerId` clear |
| A drop edge arrives the tick the dropper's pawn despawns (disconnect) | Disconnect teardown vs the drop path both act this tick | Teardown wins: the drop is honored only if the pawn's inventory still resolves when the pass runs. A pawn already torn down by the slot close exposes no inventory, so the edge is a silent no-op — the item leaves with the departing inventory, not into the world. |
| Player dies while overlapping an item | The death sweep latches but never despawns a player pawn, so the corpse keeps `Transform` and `PlayerMovement` and the capsule lookup still succeeds | The corpse keeps occupancy and stays a toucher. It generates no enter edge while it lies still, but a corpse-held drop edge still drops, and a corpse counts among the drop point's occupants. The engine holds no aliveness opinion — a mod that wants otherwise authors it once the dispatch source lands. |
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

The policy decides whether a player *wants* an item; a second reducer decides which of several wanters
gets it. This spec ships the simple default — nearest by squared centre distance, ties on the lower
`PlayerId` — but the reducer is the seam a fairness rule plugs into. It bites once an effect arm lets an
owner contest: `grantAmmo` makes a player already holding the weapon want the item for its ammo, so an
owner and a non-owner contest one pickup, and *the weapon to the player without it, the ammo to the player
holding less* becomes a real choice over the contestants' facts. The general form — rank N contestants
under slot capacity — is a scan over a player collection, the shape `scripting.md` §11 forbids a mod
policy, so the reducer stays engine-owned until that substrate lands.

**Prior commitments.** `context/research/weapon-model.md` invariant 6 pins held and dropped wieldables as the same
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
instance model arriving through a worse door. It also contradicts `context/research/weapon-model.md` invariant 6 directly.
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
3. [ ] An `auto` item is taken the first tick a player's capsule overlaps its sphere, and stops being a
   world item in that same tick: it carries no touchable component, is no longer replicated, and no longer
   renders at its pickup position.
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
    `auto` item is never reported prompt-eligible, including on a tick where its policy returns effects
    that no enter edge applied.
11. [ ] A weapon instance sitting in the world advances no cooldown, no reload, and no state timer over
    any number of ticks.
12. [ ] A weapon picked up during a level is present in the inventory after a level transition, with its
    magazine preserved.
13. [ ] In a host-plus-client session, a client standing on an item does not remove it locally; the item
    disappears only after the host's despawn arrives, and the client's inventory shows the new weapon
    only after the host reports it.
14. [ ] A client whose inventory has an empty slot materializes a wieldable in that slot when the host's
    tuning payload names one, at the slot index the host named. A tuning slot that is `None` where the
    client holds a wieldable releases and despawns the local instance. Descriptor identity only: the
    payload carries no magazine and no instance id, so a same-canonical-name swap reaching the client as
    an equal payload is correct and sends nothing.
15. [ ] A dropped weapon lands at a reachable point in front of the pawn, never inside world geometry,
    and lands idle — a weapon dropped mid-reload or mid-raise shows no residual timed or consumed-edge
    state.
16. [ ] Pressing drop with an empty inventory, and dropping a wieldable whose descriptor authors no
    touchable block, both leave the inventory unchanged.
17. [ ] A touch radius authored as zero or a negative number fails descriptor load with a shape error
    naming the offending entry, and no entity spawns for it.
18. [ ] A single drop press produces exactly one drop across a packet gap that holds the command for
    several ticks.
19. [ ] A player standing on an item whose policy returns no effects has that policy invoked exactly once
    per tick, and the item is neither acquired nor re-evaluated as a fresh enter edge on any later tick of
    the same unbroken overlap.
20. [ ] In a host-plus-client session, a client-side world item draws its authored model rather than
    nothing — both a map-baselined item and a host-dropped item.

## Tasks

### Task 1: Touchable descriptor, component, and map placement

Add a `TouchableDescriptor { mode: TouchMode, radius: f32 }` beside `WeaponDescriptor` in
`crates/foundation/src/data_descriptors/types/combat.rs`, carrying `#[serde(rename_all = "camelCase")]`
to match its sibling. `TouchMode` is `Auto | Press` with `rename_all = "camelCase"`, wire spellings
`"auto"` and `"press"`; `mode` defaults to `Auto` and `radius` defaults to `40.0`, each through a
`#[serde(default = "…")]` free function as `cost_per_shot` does, so a block authoring only `mode` loads.
Add `touchable: Option<TouchableDescriptor>` to `EntityTypeDescriptor` in
`crates/entities/src/data_descriptors/types/entity.rs`. `EntityTypeDescriptor` derives no `Default` and is
built by exhaustive struct literal at roughly 90 sites across 21 files, so every one gains
`touchable: None`. Adding `impl Default for EntityTypeDescriptor` is no shortcut: no site uses
`..Default::default()`, so the impl alone changes nothing, and converting the sites is a larger edit than
the one it replaces. Read the `touchable` key under `components` in
both FFI readers — `crates/scripting-core/src/data_descriptors/lua/entity.rs` and the JS mirror in the
sibling `js/entity.rs` — following the shape the existing `weapon` arm uses in each. Register
`TouchableDescriptor` and `TouchMode` in the typedef schema in
`crates/postretro/src/scripting/primitives/mod.rs` beside `WeaponDescriptor`, add `touchable` to the
`EntityTypeComponents` type, and regenerate the SDK types with
`cargo run -p postretro --bin gen-script-types`; the committed-typedef drift test fails until you do.
Validate in `TouchableDescriptor::validate` that `radius` is finite and positive, rejecting with
`DescriptorError::InvalidShape` as `WeaponDescriptor::validate` does for `damage` and `range`. The
signature is `fn validate(self) -> Result<Self, DescriptorError>` and receives only the block, so it
cannot name the descriptor; both FFI readers already call `descriptor.validate()?` in their `weapon` arm,
and the load error names the offending entry. Rejecting rather than clamping follows every sibling
validator: descriptor tuning is authored data, and only FGD key-values warn and fall back.

Add a runtime `TouchableComponent { mode, radius }` in `crates/entities/src/components/touchable.rs`. A
new component kind touches six groups of sites. `Inventory` is the most recently added kind and is the
template to follow at each:

- `ComponentKind::Touchable` on the next free discriminant, plus the `VARIANTS` array behind
  `ComponentKind::COUNT`, both in `crates/entities/src/registry.rs`. That array is not compiler-enforced
  and `COUNT` sizes the registry's component column storage, so a missed entry costs the component its
  column.
- `ComponentValue::Touchable`, its arm in the value-to-kind match, and the `Component` trait impl —
  same file.
- The kind-to-name arm and the two rejecting `ComponentValue` conversion arms in
  `crates/entities/src/ffi.rs`, one per script runtime.
- `ALL_KINDS`, the kind-to-name arm, and the kind-to-value arm in
  `crates/postretro/src/observability/mod.rs`.
- `component_kind_discriminant` in `crates/postretro/src/netcode/mod.rs` — an exhaustive match with no
  `_` arm, pinned numerically to `ComponentPayload::kind()` in `postretro-net`. `Touchable` takes the
  matching discriminant and adds no wire payload: `ComponentPayload` carries four variants against
  twenty-one kinds once `Touchable` lands, so a payload-less kind is the norm. The drift guard
  `discriminant_mapping_matches_enum_layout` in the same file holds a second `_`-less match, `next_kind`,
  walking the variant chain in discriminant order; it compile-blocks until `Touchable` is spliced in
  (`Inventory => Some(Touchable)`, `Touchable => None`).
- Three further exhaustive matches over `DescriptorComponentKind` in
  `crates/scripting-core/src/refresh_plan.rs` — `descriptor_declares`, `live_component_exists`, and
  `plan_component_replace` — driven by the `DescriptorComponentKind::ALL` loop. A touchable block is
  pure tuning with no live state, so it hot-reloads by replacement, following `plan_health_replace`
  rather than the declined `Mesh` arm.

Then add the `DescriptorComponentKind::Touchable` variant in `crates/entities/src/provenance.rs`. Its
`component_kind()` and `label()` are exhaustive matches and will not compile until extended; its
`ALL: [Self; 6]` const compiles fine while silently stale, so extend it by hand and widen it to 7.

Then make the placement change in `crates/postretro/src/scripting/builtins/data_archetype.rs`: add
`|| descriptor.touchable.is_some()` to `is_directly_map_placeable`, and change the map-sweep call site's
`attach_weapon` argument from the literal `false` to `descriptor.touchable.is_some()`. Attach the
touchable component in `attach_descriptor_components` under the same condition, recording the provenance
kind. `is_directly_map_placeable` lists `light || emitter || movement || mesh || health` and excludes
weapon **by omission** — do not add a `weapon` term; that exclusion is the shipped guarantee. The two
existing regression tests place descriptors that author no touchable block, so both test bodies must still
pass unmodified, and a test asserting that is part of this task; the shared `weapon_descriptor` and
`light_descriptor` helpers in that test module gain the new field.

### Task 2: Inventory fill and release chokepoint

Add the first live-inventory read-modify-write to
`crates/postretro/src/scripting/builtins/wieldable_inventory.rs`. The "this is the only composition path"
sentence is the doc comment on `compose_wieldable_inventory`, not the module doc — update that function's
doc in the same change rather than leaving it false.

Three functions, all pure effects. Whether an acquisition *should* happen is Task 3's decision, not this
layer's.

`acquire_wieldable_at(registry, pawn, slot, item) -> bool` is the primitive. It writes the item id into
the named slot and returns whether it did, returning `false` when the slot is occupied, out of range, or
the item carries no weapon component. `acquire_wieldable(registry, pawn, item) -> Option<usize>` is the
lowest-index-free-slot wrapper over it, returning the filled slot. Task 7's client path needs the
slot-targeted form: the host names a slot index, and picking the client's own lowest free slot would
desync the two inventories by index.

Apply the one mesh rule at composition too: `compose_wieldable_inventory_slots` in the same file spawns
each loadout wieldable through `spawn_descriptor_instance`, and `attach_descriptor_components` attaches a
`MeshComponent` whenever the descriptor authors a mesh block, on every spawn path. Remove it after the
spawn, so a spawn-composed wieldable matches an acquired one: a wieldable owned by an inventory carries no
`MeshComponent`, and its visual is the pawn's `thirdPersonModel` attachment.

Neither must modify `active_slot` unless the inventory held no wieldable at all, in which case
`active_slot` becomes the filled slot. Neither must touch `AmmoReserve`, `switch_target`, or
`switch_origin`, and neither must rewrite the item's `DescriptorProvenance` — cross-level carry reads the
canonical name from it. Neither may reject a duplicate: a policy that wants a second shotgun in slot 4
gets one.

`release_wieldable(registry, pawn, slot) -> Option<EntityId>` clears the slot, returns the freed entity,
and re-picks `active_slot` as the lowest occupied slot (or leaves it at zero when the inventory is now
empty), clearing `switch_target` and `switch_origin` if either referenced the released slot. It holds no
eligibility rule and takes no descriptor slice: a host-side slot emptying is not always a drop, and Task
7's client release path must reach it unconditionally.

Add a source-scanning test in the same shape as
`ammo_reserve_writes_have_only_grant_seed_and_carry_restore_non_test_call_sites` in
`crates/entities/src/components/grant.rs`. Those helpers live inside that file's `#[cfg(test)] mod tests`
in `postretro-entities`, a different crate, so they cannot be imported — **copy** the directory walk,
`is_test_source_file`, and `mask_test_only_blocks` together with the two helpers it calls,
`mask_comments_and_string_literals` and `matching_brace_end`. The matcher does not carry over at all: that test's
`method_call_count` counts
literal `.credit` / `.set_exact` occurrences, and a slot fill is an assignment, not a method call. Walk
`crates/`, mask `#[cfg(test)]` blocks and test files, and count **slot-fill** writes to
`Inventory::wieldables` — `Some(...)` assignments into an inventory slot — outside them. Discriminate on the
`Inventory` target, not the bare `.wieldables[` token: `CarriedState.wieldables` in
`crates/postretro/src/netcode/seat.rs` and the `TuningPayload.wieldables` payload array share the field name.
Both are distinguishable today — seat.rs assigns `carried_wieldables[slot] = weapon`, not a `Some(...)` into
`.wieldables[` — so a matcher keyed on `.wieldables[..] = Some(` counts only inventory fills now; a future
`Some(...)` write to either sibling field would trip the test and must be excluded then. Scan fills only, not clears: `normalize_inventory_liveness` in
`crates/postretro/src/sim/weapon_stage/commands.rs` nulls slots in production and is not a growth path.
The allowlist is exactly the composition site and the two acquire functions.

### Task 3: Touch pass, facts, and default policy

Add `crates/postretro/src/sim/touch.rs` with a `TouchSystem` holding
`occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>` and its own
`warned_duplicate_players: HashSet<PlayerId>` — the same shape `TriggerSystem` uses, and for the same
reason: sorted keys make edge emission stable across equivalent input orderings.

Its per-tick entry point takes `&mut EntityRegistry`, `&CollisionWorld` (Task 6's drop placement needs
it), `descriptors: &[EntityTypeDescriptor]` (acquire and drop both rebuild components from the
descriptor), the same `players: &[AuthoritativePlayer]` slice and `use_pressed: &HashMap<PlayerId, bool>`
map that `TriggerTickInputs` already carries, and a `drop_pressed: &HashMap<PlayerId, bool>` map from
Task 4. `collision_world` is already a `simulate_tick_with_presentation_aim` parameter. `descriptors` is
not — add it, sourced at the `main.rs` call site from `script_ctx.data_registry.borrow().entities`, which
is how `update_repointed_weapon_attachments` already obtains the same slice.

Three results leave the pass. Two ride `TickEvents` vectors the tick already returns; the third is
per-tick state on `TouchSystem`.

`repointed` lists every pawn whose inventory or `active_slot` changed. It merges into the existing
`TickEvents.repointed_pawns` field the tick already returns; `main.rs` extends its `repointed_pawns`
vector from that, and `update_repointed_weapon_attachments` runs after the tick loop. Without it the
acquired weapon never gains the pawn's socket attachment, so the holder carries no visible weapon — the
item's own mesh is already gone, so the failure is a missing attachment, not a stale world render. On a
listen host that function marks the pawn attachment-dirty and returns; the attachment itself lands in
`synchronize_weapon_owner_attachments` before snapshot production. Single-player takes the direct branch.

Dropped item ids needing clip resolution leave on a new `TickEvents.dropped_item_meshes` vector, a
`Vec<EntityId>` following the `repointed_pawns` shape. Task 6's drop populates it; `main.rs` drains it per
fixed tick into the clip-binding resolve it already runs for runtime-spawned host enemies. Acquisition
populates neither this vector nor a resolve — it removes the item's mesh, it never adds one.

`prompts` is a `Vec<(PlayerId, EntityId)>` field on `TouchSystem`, replaced wholesale by each tick's pass
and readable off the session-owned system after the tick loop. It is not a return value: prompts are
per-tick state a later presentation spec reads, and a frame rendering zero fixed ticks must leave the last
published set standing rather than clearing it.

The pass does no replication bookkeeping and needs none — Task 5's sweep derives membership from
`TouchableComponent` presence.

The call site needs a refactor the task owns. `run_authoritative_tick_with_dispatch` sits inside
`if let Some(trigger_context)` in `crates/postretro/src/sim/mod.rs`, and both `players` and
`use_pressed` (as `trigger_context.use_edges`) are built inside that block, so a literal placement makes
pickup conditional on a trigger context existing and leaves `drop_pressed` no route at all. Hoist the
`players` construction above the `if let`, and carry `use_pressed` and `drop_pressed` as their own
`simulate_tick_with_presentation_aim` parameters rather than reading them off `TriggerTickContext`. The
touch pass runs on every tick, including ticks with no trigger context. Place it immediately after the
trigger stage and before the AI tick; `Session` owns the `TouchSystem`
beside its `trigger_system` field (`crates/postretro/src/session/mod.rs`), and
`clear_surface_lifetime_level_state` in `crates/postretro/src/startup/lifecycle_net.rs` clears it beside
the existing `session.trigger_system.clear()`.

The pass needs no host gate of its own. A connected client's fixed-tick branch in `main.rs` `continue`s
after its movement-only prediction and never reaches `simulate_tick_with_presentation_aim`, so the pass —
and Task 6's drop with it — is host-and-single-player-only by placement. Preserve that: no client path
calls into this module.

Resolve player capsules with the existing `canonical_player_capsules` helper in
`crates/postretro/src/trigger_system.rs`. It and `range_distance` in the same file are both private bare
`fn`s the touch module cannot call, so make each `pub(crate)`. `canonical_player_capsules` takes a third
argument, `warned_duplicate_players: &mut HashSet<PlayerId>`, which is why `TouchSystem` carries its own.
It returns `(pawn, position, radius, half_height)` per `PlayerId`; a `PlayerId` absent from the returned
map is treated as exited, clearing its occupancy entries, exactly as the trigger stage treats a vanished
player.

Overlap is sphere-vs-capsule: horizontal distance from the item's `Transform.position` to the capsule
axis, and `range_distance(item.y, capsule_min_y, capsule_max_y)` on the vertical — the point-vs-range
helper beside `capsule_overlaps_aabb`, not `segment_range_distance`, which is segment-vs-range and the
wrong shape here. Overlap holds when the sum of those two squared distances is at most
`(touch_radius + capsule_radius)` squared. Iterate items via
`registry.iter_with_kind(ComponentKind::Touchable)` collected and sorted ascending by `EntityId`, dropping
the occupancy entry of any entity marked for end-of-frame removal before skipping it — dropping before
skipping leaves no stale key, exactly as for a vanished item. Prune the occupancy map to this tick's
collected touchable set as well: an `EntityId` key absent from it is dropped, mirroring the player-exit
rule that treats a `PlayerId` absent from the returned map as exited, so `occupants` never accumulates
stale keys for despawned or vanished items. The pass needs no radius guard: `TouchableDescriptor::validate`
rejects a non-positive radius at descriptor load, so no zero-radius item ever spawns.

Resolve items one at a time, in ascending `EntityId` order. Evaluate every overlapping `(player, item)`
pair when its item resolves, against the world lower-`EntityId` items already mutated this tick — a player
who filled their last slot on an earlier item evaluates this one with `free_slots = 0`. The one policy run
per pair drives both prompt-eligibility and, if the pair wins, application; there is no global pre-pass.

Before the loop, fix each pressing player's **claim**: the nearest press item they overlap and do not
already own, ties on the lower `EntityId`. An item's **contestants** are the players whose touch can win
it — a player with an `auto` enter edge on it, or the player whose claim it is. A press pair that is not
its player's claim is evaluated for prompts but never contests, so one `use_pressed` edge acquires at most
one item.

Reduce an item's contestants to one winner: the default reducer picks the nearest by squared centre
distance, ties on the lower `PlayerId`; a later spec that ranks contestants by need replaces this default
reducer. Apply the winner's effects; the losers acquire nothing. One winner
per item bounds acquisition to a single player, so the pass needs no separate re-check that an item is
still unclaimed. When one free slot is contested by both an `auto` and a `press` item, the lower-`EntityId`
item resolves first and takes it; the other takes nothing.

Three steps run each tick; only the third mutates. **Overlap** is geometric state, recomputed every tick,
maintaining the occupancy set and producing enter and exit edges. **Evaluation** runs every tick for every
overlapping pair: build `TouchFacts { owned_count, free_slots, magazine, reserve, pressed }` and run the
policy once. **Touch** reduces each item's contestants to one winner and applies that winner's effects; a
contestant is a player with an `Auto` enter edge on the item, or the player whose press claim is the item.

`owned_count` counts occupied slots whose wieldable's `DescriptorProvenance.canonical_name` matches the
item's. `reserve` reads the pawn's `AmmoReserve` balance for the item's ammo type, zero when the weapon
authors no ammo resource. `magazine` and `reserve` guard on `WeaponComponent` presence and read zero when
it is absent — a touchable descriptor authoring no weapon block is out-of-scope authoring, but the pass
tolerates it rather than reading a missing component. `acquire_wieldable_at` already refuses a weaponless
item, so the default policy's `Acquire` never succeeds against one: nothing is applied, and a `press` item
of that shape stays prompt-eligible rather than being silently taken. `pressed` is that player's
`use_pressed` entry, which is an **edge**: the map is
built as `tick_index == 0 && ButtonState::Pressed` and holds only `true` entries, so it is true on exactly
one tick per press. Build the facts whether or not the acting policy reads them, and on ticks where
nothing is applied.

`default_touch_policy(facts) -> Vec<TouchEffect>` is the single decision site. It returns
`vec![TouchEffect::Acquire]` when `owned_count == 0 && free_slots > 0`, and an empty vector otherwise.
`TouchEffect` is a closed enum holding `Acquire` alone. Hold the policy as an
`fn(&TouchFacts) -> Vec<TouchEffect>` field on `TouchSystem`, defaulted to `default_touch_policy`, so a
test can substitute one — AC 7 requires that seam. Apply the returned effects in order; `Acquire` calls
`acquire_wieldable` from Task 2. An empty list leaves the occupancy entry in place so the next tick does
not re-fire. A `press` pair is prompt-eligible when its evaluation returned a non-empty list and no
effects were applied for it this tick; `auto` pairs are never prompt-eligible. Filter on mode, never on
which effect the policy returned.

On a successful acquisition the item stops being a world item, which is one rule with three parts. Remove
its `TouchableComponent` — Task 5's sweep reads that presence, and Task 6 rebuilds it from the descriptor
on drop. Remove its `MeshComponent`: the mesh collector draws every `Mesh` plus `Transform` entity at that
entity's own transform, so the wieldable's own mesh is what renders it in the world. A held wieldable's
visual is the `thirdPersonModel` attachment pushed onto the *pawn's* mesh, which is additive and
suppresses nothing — a retained `MeshComponent` therefore keeps drawing the weapon at its stale pickup
position. One rule governs both ends: **a wieldable owned by an inventory carries no `MeshComponent`.**
Both components rebuild from the descriptor, which is why the pass takes the descriptor slice. Then drop
the occupancy entry, clear the item's pending `DeferredEffect` queue and its `overflow_reported` flag —
leaving `inert` false, so the weapon still admits later deferred effects — and add the holder to
the tick's `repointed` list.

Add `EntityRegistry::is_marked_for_end_of_frame_removal` in `crates/entities/src/registry.rs` — the
registry exposes only `mark_for_end_of_frame_removal` and `take_end_of_frame_removals`, so the skip this
task requires is not otherwise expressible. `tick_deferred_effects` runs near the top of the tick, before
this pass, so an effect that fires there has already marked the item and emptied its queue; without the
predicate the pass would acquire a weapon the end-of-frame removal pass then deletes.

Amend `context/lib/entity_model.md` §7 in this task, in two places. Collision Timing states that
entity-entity overlap runs after all entity updates complete; this pass runs between the trigger and AI
stages, so record the narrower placement and why it is sufficient. §7 also fixes volume size per entity
type; restate it as per descriptor. Both sentences are false once this ships, and the next
entity-overlap spec reads them.

### Task 4: Drop action and command plumbing

Add `Action::Drop` to `crates/postretro/src/input/types.rs` and bind it in
`crates/postretro/src/input/defaults.rs` — `KeyG`, a gamepad button, and an entry in `common_actions()`
inside that file's `#[cfg(test)]` module, following the three sites `Action::Use` occupies.
`keyboard_mouse_bindings_cover_all_actions` and its gamepad sibling walk `common_actions()`, so the new
action needs both a keyboard and a gamepad binding or those coverage tests fail. `G` is a provisional default and the owner may revise
it; nothing else depends on the choice. `KeyCode::KeyG` currently also drives
`DiagnosticAction::SpawnChaseAgent` in `crates/postretro/src/input/diagnostics.rs`, so the provisional
gameplay binding shares the physical key with a dev-tools chord. Read `Action::Drop` from the latched gameplay snapshot produced by
`GameplayInputLatch::snapshot_for_ticks` in `crates/postretro/src/input/mod.rs`, not the raw frame
snapshot: the latch accumulates `Pressed` actions across zero-tick frames into a set and drains them onto
tick 0 of the next tick-bearing frame, so N drop presses across N consecutive zero-tick frames collapse to
exactly one drop. A latched press is discarded by `gameplay_input_latch.clear()`, whose three sites are window focus loss in
`main.rs`, `clear_surface_lifetime_level_state` in `crates/postretro/src/startup/lifecycle_net.rs`, and
the client's content-parity hold in `crates/postretro/src/netcode/endpoint.rs` — so a press latched across
a level unload or a focus loss is lost. Read it as a rising edge at tick index zero in
`crates/postretro/src/main.rs`, in the same block that builds `use_pressed` from `Action::Use`, producing
a `HashMap<PlayerId, bool>` of drop edges for the local pawn, and thread it into the touch pass beside
`use_pressed`.

Add a `drop_pressed` field beside `use_pressed` at three sites: `MovementInput` in
`crates/postretro/src/movement/mod.rs`, `SimCommand` in `crates/postretro/src/sim/mod.rs`, and
`WireMovementInput` in `crates/net/src/wire.rs`, so remote players' drop presses reach the host, merged
into a `PlayerId::Remote` map in `main.rs` beside the existing remote use-edge construction. `SimCommand`
mirrors `MovementInput`'s field, as it already does for `use_pressed`. `drop_pressed` is an **edge bit**:
clear it in `held_gap_sim_command` and synthesize it
false in `neutral_sim_command` in `crates/postretro/src/netcode/command_queue.rs`, exactly as both
already treat `use_pressed`; `held_gap_sim_command` clears both the `SimCommand` field and its
`sim.movement` mirror, as it already does for `use_pressed` — missing one carries the press forward
silently. Do not carry it forward the way `reload` is carried — `reload` is a level
bit deduplicated by weapon-owned `reload_press_consumed`, and an edge bit held across a gap would drop one
weapon per held tick from a single press. A drop press discarded by the catch-up trim is lost, matching
`use`; do not extend reload's pre-trim edge lane.

Enumerate and update every struct-literal construction site — the field is not `Option`, so the compiler
lists them, as the parallel `use_pressed` field already forces at every site. This widens the transport wire, so bump `WIRE_VERSION` in `crates/net/src/handshake.rs` here,
from 15 to 16, extending its doc comment with this bump's reason. `WIRE_VERSION` is the governing
constant: `InputCommand` rides `ClientMessage` on the Input channel, and both its own doc comment ("the
field order is part of the wire layout (`WIRE_VERSION`)") and `networking.md` §Version gates place it
there, so the `SNAPSHOT_VERSION` bump-to-9 doc entry that mentions `use_pressed` does not generalize —
that is a historical bump note, the current value is 12, and `SNAPSHOT_VERSION` bumps only for changes
landing on the snapshot record. Do not fold the bump into Task
7's `TUNING_PAYLOAD_EPOCH` change: the two gates answer different questions and a peer can fail one while
satisfying the other.

### Task 5: World-item replication

Give world items the AI-enemy replication discipline. Add
`descriptor_materializes_world_item(descriptor) -> bool`, returning `descriptor.touchable.is_some()`,
beside `descriptor_materializes_ai_enemy` in
`crates/postretro/src/scripting/builtins/data_archetype.rs`, and add it to the client-side map-sweep
filter that already suppresses AI enemies there, so a connected client does not spawn its own copy of a
map-placed item. `filter_out_client_ai_enemies` no longer describes what it filters; rename it and its
callers in the same change.

Filtering a placement out of the client sweep also removes its model from the registry-driven upload set,
which is the exact regression `suppressed_ai_enemy_mesh_models` exists to prevent — see its doc comment.
Extend that collection to cover suppressed world-item placements too, so a dropped or host-baselined item
draws its real model rather than nothing.

Add a host registration sweep beside `host_register_map_enemies` in
`crates/postretro/src/netcode/replication.rs` that stamps a `NetworkId` and registers every entity
carrying a `TouchableComponent`, with the same stale-id unregister-and-forget prologue so a level reload
is idempotent.

Call it from **both** sites its enemy counterpart uses: `host_register_map_enemies_after_install`, invoked
from `crates/postretro/src/startup/lifecycle.rs`, and `host_register_map_enemies_after_fixed_sim_tick`,
invoked from the fixed-tick loop in `crates/postretro/src/main.rs`. The two wrappers share one
implementation in `main.rs`. The per-tick call is what makes
acquisition and drop replicate with no delta protocol: acquisition removes the `TouchableComponent`, so
the next sweep's stale-id prologue unregisters the item and forgets its mapping, sending the client a
despawn tombstone; drop attaches the component, so the next sweep stamps and registers it, sending a
baseline. There is no separate register/unregister path and nothing for the touch pass to report —
membership is derived from component presence, one mechanism rather than two. Held wieldables
are not replicated today — the only `replicable.register` calls naming a weapon are in
`netcode/lifecycle.rs` test fixtures — so acquisition ends an item's replicated life rather than
transferring it, and the client learns the pawn's new weapon through the tuning path in Task 7. Confirm
the client's apply path materializes an item baseline from `entity_class` plus `Transform` without
further metadata; that combination is already valid on non-despawn records.

### Task 6: Drop

Add the drop path to `crates/postretro/src/sim/touch.rs`, driven by the `drop_pressed` map Task 4
produces and evaluated at the top of the same pass, before any overlap or touch edges, so a drop and an
acquisition on one tick cannot interact. For each player with a drop edge, resolve the active slot's
wieldable and its descriptor first. A wieldable whose descriptor authors no touchable block cannot be
dropped — it could never be recovered — so refuse before releasing, warning once per descriptor and
leaving the inventory untouched. This refusal lives here, not in the chokepoint: whether a slot emptying
is a drop is the drop path's question. Otherwise call `release_wieldable` from Task 2 for that slot. On
success:

Write the freed item's `Transform.position` to a point one capsule radius in front of the pawn's facing,
at the pawn's capsule base. Sphere-cast that point against the `&CollisionWorld` the pass takes, using
the `parry3d` free functions `entity_model.md` §7 pins rather than a `QueryPipeline`; on a hit, fall back
to the pawn's `Transform.position` — the capsule centre — and sphere-cast that point too. The movement
solver keeps only the capsule *surface* clear of geometry by `SKIN_DISTANCE`, not the centre clear by a
full capsule radius, so the fallback point is re-tested rather than assumed clear: the pawn's own capsule
occupies that point collision-free this tick, and the second cast confirms a sphere of the drop radius
fits there. AC 15 forbids a placement inside geometry, so the landing point is verified, never asserted.

Restore the two world-item components Task 3 removed, both rebuilt from the descriptor the pass now
carries: a `TouchableComponent` from the touchable block, and the `MeshComponent` from the mesh block, so
the item is visible where it lands. A descriptor with no mesh block drops an invisible but acquirable
item, matching what it spawns as. Setting the component directly enqueues no spawn-context clip resolve,
so report the dropped item's `EntityId` on the pass's `TickEvents.dropped_item_meshes` vector alongside
`repointed`. `main.rs` drains it per fixed tick at the existing runtime-spawned-host-enemy site, extending
the `spawned_meshes` list it hands to `resolve_mesh_entity_bindings_for_entities` inside the tick loop —
not after it. Resolving an id whose `MeshComponent` was removed later that frame is a harmless no-op.

Force the released wieldable idle. The rule: restore every live-state field on `WeaponComponent` to its
spawn value, leaving descriptor tuning and the magazine intact. Nothing ticks a world item, so any
residual timer or consumed-edge flag would survive until the weapon is picked up again. Today that is
`transition_to_idle` in `crates/postretro/src/sim/weapon_stage/state.rs`, which writes `state`,
`state_remaining_ms`, `state_total_ms`, `state_elapsed_sub_ms`, and `reload_credited` — and nothing else.
It does not reset `cooldown_remaining_ms`, `shoot_press_consumed`, `reload_press_consumed`, or
`reload_feedback`, so drop calls `transition_to_idle` and then separately resets those four to their spawn
values. That two-part enumeration is a convenience; the rule governs, and a field added to the component
later joins it. `transition_to_idle` is a private `fn` in the private `weapon_stage::state` module, and
`sim::touch` is a sibling, so make it `pub(crate)` and re-export it from `weapon_stage` beside the existing
`pub(super) use commands::{…}` list; the four extra resets live in `sim::touch` beside the call. Add the holder to the tick's `repointed` list so the attachment
refresh detaches the dropped weapon in the same frame.

Seed the item's occupancy entry with **every player whose capsule overlaps the drop point**, not only the
dropper. A second player already standing there would otherwise take the item on the drop tick, and two
adjacent players dropping together would each get an enter edge on the other's item and swap weapons. The
seed suppresses enter edges only — a `press` item can still be taken by a deliberate press.

The existing `normalize_inventory_liveness` in `crates/postretro/src/sim/weapon_stage/commands.rs`
handles the pawn side of a vanished slot; releasing is not a vanish, so drop must leave the inventory
consistent itself rather than relying on that reconciliation.

### Task 7: Client inventory growth and release over the tuning channel

`materialize_net_local_wieldable_inventory_from_tuning` in
`crates/postretro/src/scripting/builtins/net_descriptor.rs` builds a client's local inventory only when
the pawn has none, and its slot-merge loop copies tuning fields onto slots the client has already filled,
skipping any slot the client holds as `None`. That skip is what makes a host-side acquisition invisible.

Change the merge loop in both directions. Where a tuning slot names a canonical name and the client's
slot is `None`, resolve the payload's canonical name through `find_descriptor`, synthesize a `MapEntity`
at the pawn's transform, and call `spawn_descriptor_instance` with `attach_weapon = true` and
`DescriptorSpawnPath::DefaultWeapon` — the same sequence `compose_wieldable_inventory_slots` runs, with
both helpers already imported in `net_descriptor.rs` — and place it with Task 2's `acquire_wieldable_at`
at the index the host named, so the two inventories agree by index. Where a tuning
slot is `None` and the client holds a wieldable, call `release_wieldable` and despawn the freed local
instance; the client owns that entity outright, and leaving it unreferenced leaks it for the level.

The merge reports no repoint. It runs inside the snapshot-apply stage, which `main.rs` drives before it
constructs the frame's `repointed_pawns` vector, so that route is closed to it — and it is not needed. The
client's third-person attachment already follows the host's replicated `active_weapon_archetype` on the
pawn record through the existing `local_weapon_attachments` path in the same apply stage.

Increment `TUNING_PAYLOAD_EPOCH` in `crates/postretro/src/netcode/tuning_payload.rs` from 2 to 3: the
layout is unchanged but the merge semantics are not, and the epoch gate is what stops a peer from
applying the old reading. This breaks the committed golden `payload_json_matches_committed_fixture` in
the same file, whose fixture embeds the old epoch; re-bless it with
`POSTRETRO_BLESS_COMPATIBILITY_FIXTURES=1`, the same way Task 1 names its typedef drift test.

For the send trigger, `host_send_tuning_if_changed` already dedupes by payload equality. Add a third call
site in `net_poll_and_apply` in `crates/postretro/src/main.rs`, iterating every participating slot's pawn
once per host poll, rather than adding a dirty-flag protocol. The two existing call sites both stay: the
accepted-client materialization in the same function, and the level-install publish in
`crates/postretro/src/startup/lifecycle_net.rs`. The equality check makes an unchanged inventory free **on
the wire** — nothing is sent — but each poll still pays one payload construction: `tuning_payload_for_pawn`
clones the pawn's `PlayerMovementDescriptor` and allocates up to ten canonical-name `String`s per call.
Accept that cost; an inventory change then publishes on the next poll with no new bookkeeping. That
trigger still escalates the channel: a config push at transitions
becomes an event-driven state channel with no delta and no ack, so a dropped payload leaves a stale
client inventory until the next publication. Accept it here — the payload is small, fixed-size, and
reliable-ordered — and hold the constraint `E15` set, that it stays opaque to `crates/net`. Route the
client's local `Inventory` writes through Task 2's chokepoint so the source-scan test stays satisfied.

### Task 8: Ordering and edge coverage

Build the ordering and edge-case coverage. Each scenario below states its expected outcome; assert that
outcome.

- Two players enter one item's radius on one tick. Exactly one acquisition. The nearest by squared centre
  distance wins, ties on the lower `PlayerId`, and the winner is the same across runs with identical inputs.
- One player enters two `auto` items on one tick with one free slot. The lower `EntityId` is acquired; the
  second evaluates with `free_slots = 0` and returns no effects.
- A frame renders zero fixed ticks. No evaluation and no prompt refresh; the last published prompt set
  stands for that frame.
- A frame renders two fixed ticks. The enter edge fires on the first; the second sees sustained overlap
  and applies nothing.
- A `use` press lands on the same tick as a trigger-volume Use activation. Both fire; neither consumes the
  press.
- A script queues `despawn(item)` on the tick the item is acquired. The weapon survives — acquisition
  purged the item's pending deferred effects — and later deferred effects against it still run.
- A `press` player overlaps two items and presses. Exactly one is taken: the nearest by squared centre
  distance, ties broken on the lower `EntityId`.
- A player drops and stands still. No enter edge, so no `auto` re-acquisition; a deliberate press on a
  `press` item does re-acquire.
- Another player already stands at the drop point. They get no enter edge on the drop tick and acquire
  only after leaving and returning, or by pressing a `press` item.
- Two adjacent players drop on the same tick. Each is seeded into both items' occupancy; neither gets an
  enter edge and no weapons swap.
- A player drops, walks out of radius, and walks back. Re-acquired on the new enter edge.
- A player dies while overlapping. The corpse keeps occupancy and stays a toucher: no enter edge while it
  lies still, but a corpse-held drop edge still drops and the corpse counts among a drop point's
  occupants.
- A drop press crosses a packet gap holding the command for several ticks. Exactly one drop.
- A weapon instance sits unowned in the world across many ticks. No cooldown, reload, or state timer
  advances.
- A weapon picked up during a level survives a level transition, with its magazine preserved.
- Picking up and dropping a weapon leaves every ammo-reserve balance on the pawn unchanged, including for
  an ammo type the pawn has never carried (AC 9).
- A client-side world item draws its authored model — both a map-baselined item and a host-dropped one —
  exercising Task 5's suppressed-mesh collection and Task 6's client-side clip resolve (AC 20).

Three tests carry more than their row. Write the unowned-inertness test against observable component
state after N ticks, never against the call sites — the invariant it pins is currently held only by two
call sites choosing not to scan the weapon column, and a test naming them would move with them. Cover the
facts by substituting a policy that acquires unconditionally and asserting a duplicate lands in the next
free slot: it proves the facts are built independently of what the default reads, which no test of the
default alone can show. Cover the no-re-fire threat by counting policy invocations against an item whose
evaluation returns an empty list, since with the default policy a spurious re-fire is otherwise
behaviorally invisible; that count is AC 19. Host-and-client cases belong in the existing netcode harness
style rather than a new fixture.

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

A second engine-owned reducer picks one winner among an item's contestants — nearest by squared centre
distance today, a fairness ranking later. It is a separate seam from the policy: the policy answers whether
a player wants the item, the reducer answers which wanter gets it.

Duplicate detection compares `DescriptorProvenance.canonical_name` between the item and each occupied
slot's wieldable. That field is what cross-level carry already harvests, so the two agree by construction
on what "the same weapon" means.

`trigger_system.rs` is 2400 lines but only ~625 are production code — the rest is its test module. It
needs no split before this work, and this spec adds nothing to it beyond making one helper visible.
`sim/weapon_stage.rs` is likewise a facade over submodules — ~15 production lines plus its test module.
Neither triggers the split-first rule.

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
- **Whether a deferred effect queued against a weapon while held should survive a same-level drop.** After
  a drop, `tick_deferred_effects` sees a world item and the queued effect fires against it. The
  acquisition matrix row purges the queue on acquire but says nothing about a held-queued effect surviving
  a later drop. Confirm this is intended, or purge the queue on drop the way acquisition does.
- **Whether the per-frame prompt sampling loss is acceptable.** The `prompts` Vec is overwritten each tick
  and read once per frame. In a two-fixed-tick frame a `press` item eligible on tick 1 but taken or
  despawned on tick 2 is never observed as eligible by presentation. Flag whether losing that eligibility
  window is acceptable, or whether prompts must accumulate across a frame's ticks.
- **How a contested pickup should choose among several players wanting it.** The default reducer picks the
  nearest, ties on the lower `PlayerId`. A fairer rule — the weapon to a player who lacks it, a granted
  resource to the player holding less — must rank the contestants, a scan over a player collection the IR
  substrate forbids a mod to author (`scripting.md` §11). The reducer stays engine-owned and the ranking is
  charted, not built, until the effect arm that makes owners contest (`grantAmmo`) and the collection-fact
  substrate both land.
- **Whether a deliberate press should beat an incidental walk-over for a shared last slot.** When a player
  presses a `press` item and steps onto an `auto` item on one tick with one free slot, the lower-`EntityId`
  item wins and the other takes nothing — deterministic but arbitrary. Reserving the slot for the press
  (deliberate action over incidental) is the alternative; it adds a pre-`auto` reservation step and is left
  out until playtest shows the `EntityId`-order outcome feels wrong.
