# Wieldables & Weapons — Design Exploration

**Date investigated:** 2026-05-23
**Status:** Pre-spec exploration. Not yet a draft plan. Captures the long-term
target so limited-scope sprints (starting with M10 weapon primitives) don't
paint the engine into a corner.

> **Read this when:** scoping any weapon, ammo, damage, inventory, augment, or
> loot work — or any *held item* (scanner, grapple, tool) — especially when a
> sprint deliberately ships a thin slice and you need to know which seams must
> stay open.
> **Key invariant:** a held item (a *wieldable*) is an *instance* that owns its
> own state, never fields stamped onto the player. A weapon is the first
> wieldable kind. Per-instance state is the foundation the looter-shooter vision
> rests on.
> **Related:** `context/lib/scripting.md` · `context/lib/entity_model.md` ·
> `context/lib/input.md` · `context/research/ui-layer.md` ·
> `context/plans/drafts/M10--weapon-primitives/`

---

## 1. Vision

The long-term target is looter-shooter weapons: a pistol and an SMG are
*archetypes*, but the pistol you're holding rolled +12% fire rate, has a
fire-element augment in slot 1, and a stability augment in slot 2 — and the one
on the floor over there rolled differently. Players carry several, switch
between them, pick new ones up, and re-roll/augment them. Each weapon consumes a
typed ammunition class drawn from a shared reserve — several weapons feed from
one pool by type — so loadout choices trade against a shared resource.

That demands per-instance state. Two SMGs cannot share one stat block. Any model
that stores "the weapon" as fields on the player collapses the moment a second
instance with different stats exists. So instance-owned state is the
load-bearing decision, and it's worth getting right before the first thin slice
ships — even though that slice has one weapon, no rolls, and no augments.

A weapon is not the only thing a player holds. The cyberpunk/immersive-sim
direction (scanners, hacking tools, grapples, deployables) and the loot system
itself (augment modules are owned, rolled inventory items, not weapons) mean the
held/inventory machinery must not be weapon-exclusive. Three layers:

- **Loot item** — anything owned, rolled, augmentable. Weapons *and* augment
  modules. The inventory holds these.
- **Wieldable** — a held item with a primary-use action and an optional
  secondary. The active-slot and switching operate on these. Weapon is the first
  kind; scanner/grapple/tool are future kinds.
- **Weapon** — the first wieldable specialization: fire mode, resolution, a
  damage payload, a consumable resource. "Fire" is weapon's flavor of
  primary-use; the resource is weapon's flavor of a consumable (ammo, heat, or a
  rechargeable cell — a tagged union, §3).

We do **not** build other wieldable kinds now — the hedge is purely about naming
the load-bearing machinery for *wieldables*, not *weapons*, so a second kind is
additive rather than a parallel system. This doc describes the target model,
then the invariants a thin slice must preserve, then how M10's scope maps onto it.

---

## 2. Core Principles

| Principle | Invariant |
|-----------|-----------|
| **Instance, not type.** | A wieldable is an entity instance with mutable, private state. The archetype is a shared template, not the thing the player holds. |
| **Wieldable layer, weapon kind.** | Identity, inventory, equip, switch, and augment slots belong to the *wieldable* layer. Weapon is the first specialization. Name the machinery for wieldables, not weapons. |
| **Primary and secondary use.** | A wieldable exposes a primary activation and an optional secondary, not one hardcoded fire path. Weapon: primary = fire, secondary = e.g. detonate/zoom/alt-fire. An activation may resolve a hit, play an effect, *or* spawn a persistent tracked entity. |
| **Declare, don't drive.** | Archetypes, augments, and rolls are data declared at load. The VM drops. Rust resolves stats and runs the fire tick. No live script during gameplay. |
| **Stats resolve through a seam.** | Fire logic reads *effective* stats, never the raw archetype field. The base → rolls → augments resolution sits behind that seam so it can grow without touching fire logic. |
| **Damage is a payload.** | A hit carries a structured damage payload, never a bare scalar. Types, crit, and falloff grow inside it. |
| **Augment behavior is data.** | Augments compose via stat deltas and pre-registered effect/reaction hooks — never live script callbacks. Preserves the no-live-VM model. |
| **One modifier system, not two.** | A visible attachment (scope, barrel) is an augment with a mount point + mesh — the same slotted-modifier machinery as an internal augment, never a parallel "attachments" system. Physical presence is the only difference. Cosmetic-only items (skins, charms) lean to a separate thinner layer. |
| **Activations carry their own outcome and emits.** | Each activation (primary/secondary) is the single source of truth for what it does *and* the events it fires. An activation resolves into a named outcome seam (`Damage` first; non-damage interactions are siblings), never a hardcoded damage path. |
| **One spawn path for held and dropped.** | Equip-at-spawn, pickup-off-the-floor, and switching all operate on wieldable instances through one spawn-and-activate path. |
| **ECS-inspired, not ECS.** | Components live in the registry keyed by `ComponentKind`; systems are hand-ordered functions in the fixed frame order. No archetype storage, query planner, or scheduler. Weapons fit this idiom — they don't justify a fuller ECS. |

---

## 3. The Model

Four concepts cover the surface. Archetype, instance, and augment are
wieldable-general; the stat/fire/resolution/ammo specifics below are the weapon
kind filling them in.

- **Archetype** — the template. Script-declared (`defineWeapon`-style, or a
  wieldable block on an entity definition). For a weapon: base stats, fire mode,
  resolution mode, a **resource** (the tagged consumable model — ammo, heat, or
  cell), per-activation `primary`/`secondary` blocks (each declaring its use and
  the events it emits), and the augment-slot count. Immutable. Many instances per
  archetype.
- **Instance** — a wieldable entity carrying its kind's component (a weapon
  carries a weapon component): a reference to its
  archetype, mutable runtime state (for a weapon: cooldown; later loaded
  magazine, heat, charge), instance rolls, and slotted augments. The reserve it
  draws from is not here — it's pooled on the inventory (§6). The entity id *is*
  the wieldable's identity — what inventory tracks, what a pickup is, what
  switching repoints to.
- **Augment** — a script-declared, slottable modifier. Carries stat deltas
  (additive *and* multiplicative) and/or behavior hooks (add a damage type, alter
  a fixed classifier like `reloadStyle`/`overheatBehavior`/resolution, attach an
  on-hit reaction). Composes onto an instance's slots. A *visible attachment* is
  an augment with a mount point + mesh — same machinery, plus physical presence
  (see below). Slots are typed (`SlotKind`) so a scope fits an optic slot, a
  barrel the muzzle.
- **Effective stats** — base archetype stats, with instance rolls applied, then
  augment modifiers applied. The fire system reads *this*. Resolve lazily and
  cache; invalidate when augments/rolls change.

```ts
// Proposed design — declared at load, VM drops after.
// String values are members of a generated union type (named in the comment),
// not free-form strings — same convention as existing descriptors
// (e.g. `primitive: "setLightAnimation"`), emitted by gen-script-types.
// Numerics are TS `number`; the Rust type each resolves to is noted per field.
defineWeapon({
  name: "smg",                 // string — canonical archetype name
  initialStats: {
    damage: 8,                 // number → f32 — base damage, pre-roll/augment
    range: 1200,               // number → f32 — world units; clamps the cast's max_toi
    fireRateMs: 90,            // number → f32 — ms; converted to ticks at materialization
    magazine: 30,              // number → u32 — rounds per magazine (ammo-kind stat)
    reloadMs: 1800,            // number → f32 — ms; reload *speed* is a stat
  },
  fireMode: "auto",            // FireMode = "semi" | "auto" — orthogonal, stays flat
  resolution: "hitscan",       // ResolutionMode = "hitscan" — orthogonal, stays flat
  augmentSlots: 2,             // number → u32 — slot count this archetype exposes
  // Resource is a tagged union: fields differ per kind, so a flat field set
  // would carry meaningless members (an `ammoType` on a heat gun). Tag it.
  resource: {
    kind: "ammo",              // ResourceKind = "ammo" | "heat" | "cell"
    type: "heavy",             // AmmoType union — key into the shared reserve pool
    reloadStyle: "magazine",   // ReloadStyle = "magazine" | "per-shell" | "internal" | "energy"
                               //   — a fixed classifier (a trait), not a number
  },
  // Per-activation blocks. `emits` is co-located on the activation that fires it
  // — there is NO top-level emits. The activation is the single source of truth
  // for what it does AND what it sounds like.
  primary: {
    use: "fire",               // ActivationUse = "fire" | "none" | (future) "scan" | …
    emits: {
      activate: "smgFire",     // EventName — &'static str the fire tick returns
      impact: "smgImpact",     // EventName — per-activation: a secondary resolves differently
    },
  },
  secondary: { use: "none" },  // a `use: "none"` activation carries no emits — no desync risk
})

// Resource variants (alternatives to the ammo block above):
//   Heat — passive dissipation + an overheat punish state, no reserve, no reload:
//     resource: { kind: "heat", overheatBehavior: "lockout" }
//       // overheatBehavior = "lockout" | "vent"
//     // its modifiable numbers live in initialStats: heatPerShot / overheatAt / ventRate
//   Cell — per-instance charge that depletes and passively regens, no reserve, no reload:
//     resource: { kind: "cell", regenDelayMs: 1500 }  // number → f32 — ms before regen starts

// A railgun shows charge as an activation axis (orthogonal to the resource —
// here charge + heat compose; a charged plasma bolt would be charge + ammo).
defineWeapon({
  name: "railgun",
  initialStats: { damage: 60, range: 4000, fireRateMs: 150,
    heatPerShot: 40, overheatAt: 100, ventRate: 25 },
  fireMode: "semi",
  resolution: "hitscan",
  augmentSlots: 2,
  resource: { kind: "heat", overheatBehavior: "vent" },
  primary: {
    use: "fire",
    charge: {                  // charge lives on the ACTIVATION, not the resource
      minMs: 200,              // number → f32 — fire floor; minMs:0 also tap-fires
      fullMs: 800,             // number → f32 — ms to reach charge level 1.0
      scales: ["damage", "range"], // StatId[] — charge level (0..1) scales these at release
    },
    emits: { activate: "railFire", impact: "railImpact" },
  },
  secondary: { use: "none" },  // releasing applies the resource cost (here, heat)
})

defineAugment({
  // Internal augment — no physical presence, just deltas + a behavior hook.
  name: "incendiary-core",     // string — augment identifier
  slot: "internal",            // SlotKind — constrains what fits where
  modifiers: [
    { stat: "damage", add: 2 },   // number → f32 — additive modifier
    { stat: "damage", mul: 1.15 },// number → f32 — multiplicative modifier
  ],
  onHit: [                     // ReactionDescriptor[] — the real reaction shape, not a bespoke DSL
    { primitive: "applyStatus", // PrimitiveName — a pre-registered Rust primitive
      tag: "@target",          // ReactionTag — same tag-targeting as UI reactions
      args: { kind: "burn",    // StatusKind union
        durationMs: 2000 } },  // number → f32 — ms
  ],
})

defineAugment({
  // Visible attachment — the SAME machinery, plus a mount point + mesh.
  name: "reflex-sight",        // string — augment identifier
  slot: "optic",               // SlotKind — an optic fits the optic slot
  mount: {                     // optional: present iff the augment is physically visible
    point: "optic-rail",       // MountPoint union — where on the weapon it attaches
    mesh: "reflex_v1",         // MeshId — the attachment's renderable
  },
  modifiers: [
    { stat: "adsSpeed", mul: 1.1 },
  ],
})

// Loop-closer: an extended magazine is a visible mount carrying a stat delta —
//   defineAugment({ name: "extended-mag", slot: "magazine",
//     mount: { point: "mag-well", mesh: "extmag_v1" },
//     modifiers: [{ stat: "magazine", add: 15 }] })
// This is exactly why `magazine` is a stat: the visible-mount and stat-delta
// decisions compose for free.
```

**Why `resource` is a tagged union, not flat fields.** `ammoType` and
`reloadStyle` were never general weapon fields — they're ammo-specific and
meaningless on a heat weapon. That's exactly when a tagged union beats flat
fields: members that don't apply to every variant. `fireMode` and `resolution`
stay flat top-level fields because they *are* orthogonal — a heat weapon still
has both. The modifiable *numbers* stay in `initialStats`, but the set is
resource-dependent: an ammo weapon carries `magazine`; a heat weapon carries
`heatPerShot` / `overheatAt` / `ventRate`. `reloadMs` is a stat (speed is a
number); `reloadStyle` is a fixed classifier (a trait), so it lives on the
resource, not in `initialStats`.

**Why `emits` is co-located on the activation.** Better than a top-level emits
keyed by primary/secondary: the activation is the single source of truth for
both what it does and what it sounds like. It can't declare emits for a `none`
secondary — no desync risk — and it generalizes to future wieldables (a
scanner's `primary: { use: "scan", emits: {…} }`). `impact` is per-activation
because a secondary resolves differently (an AoE blast vs. a hitscan impact).
The `activate`/`impact` strings are the same `&'static str` event names the fire
tick returns and the caller drains into the reaction system — the `landed` /
`jumped` pattern from the movement tick (§4).

**Why charge is an activation axis, not a resource kind.** Charge belongs to the
activation, orthogonal to the resource, because the two compose: a railgun is
charge + heat; a charged plasma bolt is charge + ammo (consumed at release).
Making charge a `resource.kind` would make those unrepresentable. Charge level
(0..1) scales the listed stats at release; `minMs` is a fire floor (`minMs: 0`
also tap-fires); releasing applies the resource cost.

**Why attachments and augments are one system.** Mechanically both are slotted
modifiers composing stat deltas + behavior hooks through the effective-stats
seam; the *only* difference is physical presence. Forking them would duplicate
composition, resolution, slot-constraint, and invalidation logic — the exact
parallel-system asymmetry the "name the machinery generically" principle exists
to prevent. The real structure the unification needs is **slot typing with mount
constraints** (`SlotKind` decides a scope fits an optic slot, a barrel the
muzzle). Augments can also alter *fixed classifiers* through behavior hooks
(swap `reloadStyle`, change `overheatBehavior`, retune resolution), not only
apply stat deltas. **Purely cosmetic items** — skins, charms: mesh/material,
zero deltas, no hooks — should lean toward a separate, thinner cosmetic layer
beside the augment system, so the augment path always implies "affects effective
stats or behavior."

```
archetype (template)
   │  + instance rolls
   │  + slotted augments
   ▼
effective stats ──read by──▶ fire tick ──emits──▶ activation outcome (Damage payload | Effect | Spawned)
                                          └──────▶ activate / impact sound events
```

The fire tick reading *effective stats* (not the archetype) is the seam that
turns rolls and augments into additive changes rather than a rewrite. In Rust
that seam is an accessor, not a stored field. Resolution order among augments —
and additive-vs-multiplicative application order — is an open fork (§9); pin it
before this is durable:

```rust
// Proposed design — the seam, not a layout.
// For M10 this is identity passthrough (base stats, no rolls/augments yet);
// rolls and augment modifiers slot in here later without touching the fire tick.
// Resolve lazily, cache the result, invalidate when rolls/augments change.
impl WeaponComponent {
    fn effective(&self) -> EffectiveStats { /* base → rolls → augments; cached */ }
}
```

---

## 4. Fire Path and Damage Payload

Firing is the weapon kind's primary-use activation; a wieldable also has an
optional secondary activation (alt-fire), and a future wieldable (scanner,
grapple) resolves its own activations differently. The weapon fire tick is stable
across the whole vision: read effective stats, gate on cooldown per fire mode,
resolve the shot (hitscan now; projectile is a sibling resolution mode), produce
an outcome on contact, emit the activation's `activate` / `impact` sound events.

An activation does not have to resolve into an immediate hit. It may instead
spawn a **persistent tracked entity** the instance owns — a launched charge, a
deployable — and a *secondary* activation can act on those tracked entities
later (the detonator pattern). This is distinct from the transient, self-cleaning
impact effect: a tracked entity lives in the world with its own state until the
weapon resolves it. The live set is instance-owned state, so each instance only
acts on the entities *it* placed.

An activation does not always resolve into damage. The tick returns an
**`ActivationOutcome`** — `Hit(DamagePayload)` for a resolved shot, `Effect(…)`
for a non-damage interaction, or `Spawned(EntityId)` for a launched charge or
deployable (the detonator / tracked-entity path). Damage is the first variant,
symmetric to "wieldable, not weapon": an immersive-sim scanner reveals, a hack
tool flips entity state, a gravity tool imparts impulse — each a sibling outcome,
not a separate consumer path bolted on later (§7).

The `Hit` variant carries a **damage payload**, not an amount. Today the payload
may hold one field; over time it grows damage type, crit flag, and range
falloff. The health/kill consumer (enemy-entity plan) reads the payload. Making
it a struct from the first slice means damage types and crit never force a
signature change through every consumer.

**Heat is not the fire-rate cooldown.** `fireRateMs` is the inter-shot gate
every weapon already has; heat is an *additional* layered gate with an overheat
punish state — it sits on top of the cooldown, not in place of it.

**The resource updates every tick, not only on fire.** Heat dissipates, cells
regen, charge bleeds off — independent of input, analogous to gravity in the
movement tick applying every frame whether or not a key is pressed. So the
resource needs a per-tick update branch, not only a fire-time decrement.

**Per-shell reload is a cancellable state machine.** Per-shell reload (the
shotgun case — quintessential to boomer shooters) is interruptible mid-cycle and
loads N rounds one at a time toward `magazine`; `magazine`-style reload is
atomic. The fire-tick state machine must model a cancellable reload for the
per-shell style. Shotguns are why `reloadStyle` earns a place in the long-term
vision.

The tick is a plain function in the game-logic stage — mirroring the existing
movement tick (`run_movement_tick`), not an ECS query/scheduler. It snapshots the
active instance's component, runs a pure tick, writes the result back, and
**returns a `Vec<&'static str>` of event names** the caller drains and fires
through the reaction system — the `landed` / `jumped` pattern:

```rust
// Proposed design — access shape, not field layout. Mirrors run_movement_tick.
// See context/lib/entity_model.md for the registry / Component idiom.

// Greenfield — no Damage/Health type exists yet. A struct from day one so
// damage type / crit / falloff grow without a signature change through consumers.
struct DamagePayload { amount: f32 }

enum ActivationOutcome {
    Hit(DamagePayload),  // resolved shot — the first and only M10 variant
    Effect(/* … */),     // non-damage interaction (scan, hack, impulse) — sibling seam
    Spawned(EntityId),   // launched charge / deployable — the tracked-entity path
}

fn weapon_fire_tick(
    reg: &mut EntityRegistry,
    input: &ActionSnapshot,        // reads primary vs. secondary activation
    collision_world: &CollisionWorld,
    dt: Tick,
) -> Vec<&'static str> {           // event names → caller drains into reaction system
    let mut events = Vec::new();
    let weapon_id: EntityId = /* player's active-wieldable reference (a weapon here) */;
    let mut weapon = reg.get_component::<WeaponComponent>(weapon_id).clone();

    // Every tick, regardless of input: heat dissipates / cell regens / charge bleeds.
    weapon::tick_resource(&mut weapon, dt);

    // primary vs. secondary read off the snapshot; gate on cooldown (fireRateMs)
    // then the resource's own gate (heat overheat / ammo / cell) before firing.
    let outcome: Option<ActivationOutcome> =
        weapon::tick_fire(&mut weapon, input, collision_world, dt, &mut events);
    match outcome {
        Some(ActivationOutcome::Hit(payload)) => { /* feed health/kill consumer */ }
        Some(ActivationOutcome::Spawned(id))  => { /* track on this instance */ }
        _ => {}
    }

    reg.set_component(weapon_id, ComponentValue::Weapon(weapon));
    events
}
```

No archetype iteration, no system registration — one ordered call, like the
other game-logic systems.

---

## 5. Augments Without a Live VM

The risk augments introduce: "on hit, apply burn" *sounds* like a runtime
script callback, which would reintroduce a live VM. It must not. Augment
behavior is expressed the same way UI events are (see `ui-layer.md`):
pre-registered, tag-targeted reactions and typed effect descriptors, declared at
load and executed by Rust. An augment's `onHit` is a list of `ReactionDescriptor`
entries (`{ primitive, tag, args }` — the same shape level reactions use), not
author code that runs mid-frame and not a bespoke per-augment DSL. A bespoke DSL
would quietly propose a second reaction system; reusing the descriptor shape is
the whole point. Banking this now keeps the declarative model intact when
augments land.

This model is weapon→target: an activation hits, the reaction applies to the
target. **Emergent world→world reactions** — fire ignites nearby gas, EMP shorts
electronics — are a different shape (a status change near a flammable triggers a
secondary reaction independent of the weapon). Whether that's in scope is an
undecided fork (§9); it would require reaction triggers to fire off *entity state
changes*, not just weapon activations.

---

## 6. Equip, Inventory, Pickup, Switch

All four are operations on wieldable instances — a weapon today, any wieldable
kind later:

- **Equip-at-spawn** — the player descriptor names a starting wieldable; spawn
  that instance and set it active.
- **Pickup** — the *same* instance, but placed in the world with a transform and
  a trigger; on pickup it joins the inventory.
- **Switch** — repoint the player's active-wieldable reference. Per-instance
  state (cooldown, ammo, augments) is preserved because each instance keeps its
  own.
- **Inventory** — the loot items the player owns. Wieldables (weapons, future
  tools) are the equippable subset; augment modules are owned but not wielded.
  The active reference selects one wieldable. Ammunition reserves are pooled here
  by ammo type — shared across every instance that uses that type. The loaded
  magazine stays per-instance.

Because equip-at-spawn and pickup spawn the *same* kind of thing, they should
share one spawn-and-activate path. Per `ui-layer.md`, the active wieldable's
ammo/heat/charge surface as engine state values the HUD binds to.

---

## 7. Load-Bearing Invariants (corner-avoidance checklist)

Any thin slice — M10 included — must preserve these, or it forces a rewrite:

1. **Wieldable state lives on a wieldable instance (entity), not on the player.**
   The player holds an *active-wieldable reference*, not weapon fields.
2. **The fire system reads effective stats through an accessor**, never the raw
   archetype field — even when resolution is an identity passthrough today.
3. **The hit/damage payload is a struct**, never a bare scalar.
4. **Augment-style behavior, when it lands, is data/reactions, not live script.**
5. **Archetype is a template, not identity.** Multiple instances per archetype;
   identity is the instance.
6. **Held and dropped wieldables are the same instance kind**, reachable by one
   spawn path.
7. **Inventory, equip, and switch are named for wieldables, not weapons.** A
   weapon is the first wieldable kind, never the only one the machinery knows.
8. **An activation resolves into a named outcome seam, not a hardcoded damage
   path.** The consumer side is `ActivationOutcome` with `Damage` as the first
   variant — symmetric to "wieldable, not weapon." A non-damage interaction
   (scan, hack, impulse) is a sibling variant, never its own consumer path bolted
   on later. M10 implements only the `Damage` variant; the seam stays open.

Violating any of these is cheap to avoid now and expensive to retrofit later.

---

## 8. M10 Mapping (the first thin slice)

The §3 sample is the **long-term vision shape** — the full resource union,
per-activation blocks, charge, and the unified augment/attachment model. This
section governs what M10 actually builds *first*. M10 weapon primitives builds,
consistent with the invariants. The wieldable layer stays thin — just enough for
one kind — so weapon is concrete while the machinery is named generically:

- A weapon **archetype** descriptor and a weapon component on a wieldable
  **instance entity** (not the player).
- Player descriptor names a default weapon → spawn the instance, set the
  active-wieldable reference. (The future pickup/switch path, used for one kind.)
- Fire tick reads **effective stats**, which for M10 equal base stats —
  resolution is an identity passthrough, but the seam exists.
- Fire tick **returns event names** the caller drains into the reaction system —
  the `landed` / `jumped` shape from the movement tick.
- An activation resolves into an **`ActivationOutcome`** — M10 builds only the
  `Hit(DamagePayload)` variant; the seam stays open for the rest.
- Hit carries a **damage payload struct** (amount only for now).
- Typed `activate` / `impact` sound events, co-located on the primary activation.

M10 leaves these as open seams — not built, not blocked:

- Instance rolls, augments, and any non-passthrough stat resolution.
- Inventory, switching, switch input, pickups.
- The full **resource discriminated union** (ammo / heat / cell); ammo's
  pooled-by-type reserve and magazine.
- **`reloadStyle`** and the cancellable **per-shell reload** state machine
  (atomic magazine reload vs. per-shell).
- **Charge-on-activation** (charge level scaling stats at release).
- **Visible-mount / attachment augments** (mount point + mesh) and the separate
  thin **cosmetic layer** (skins, charms).
- **`ActivationOutcome` variants beyond `Damage`** (`Effect`, `Spawned`).
- **Emergent world-state reactions** (world→world; pending the §9 fork).
- Damage types, crit, falloff (grow inside the payload).
- Projectile resolution mode (sibling to hitscan).
- Secondary activation (alt-fire), and primary-use that spawns a persistent
  tracked entity (charges, deployables) for a secondary to resolve later.
- Area-of-effect damage (radial volume query emitting a payload per target).

---

## 9. For Discussion (genuine forks)

These aren't decided — they want a human call before this becomes durable:

- **Stat representation: named fields vs. open stat map.** Looter augments that
  touch arbitrary stats lean toward a `StatId → value` map; M10 simplicity leans
  toward named fields. Proposed middle path: start with named fields behind the
  effective-stats accessor; promote to a map when the first augment that needs an
  unanticipated stat actually lands. Open: is that promotion cheap enough, or do
  we eat the map's cost up front?
- **Where the archetype is declared:** a dedicated `defineWeapon` vs. a wieldable
  block on the existing entity-definition surface. Affects how a pickup entity
  and a weapon archetype relate.
- **How the wieldable/weapon split is typed:** a wieldable marker plus a per-kind
  component (weapon component, later scanner component) vs. one component with a
  kind tag. M10 has one kind and can defer the choice — but the inventory and
  active-slot machinery must key on "wieldable instance," not on the weapon
  component specifically, so a second kind drops in without rework.
- **Augment behavior surface:** how rich the `onHit`/`onFire` reaction hooks
  need to be, and whether they reuse the UI/reaction registry wholesale.
- **Roll model:** are rolls fixed deltas baked at instance spawn, or live
  modifiers re-resolved each shot? Affects whether effective-stat caching is
  optional or required.
- **Ammo reserve model:** pooled by ammo type (shared across weapons of that
  type — the dominant looter pattern, and what "ammo *type*" implies) vs.
  per-weapon-instance reserves. Lean pooled; the type is the sharing key. The
  loaded magazine stays per-instance either way.
- **Augment resolution order.** `effective()` says "base → rolls → augments" but
  doesn't define the order *among* augments, nor the additive-vs-multiplicative
  application order — a notorious looter balance footgun (the Borderlands-style
  additive-pool vs. multiplicative-pool question). This must be pinned before the
  effective-stats seam is durable. It couples to the roll model and the
  effective-stat-caching fork above — decide all three together. (§3's
  `effective()` accessor is where this lands.)
- **Emergent world-state reactions: in scope?** Immersive sim's signature is
  emergence — fire ignites gas, EMP shorts electronics, water conducts shock.
  The §5 reaction model is weapon→target; emergence is world→world (a status
  change near a flammable triggers a secondary reaction independent of the
  weapon). The existing `ReactionDescriptor` system *could* support this — but
  only if reaction triggers can fire off **entity state changes**, not just
  weapon activations. The doc never states emergent interaction as a goal. Lean
  toward "in scope" (immersive sim implies it), but this wants a human call: if
  yes, reaction triggers gain an entity-state-change source and it becomes an
  invariant — do not commit it silently.

## 10. Stress Test — Remote-Detonated Explosive

A weapon that launches charges and detonates them on a separate trigger, to
check the model holds against a non-trivial weapon:

| Mechanic | Maps onto |
|---|---|
| Wield the launcher | A weapon-kind wieldable; nothing new. |
| Primary-use launches a charge | Projectile resolution mode, but on impact the activation outcome is `Spawned` — a **persistent tracked entity** (the armed charge) — instead of `Hit`. |
| Track placed charges | The instance's live-charge set — instance-owned mutable state. |
| Detonator | **Secondary activation** (alt-fire): iterate the instance's tracked charges and resolve them. |
| The explosion | Area-of-effect — radial volume query emitting a **damage payload per target**; distance falloff is the payload's falloff field. The transient impact burst generalizes to the explosion effect. |

Verdict: no invariant bends. Two pieces are additive seams the model now names
(secondary activation; primary-use spawning a persistent tracked entity), plus
AoE damage resolution, which extends the enemy-entity plan's volume query from
ray to radius. The notable confirmation: **per-launcher detonation falls out of
instance-owned state for free** — two launchers detonate their own charges
because each instance tracks its own set. A player-fields model could not.

---

## Non-Goals

This doc is exploration, not a spec. It does not define struct layouts, field
names, module paths, or task breakdowns — those live in the per-sprint plans
under `context/plans/`. It defines the target shape and the invariants that keep
the thin slices compatible with it.
