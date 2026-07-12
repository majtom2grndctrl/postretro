# E16 - Ammo Resource

> **Status:** in-progress.
>
> **Epic:** 16 - Combat.
>
> **Milestone:** Weapon Systems.
>
> **Builds on (shipped):** the **Client-Authoritative Combat** spec (Epic 16,
> `plans/done/`). It already introduced the `reload` field on `SimCommand`
> (`crates/postretro/src/sim/mod.rs:36`) — a **held level bit**, not a rising
> edge — its wire carry (`input_command_to_sim` / `sim_command_to_input`,
> `crates/postretro/src/netcode/wire_convert.rs`), the `neutral_sim_command`
> default (`crates/postretro/src/netcode/command_queue.rs:452`), and the
> per-pawn reload delivery seam `deliver_reload_to_weapon`
> (`crates/postretro/src/sim/mod.rs:424`). Both the local/host pawn
> (`sim/mod.rs:200`) and remote co-op pawns (`run_remote_weapon_commands`,
> `sim/mod.rs:382`) route reload intent through that seam. This spec makes the
> seam own reload timing and ammo transfer. The production host resolver
> `host_resolve_remote_commands`
> (`command_queue.rs:396`) already resolves the full per-pawn `SimCommand`
> (movement + fire + reload); there is no movement-only limitation to work
> around.
>
> **Fits first:** front-loads the hard-to-reverse resource tagged-union
> discriminant and the magazine/reserve/reload contract. Later `heat` / `cell`
> land as sibling variants and the discriminant is expensive to change
> afterward. `onKill` then consumes the clean resource-grant seam this leaves
> open (grant-ammo-on-kill), but this ships no grant policy.

## Goal

Add the first variant of the weapon `resource` tagged union: per-weapon-instance
ammo. A weapon draws from a **magazine** (currently-loaded rounds, live
per-instance state), backed by a **reserve pooled by ammo type** (the union
discriminant keys the pool). Firing consumes the magazine at the activation
chokepoint; an empty magazine blocks the shot and surfaces the block rather than
silently no-opping.

**Reload** is a **timed** transfer from the reserve into the magazine: a reload
press starts a reload timer sized by an **effective reload duration**, and on
completion performs a single atomic transfer. The duration is a Borderlands-style
stat — authored per weapon as milliseconds, augmentable, and later enhanced
through a scripting-driven progression system (the effective-stat seam
accommodates it; the progression system itself is not built here). Reload is
**non-cancellable and not per-shell** — it is one timed transfer, not an
incremental shell loop.

The live magazine and reserve feed a real `player.ammo` HUD readout; the reload
timer drives the already-shipped crosshair reload meter (`player.reloadProgress`
/ `player.reloadActive`) for real, retiring its dev-only stand-in producer with
no UI rework.

This ships no grant/reward policy and no world pickups. It leaves the
resource-grant chokepoint (inverse of `applyDamage`) as a clean seam a later
`onKill` spec fills.

## Scope

### In scope

- A `resource` field on `WeaponDescriptor`, a serde-tagged union keyed by `kind`
  with a single `ammo` variant now; absent = the current unlimited-fire weapon.
  Parsed in both descriptor runtimes and emitted in generated SDK types.
- The ammo variant carries the ammo `type` (reserve pool key), magazine
  `magazine` (capacity), `costPerShot`, a starting `reserve`, and the base reload
  duration `reloadMs`.
- Magazine capacity, per-shot cost, and reload duration flow through the
  `WeaponComponent::effective()` / `EffectiveStats` seam (so later
  augments/attachments/progression modify them), extended the same way
  `credit_source` was added in the ledger spec. **Reload speed is a stat:** the
  authored `reloadMs` is the base; producers read the *effective* reload
  duration, never the raw field — the seam a later progression/augment layer
  modifies.
- A live `magazine` count and a live reload timer (`reload_remaining_ms` +
  `reload_total_ms`) on `WeaponComponent`, preserved across hot reload like
  `cooldown_remaining_ms`; capacity/cost/type/reload-duration refresh from the
  descriptor.
- A pawn-owned reserve component pooling rounds by ammo type. Seeded at
  equip-at-spawn from the weapon descriptor's starting reserve; the magazine
  materializes full.
- Firing consumes `costPerShot` from the magazine at `weapon::tick_resolved`. An
  empty magazine blocks the activation and surfaces it (a `dry_fire` event +
  `WeaponFireAuthorization::Empty`), consuming no ammo and dealing no damage.
  Empty activations use the effective fire interval, so a held Auto trigger emits
  at weapon cadence rather than every fixed tick. Firing is also blocked while a
  reload is in flight (reload is non-cancellable).
- Reload (`Action::Reload`, already bound) as a timed transfer. On the rising
  edge of the reload command the weapon starts a reload timer from the effective
  reload duration (guarded: a fresh press while already reloading is a silent
  no-op — the rising-edge dedup; a full magazine or empty reserve is a distinct
  blocked outcome). The timer advances per fixed
  tick; on completion it performs one atomic transfer of
  `min(capacity - magazine, available(type))` from the pawn reserve pool into the
  magazine. The reload seam is the shipped `deliver_reload_to_weapon`, which both
  the local/host pawn and remote co-op pawns already route through — filling it
  wires reload for every pawn.
- A real producer for `player.reloadProgress` (= `1 - reload_remaining_ms /
  reload_total_ms`) and `player.reloadActive` (reloading), published each frame
  from the active wieldable's reload timer, replacing the dev-only
  `DevReloadProgressDriver`. The producer **always** writes
  `reloadActive = false` / `reloadProgress = 0` when idle so the meter's retained
  `exitFade` triggers.
- Live `player.ammo` (magazine) and `player.ammoReserve` (active weapon's pool)
  engine-owned HUD slots, a publisher mirroring `PlayerHudStatePublisher`, and a
  restored ammo readout in the dev HUD.
- Flipping `player.reloadProgress` / `player.reloadActive` from
  `ReplicationScope::None` to `OwnerPrivatePlayer`, plus a per-owner projection
  for `player.ammo` / `player.ammoReserve` / `player.reloadProgress` /
  `player.reloadActive` for co-op remote-client pawns, mirroring the
  weapon-cooldown and health projections. Single-player and the host's own pawn
  already get correct values from the publisher alone. (This is the per-owner
  projection the shipped reload-feedback plan explicitly deferred to the real
  producer.)
- Tests: descriptor parse/validate both runtimes (incl. `reloadMs`), SDK type
  generation, magazine + reload-timer seed and hot-reload preservation, reserve
  seed, fire consumption, empty-magazine block, fire-blocked-while-reloading,
  timed atomic reload transfer at completion, reload block outcomes (full / empty
  reserve), unlimited-fire back-compat, HUD publish (ammo + reload progress),
  reload-meter idle write, dev-driver removal, per-owner projection for ammo and
  reload.

### Out of scope

Each is its own later roadmap bullet; the shape only accommodates them.

- `heat` and `cell` resource variants (sibling union variants — later Weapon
  Systems spec) and the per-tick *resource* update they need (heat dissipation,
  cell regen). The reload **timer** advance is in scope; the deferred per-tick
  update is the heat/cell resource decay, not the reload countdown.
- Per-shell / incremental / cancellable reload and the `reloadStyle` classifier.
  A **timed, non-cancellable, single-transfer** reload IS in scope (this is what
  the meter needs); only the per-shell classifier and its state machine are
  deferred (per-shell reload spec).
- The scripting-driven **progression system** that enhances reload speed (and
  other effective stats). This spec seats reload duration on the effective-stat
  seam so a later progression/augment layer can modify it; it builds no
  progression, augment, or attachment.
- Weapon switching and inventory. The reserve lives on the pawn as an
  inventory precursor; the switching+inventory spec relocates it and repoints
  `active_wieldable`.
- World ammo-pickup entities (pickup spec). Descriptor-seeded reserve is the
  only ammo source here — see the seeding rationale below.
- Dual-wield, augments/attachments, charge-on-activation, secondary/alt
  activation, viewmodel.
- The resource-grant chokepoint itself and any `onKill` / `onImpact` / `onDamage`
  dispatch. Named as consumers of the grant seam; not built. No ammo is granted
  by any event here.
- A generated `AmmoType` union backed by a declared-ammo-type registry — the
  type validates as a free-form ASCII identifier now (see design decisions below).
- Client-side prediction/reconciliation of the reload timer. Host-side reload
  application already exists (`deliver_reload_to_weapon`, routed for own and
  remote pawns) and this spec fills its transfer; per-owner projection surfaces
  the authoritative reload state to each owner. Predicting a remote client's own
  reload locally and reconciling it (the movement-prediction analog) is a
  follow-up, not built here.

## Acceptance criteria

- [ ] Authors can set `components.weapon.resource` (an `ammo`-kind block) in
  TypeScript and Luau. An absent `resource` parses to `None`. Rejection happens
  at two layers: (a) serde-deserialize rejections — unknown `kind`, or a
  wrong-type or negative value for any `u32` field (`magazine`, `costPerShot`,
  `reserve`, `reloadMs`) — fail before `WeaponDescriptor::validate()` runs; (b)
  `validate()` rejections — empty/overlong/illegal `type` (charset),
  `magazine < 1`, `cost_per_shot < 1`, `reload_ms < 1`. Both layers reject
  identically across the QuickJS and Luau runtimes, which are behavioral twins
  for descriptor parsing (`scripting.md` §1).
- [ ] Generated TypeScript and Luau SDK types include the `resource` tagged union
  on `WeaponDescriptor` with an `ammo` variant carrying `type`, `magazine`,
  `costPerShot`, `reserve`, `reloadMs`, all camelCase, identical in both runtimes.
- [ ] A weapon with `resource: None` fires with no magazine gating and cannot
  reload — the current single-weapon fire tests pass unchanged (back-compat).
- [ ] At equip-at-spawn a weapon with an ammo resource materializes with a full
  magazine (`= magazine` capacity), an idle reload timer, and the pawn's reserve
  pool for that ammo type credited the descriptor's starting `reserve`.
- [ ] Firing consumes `costPerShot` from the magazine; the shot resolves exactly
  as today (hit-zone multiplier, ledger attribution, impact FX unchanged). The
  reserve is untouched by firing.
- [ ] With magazine `< costPerShot` the trigger blocks: no shot resolves, no ammo
  is consumed, no damage is applied, and the block is observable. The caller-drain
  signal is the `dry_fire` event (via `event_names()`); the internal
  fire-state seam returns `WeaponFireAuthorization::Empty`. A dry fire builds no
  `WeaponImpact`; it arms the effective fire interval and emits once per interval.
  It is not a silent no-op. Cooldown-blocked shots stay silent as today. While a
  reload is in flight the trigger is likewise blocked silently and does not cancel
  the reload.
- [ ] Reload is timed by the **effective** reload duration: on a fresh reload
  press (rising edge of the held `SimCommand.reload` bit) the reload timer starts
  from `effective().reload_ms`; a held reload button starts exactly one reload.
  The timer advances per fixed tick, and on completion transfers
  `min(capacity - magazine, available(type))` rounds from the pawn reserve pool
  into the magazine in one atomic step. A reload attempt with a full magazine or
  an empty reserve pool is a distinct blocked outcome (no timer started), not a
  partial or silent transfer.
- [ ] Hot reload preserves the live `magazine` count, the in-flight reload timer,
  the `reload_press_consumed` edge flag, and cooldown through
  `refresh_from_descriptor` while updating authored capacity/cost/type/reload-duration
  — an implementation that resets the magazine to full or aborts an in-flight
  reload on descriptor reload fails this criterion.
- [ ] `player.ammo` reflects the active wieldable's live magazine and
  `player.ammoReserve` reflects its ammo type's reserve pool, republished each
  frame; both are readonly engine-owned slots the dev HUD reads through
  `getGameState().player`. Correct for single-player and the host's own pawn via
  the publisher; correct per-owner values for a co-op remote client's pawn need
  the projection (Task 7). With the active weapon's `resource: None` or no active
  wieldable (no pawn / fly-camera), the publisher skips the ammo write, matching
  the health publisher — the slots keep their last value rather than publishing a
  stale 0.
- [ ] `player.reloadProgress` ramps `0 → 1` over the effective reload duration
  and `player.reloadActive` is true only while reloading; unlike the ammo slots,
  the reload producer writes `reloadActive = false` / `reloadProgress = 0` every
  idle frame (no active reload) so the retained `exitFade` triggers rather than
  latching. The dev-only `DevReloadProgressDriver` is removed and is no longer the
  producer. The automated slice is the producer's slot lifecycle (progress ramp,
  active-only-while-reloading, idle false/0 write), unit-tested via slot reads like
  the reload-feedback driver test; the `hud.reloadMeter` bar fill and exit-fade is
  retained-UI presentation (`ui.md` §3), verified by manual dev observation as the
  shipped reload-feedback plan did.
- [ ] No ammo-grant, `onKill`, resource-grant, or progression behavior runs as
  part of this plan (review/grep gate).
- [ ] No heat/cell variant, per-shell / cancellable reload, `reloadStyle`,
  pickup, or inventory is built (review/grep gate). No new `unsafe`
  (review/grep gate).
- [ ] A net-slot pawn materializes host-side with its descriptor-seeded reserve
  and a full magazine (Task 3 seeding). A remote client's reload, routed through
  the shipped `deliver_reload_to_weapon` seam this spec fills, draws against this
  same host-side reserve.
- [ ] A co-op remote client observes their own pawn's `player.ammo` /
  `player.ammoReserve` / `player.reloadProgress` / `player.reloadActive`, not the
  host's, via the per-pawn projection (Task 7).

## Tasks

### Task 1: Resource tagged union on the descriptor + SDK types

Add `resource: Option<WeaponResource>` (with `#[serde(default)]`, matching the
sibling `credit_source` field at `combat.rs:35`, so an absent `resource`
deserializes to `None`) to `WeaponDescriptor`
(`crates/foundation/src/data_descriptors/types/combat.rs:28`).
`WeaponResource` is a serde-tagged (`#[serde(tag = "kind", rename_all = "camelCase")]`)
enum with one variant, `Ammo(AmmoResource)`; the tag reserves `heat`/`cell` for
siblings. `AmmoResource` carries `ammo_type` (wire `type`), `magazine` (u32
capacity), `cost_per_shot` (u32, wire `costPerShot`, `#[serde(default =
"default_cost_per_shot")]`), `reserve` (u32 starting pool), and `reload_ms` (u32,
wire `reloadMs`, base reload duration in milliseconds,
`#[serde(default = "default_reload_ms")]`). Define `fn default_cost_per_shot() ->
u32 { 1 }` and `fn default_reload_ms() -> u32 { 1000 }` in `combat.rs`, matching
the `default_credit_source` convention (defined in `weapon.rs:102`). Validate in
`WeaponDescriptor::validate()` (`combat.rs:40`): `type` an ASCII identifier
matching the existing `credit_source` charset (`[A-Za-z0-9_.:-]`, ≤64 bytes,
non-empty, `combat.rs:65-97`), `magazine ≥ 1`, `cost_per_shot ≥ 1`,
`reload_ms ≥ 1`, `reserve` any u32. The POD references no `EntityId`, so it stays
in `postretro-foundation` per the descriptor partition rule (`scripting.md`).

Parsing is free once the field is serde — `entity_descriptor_from_js`
(`crates/scripting-core/src/data_descriptors/js/entity.rs:14`) /
`entity_descriptor_from_lua` (`.../lua/entity.rs:11`) deserialize
`WeaponDescriptor` via `serde_json`. Register SDK types in
`crates/postretro/src/scripting/primitives/mod.rs`, in order: first
`register_type("AmmoResource")` with its
`type`/`magazine`/`costPerShot`/`reserve`/`reloadMs` fields, then
`register_tagged_union("WeaponResource")` with the `ammo` variant pointing at
that now-registered `AmmoResource` type (mirroring the
`register_tagged_union("ComponentValue")` registration at `primitives/mod.rs:52`),
and a `resource?` field on the `WeaponDescriptor` registration (`:250`).
Regenerate committed SDK fixtures.

`WeaponDescriptor` is not `Default`, so adding `resource` requires updating every
`WeaponDescriptor` struct literal (production + tests); `Option` keeps existing
literals a one-field addition (`resource: None`).

### Task 2: Magazine + reload-timer state + effective() extension

`effective(&self)` takes no descriptor
(`crates/entities/src/components/weapon.rs:63`), so — exactly as `damage` /
`range` / `credit_source` are stored — `WeaponComponent` must also store the
raw ammo tuning to return it from `effective()`, not just the live state.
Add to `WeaponComponent`:

- a stored ammo tuning `Option<{ ammo_type, capacity, cost_per_shot, reload_ms }>`
  (absent when `resource: None`) — authored, refreshed on hot reload;
- the live `magazine: u32`;
- the live reload timer `reload_remaining_ms: u32` (0 = not reloading) and
  `reload_total_ms: u32` (the effective duration sampled at reload start, so
  progress reads `1 - remaining / total`), plus a serde-defaulted fractional
  elapsed-millisecond carry. The carry prevents fixed-tick rounding drift while
  keeping HUD and replication timer fields as `u32`.

`from_descriptor` initializes the tuning and magazine from the descriptor's
`AmmoResource` (0 / absent when `resource: None`), the magazine materializing at
full capacity, the reload timer idle. Surface the augmentable numbers through
`effective()` / `EffectiveStats`: capacity, per-shot cost, ammo type, **and the
reload duration** — producers read the effective values, not raw component
fields, exactly as `credit_source` does today. The augment/progression math (a
reload-speed multiplier over the base duration) is the later progression spec's
concern; `effective()` returns the base today, same as capacity/cost do.
Represent "no ammo resource" as an `Option` on the effective stats so a
resourceless weapon skips gating.

`refresh_from_descriptor` (`weapon.rs:74`) overwrites the stored tuning
(type/capacity/cost/reload-duration) from the descriptor but must retain the live
`magazine` count and any in-flight reload timer — per-instance state like
`cooldown_remaining_ms` (`weapon.rs:35`, preserved at `:83-88`), not authored
tuning. Adding the tuning `Option`, the `magazine` field, and the reload-timer
fields to `WeaponComponent`, and the mirrored fields to `EffectiveStats`,
requires updating their struct literals across the workspace — not only in
`postretro-entities` but also postretro-crate literals such as the
`WeaponComponent` built in `netcode/state_slots.rs` test code; the workspace build
surfaces each one.

### Task 3: Pawn reserve pool + spawn seeding

Add an engine-owned reserve component (`AmmoReserve`, entities crate) pooling
rounds by ammo type (`type → u32`). It references no `EntityId`. As a new
registry component read per-owner (like `HealthComponent` / `WeaponComponent`), it
extends the closed component vocabulary: add an `AmmoReserve` arm to
`ComponentKind` (`crates/entities/src/registry.rs:94`), a `ComponentValue` variant
(enum decl `registry.rs:176-197`, `Weapon` at `:189`, plus its `kind()` match arm
~`:208`), and a `Component`/`KIND` impl mirroring `Weapon` (`registry.rs:398`) —
the compiler's exhaustive-match errors enumerate the remaining sites. The pawn and
its active wieldable are separate entities: inside `attach_descriptor_components`
(`crates/postretro/src/scripting/builtins/data_archetype.rs:365`) only the
single spawned entity's `id` is in scope, and for the weapon spawn that `id` is
the weapon entity, not the pawn — a pawn-owned `AmmoReserve` cannot be seeded
there. Seed where pawn `id`, weapon `id`, and the weapon descriptor all coexist:
in `spawn_from_player_starts` (`data_archetype.rs:654`), at the default-weapon
spawn that resolves `weapon_id` (`:744`) — attach/credit the pawn's
`AmmoReserve` for the weapon's ammo type by the descriptor's starting `reserve`,
and let the weapon materialize a full magazine (Task 2). The net-slot path seeds
at the analogous site in `net_descriptor.rs`'s `spawn_net_slot_pawn`
(defined at `net_descriptor.rs:39`; the three values coexist mid-body ~`:90-107`),
for the host-authoritative remote pawn. A net-slot pawn needs its reserve seeded
host-side so its owner has a pool to draw from when reload reaches it — the
reload transfer host-side is now built here (Task 5, via the shipped
`deliver_reload_to_weapon` seam). The net path's only distinct concern remains
spawning the sibling weapon instance but never promoting it to the host's active
wieldable.

`AmmoReserve` exposes a small interface with its backing map private: a query
`available(type) -> u32` (rounds on hand for a type) and an atomic consume
`take(type, n) -> u32` (removes up to `n`, returns the amount actually removed).
All reserve access — the reload transfer (Task 5) and the HUD publisher (Task 6)
— goes through this interface, never direct map indexing. That makes the later
switching+inventory relocation and any inventory-backed storage a single-seam
change: swap the backing store, callers unchanged.

**Seeding rationale (the judgment call).** World pickups are out of scope, so a
test map has no other ammo source. Descriptor-seeded starting reserve at spawn is
grounded in the one path that already reads the weapon descriptor *and* knows the
pawn; it needs no new entity kind and no dev-only grant. A dev grant primitive is
rejected — it would foreshadow the resource-grant chokepoint this spec keeps out
of scope.

### Task 4: Fire-tick consumption + empty-magazine block

In `weapon::tick_resolved` (`crates/postretro/src/weapon/mod.rs:309`), after the
cooldown gate passes and the weapon wants to fire, gate in order: (1) if a reload
is in flight (`reload_remaining_ms > 0`), block silently — reload owns the weapon
and is non-cancellable; (2) if the effective stats carry an ammo resource and
`magazine < cost_per_shot`, block and surface it — resolve no shot, consume no
ammo, and return `WeaponFireAuthorization::Empty`. Arm the effective fire interval
for this activation so a held Auto trigger cannot emit every fixed tick. These
gates live where the fire decision is made: the private `apply_weapon_fire_state`
(`weapon/mod.rs:369-396`, called by `tick_resolved`) distinguishes the
empty-magazine rejection from silent cooldown/reload rejections, so
`tick_resolved` sets `dry_fire = true` only for the empty case.

`ActivationOutcome` alone does not reach the caller: it rides inside
`WeaponImpact` (`:231-247`), which requires a `point`/`normal` a dry fire has
neither, and `WeaponFireEvents` (`activate`/`impact`, reported by `event_names()`,
`:250-266`) has no field for it. Add a `dry_fire: bool` field to
`WeaponFireEvents` that `event_names()` reports as the event name `"dry_fire"`,
so the empty-magazine block reaches the caller's event drain and audio/HUD can
react. (Chosen over broadening to `outcome: Option<ActivationOutcome>`: a bool is
the minimal carrier for the one dry-fire case, and `WeaponImpact` already carries
`outcome` for the shot-resolved cases.)

Otherwise decrement `magazine` by `cost_per_shot` (mirror how the cooldown is set
on fire) and resolve exactly as today. A fired round is spent on a miss or
overkill. A resourceless weapon skips the gate entirely. Reserve is not touched
here.

### Task 5: Reload as a timed atomic transfer

`SimCommand.reload` (`sim/mod.rs:36`) is a **held level bit** — `build_sim_command`
samples it with `snapshot.button(Action::Reload).is_active()` (`main.rs:790`), and
`held_gap_sim_command` documents it as "a level bit the ammo spec dedups at
consumption on its rising edge" (`command_queue.rs:462`). So a held R stays true
across ticks; this spec derives the **rising edge** from a new per-instance
`reload_press_consumed` flag on `WeaponComponent`, mirroring the existing
`shoot_press_consumed` field (`weapon.rs:37`) — set on start, cleared when the bit
goes false — so a held R starts exactly one reload. There is no per-pawn
resolved-command store at the local-pawn consumption site
(`ClientCommandState.last_resolved`, `command_queue.rs:152`, is per-client and
remote-only); the weapon-component flag is the reachable home. `reload_press_consumed`
needs the same treatment Task 2 gives `magazine` and the reload timer — a
`from_descriptor` init, the workspace struct-literal update, and
`refresh_from_descriptor` preservation (as `shoot_press_consumed` already has) —
so it survives hot reload; otherwise a freed edge under a held R re-triggers a
second reload.

The shipped `deliver_reload_to_weapon` seam (`sim/mod.rs:424`) owns reload timing
and ammo transfer for every pawn against that pawn's `AmmoReserve`. It is called
each tick for the local/host pawn (`sim/mod.rs:200`) and remote co-op pawns
(`run_remote_weapon_commands`, `sim/mod.rs:382`). Its contract requires mutable
registry access and the fixed-tick delta so a released button still advances and
completes an in-flight reload. All logic runs inside this one seam each tick:

- **On a fresh rising edge with an ammo resource — Guards (start-time):** a fresh
  edge while already reloading is a silent no-op (no `ReloadDelivery`) — the
  rising-edge dedup. A full magazine (`magazine == capacity`) or an empty reserve
  (`available(type) == 0`) is a distinct blocked outcome carried on
  `ReloadDelivery` (`blocked-full` / `blocked-empty`); no timer starts and no
  rounds move.
- **On that same fresh edge — Start:** set
  `reload_total_ms = reload_remaining_ms = effective().reload_ms` and clear the
  fractional elapsed carry.
- **Every tick while `reload_remaining_ms > 0`, regardless of the reload bit —
  Advance:** accumulate the fixed-tick delta in milliseconds, decrement by whole
  elapsed milliseconds, and carry the fraction into the next tick. A released
  button must still complete a non-cancellable reload. Do this here, *not* on the
  cooldown-decrement pass: `cooldown_remaining_ms` is decremented in the private
  `apply_weapon_fire_state` (`weapon/mod.rs:376`), which is weapon-only with no
  `AmmoReserve` in scope; the reload timer must live where the pawn reserve is
  reachable — this seam.
- **The tick `reload_remaining_ms` reaches 0 — Complete:** perform one atomic
  `take(type, min(capacity - magazine, available(type)))` and add the returned
  rounds to the magazine. `take` is the atomic step; never index the pool
  directly. Evaluate the `min`/`take` at completion against the live reserve.
  A `Started` outcome owns the rest of its start tick: fire stays blocked that
  tick even when a short reload also reaches `Completed` immediately.

Reload does not interrupt cooldown, does not cancel on fire (the fire gate blocks
the trigger while reloading), and is not a per-shell state machine (out of scope).
Reload skips the `weapon_fire_command` → `WeaponFireCommand` hop entirely — that
command is aim/fire only; reload rides `SimCommand.reload` and `ReloadDelivery`.

### Task 6: HUD + reload-meter slots, publisher, readout, docs, tests

**Catalog.** Add `player.ammo` and `player.ammoReserve` to `BUILTIN_ENGINE_STATE`
(`crates/entities/src/engine_state_catalog.rs`): readonly Number,
`OwnerPrivatePlayer` network scope, matching **`player.weaponCooldownMs`**
(`:395`) — the existing `OwnerPrivatePlayer`, weapon-derived, per-owner precedent,
a closer analog than `player.health`. Flip the already-shipped
`player.reloadProgress` (`:385`) and `player.reloadActive` (`:375`) from
`ReplicationScope::None` to `OwnerPrivatePlayer` (the reload-feedback plan
deferred this to the real producer). Adding two slots and re-scoping two breaks
two tests in the catalog's test module — update both:
`built_in_catalog_preserves_wire_names_and_capabilities` must assert the full
sorted wire-name vector includes `player.ammo` and `player.ammoReserve`, and that
both reload slots use `OwnerPrivatePlayer`. The
**`player_owner_private_slots_are_replicated`** test must assert the exact
owner-private set: `player.ammo`, `player.ammoReserve`, `player.health`,
`player.maxHealth`, `player.reloadActive`, `player.reloadProgress`, and
`player.weaponCooldownMs`; every other built-in slot remains `None`. The
`netcode/state_slots.rs` tests must derive the same contract from
`SlotTable::new()`: replicated names stay sorted, the default schema contains
exactly those owner-private player slots, and the net schema carries matching
descriptors and fingerprint. No version bump is required: the replicated
state-slot fingerprint (`compute_fingerprint`, `state_slots.rs:197`) is
content-derived — it hashes each replicated entry's name/type/range/scope, so
adding and re-scoping slots changes it automatically and both peers recompute it
identically. `FINGERPRINT_STREAM_VERSION` (`state_slots.rs:23`) bumps only on a
stream-*shape* change, which this is not; leave it and `WIRE_VERSION` untouched.

**Ammo publisher.** Extend/mirror `PlayerHudStatePublisher`
(`crates/postretro/src/scripting/systems/ui_proxy.rs:24`) to read the active
wieldable's live magazine → `player.ammo` and the pawn reserve's
`available(type)` for that weapon's ammo type → `player.ammoReserve`; the
publisher needs the active-wieldable id, which the `main.rs` tick site
(`self.active_wieldable`, `:529`, near the `player_hud_state.tick_for_role` call
at `:2081`) supplies — thread it into `tick_for_role`, which does not receive it
today. With `resource: None` or no active wieldable, skip the ammo write, matching
the publisher's no-pawn handling (`ui_proxy.rs` no-pawn test `:299`).

**Reload producer.** Publish `player.reloadProgress` (=
`1 - reload_remaining_ms / reload_total_ms`, `0` when idle) and
`player.reloadActive` (`reload_remaining_ms > 0`) each frame from the active
wieldable's reload timer. Unlike the ammo slots, this producer **always** writes
`reloadActive = false` / `reloadProgress = 0` when there is no active reload (or
no pawn / `resource: None`) so the meter's retained `exitFade` fires on the
active→inactive transition rather than latching the last value. This may extend
`PlayerHudStatePublisher` or be a sibling system on `session.scripting`; either
way it writes through the same engine-owned readonly-slot path.

**Retire the dev driver.** Delete `DevReloadProgressDriver`
(`crates/postretro/src/scripting/systems/reload_progress.rs`), its
`#[cfg(feature = "dev-tools")]` tick (`main.rs:2083-2087`), and its field and
construction sites (`session/mod.rs:227-228` field, `:519-521` construct,
`:535-536` assign; `startup/lifecycle.rs:1317-1318`) — deleting the type alone
leaves those dangling. The real producer replaces it. Leave the `hud.reloadMeter`
tree in
`content/dev/scripts/hud.ts` untouched (already bound to the two slots with
`visibleWhen` + `exitFade`); the reload-feedback plan promised no UI rework.

**Dev HUD + content.** Restore an ammo readout in `content/dev/scripts/hud.ts`
bound to `getGameState().player.ammo` / `player.ammoReserve`. Seed the reference
pistol (`content/dev/scripts/reference-pistol.ts`, `canonicalName
"reference_pistol"`, currently no `resource`) with an `ammo` resource — including
`reloadMs` (≈500 to match the retired dev ramp) — so the fire→empty→reload loop
and the meter are demoable end to end.

**Docs + fixtures + tests.** Update the committed SDK snapshot tests that assert
ammo's absence (`crates/postretro/src/scripting/typedef/tests/committed.rs:261`
`readonly ammo:` / `:273` `ammo: ReadonlyStateRef<number>`) to assert its
presence; because `player.ammo` / `player.ammoReserve` sort before `player.health`
in the generated `player` object, also update the prefix-ordering assertion at
`committed.rs:254` (which pins `readonly player: { readonly health: ...`). Extend
the `## components.weapon` section of `docs/scripting-reference.md` (line 145) with
the resource block (incl. `reloadMs` and the reload-speed-as-stat note). Add the
Rust tests: descriptor parse/validate in both runtimes (incl. `reloadMs` bounds
and the two-layer serde/`validate()` rejection), SDK type generation, magazine +
reload-timer seed and hot-reload preservation, reserve seed, fire consumption,
empty-magazine block (`dry_fire` + the internal `WeaponFireAuthorization::Empty`),
fire-blocked-while-reloading, timed atomic reload transfer at completion, the
full/empty reload block outcomes, unlimited-fire back-compat, ammo +
reload-progress HUD publish, reload-meter idle write, dev-driver removal (grep
gate), and the per-owner projection for ammo and reload.

### Task 7: Per-owner ammo + reload projection (co-op)

Single-player and the host's own pawn get correct values from the Task 6
publisher/producer alone. A co-op remote client does not: `player.ammo` /
`player.ammoReserve` / `player.reloadProgress` / `player.reloadActive` are
`OwnerPrivatePlayer` slots, but without a per-pawn source they fall back to the
slot table's single global value (`owner_private_source_value`,
`crates/postretro/src/netcode/state_slots.rs:448`) — every owner would see the
host's values. Add per-pawn projections in `owner_private_source_value` alongside
the two existing projections:

- **`player.ammo`, `player.reloadProgress`, `player.reloadActive`** live on the
  pawn's **weapon** entity (magazine, reload timer). Mirror
  **`descriptor_weapon_cooldown_for_pawn`** (defined at `state_slots.rs:498`,
  dispatched from `owner_private_source_value` at `:458`), which reaches
  the pawn's active weapon through the `WeaponOwners` map — *not*
  `descriptor_health_for_pawn`, which is pawn-local and cannot see the weapon's
  magazine or timer. Note `player.reloadActive` is a **Boolean**: the projection
  must yield a Boolean slot value, not only the numeric shape the cooldown/health
  projections use.
- **`player.ammoReserve`** reads the pawn-local `AmmoReserve`, but is keyed by the
  active weapon's ammo type — so it is a hybrid, not a pure mirror: resolve the
  pawn's weapon via `WeaponOwners` (as `descriptor_weapon_cooldown_for_pawn` does)
  to read the effective ammo type, *then* read that pawn-resident
  `AmmoReserve.available(type)` (the pawn-local read `descriptor_health_for_pawn`,
  `state_slots.rs:471`, models).

This is ammo's/reload's own projection over state already seeded by Task 3 and
timed by Task 5. It is not client-side prediction of reload (out of scope) — it
surfaces the host-authoritative per-pawn value to each owner.

## Sequencing

**Phase 1 (sequential):** Task 1 → Task 2 → Task 3 — descriptor union, component
magazine/timer/effective, and the reserve component + spawn seed all edit shared
struct-literal call sites and the spawn path, so they run in sequence.
**Phase 2 (sequential):** Task 4 — the fire chokepoint (magazine consume,
empty-magazine and reloading blocks).
**Phase 3 (sequential):** Task 5 — the timed reload transfer over the reserve and
the `deliver_reload_to_weapon` seam.
**Phase 4 (sequential):** Task 6 — the HUD + reload-meter slots, publisher/producer,
dev-driver retirement, docs, and tests over the completed magazine/reserve/timer
surface.
**Phase 5 (sequential):** Task 7 — the co-op per-pawn ammo + reload projections
over the state seeded by Task 3 and timed by Task 5.

## Rough sketch

Grounded identifiers: `WeaponDescriptor` and its `validate()` in
`crates/foundation/src/data_descriptors/types/combat.rs:28,40` (re-exported as
`postretro_foundation::WeaponDescriptor`); `WeaponComponent` / `EffectiveStats`
/ `effective()` (`:63`) / `from_descriptor` (`:43`) /
`from_descriptor_with_canonical` (`:47`) / `refresh_from_descriptor` (`:74`) /
`cooldown_remaining_ms` (`:35`) in `crates/entities/src/components/weapon.rs`;
`ActivationOutcome::{Hit, Effect, Spawned}` (`:30-34`), `WeaponImpact` (`:231-247`,
carries `point`/`normal`/`outcome`), `WeaponFireCommand` (`:50`),
`WeaponFireEvents` + `event_names()` (`:250-266`), and `tick_resolved` (`:309`) in
`crates/postretro/src/weapon/mod.rs`; descriptor parsers
`entity_descriptor_from_js` / `entity_descriptor_from_lua` in
`crates/scripting-core/src/data_descriptors/{js,lua}/entity.rs`; SDK registry
(`register_type`, `register_tagged_union` — `ComponentValue` at `primitives/mod.rs:52`,
`WeaponDescriptor` fields at `:250`) in
`crates/postretro/src/scripting/primitives/mod.rs`; equip-at-spawn in
`crates/postretro/src/scripting/builtins/data_archetype.rs`
(`spawn_from_player_starts:654`, `weapon_id` at `:744`,
`attach_descriptor_components:365`) and `.../net_descriptor.rs`
(`spawn_net_slot_pawn:39`); the fire command build in
`crates/postretro/src/main.rs` (`build_sim_command`, `reload.is_active()` at
`:790`); the reload seam — `SimCommand.reload` (`sim/mod.rs:36`, held level bit),
`deliver_reload_to_weapon` (`:424`), `ReloadDelivery` (`:70`),
`TickEvents.reload_deliveries` (`:82`), own-pawn (`:200`) and remote
(`run_remote_weapon_commands:382`) routing, and `run_weapon_fire_tick` /
`local_movement_pawn` in `crates/postretro/src/sim/mod.rs`; the production host
resolver `host_resolve_remote_commands` (`command_queue.rs:396`),
`neutral_sim_command` (`:434`), and `wire_convert.rs`
`input_command_to_sim`/`sim_command_to_input`; `WIRE_VERSION`
(`crates/net/src/transport.rs:52`) — reload already ships on `InputCommand`, so
no wire field is added and no bump happens here; the HUD catalog in
`crates/entities/src/engine_state_catalog.rs` (`BUILTIN_ENGINE_STATE`;
`player.maxHealth:361`, `player.reloadActive:375`, `player.reloadProgress:385`,
`player.weaponCooldownMs:395`; tests `built_in_catalog_preserves_wire_names_and_capabilities`,
`player_owner_private_slots_are_replicated:711`); the HUD publisher
`PlayerHudStatePublisher` (`ui_proxy.rs:24`) and its tick site near `main.rs`
`player_hud_state.tick_for_role` (`:2081`, `self.active_wieldable:529`); the dev
reload driver `DevReloadProgressDriver`
(`crates/postretro/src/scripting/systems/reload_progress.rs`, `dev-tools`-gated at
`main.rs:2083`), retired here; the dev HUD `content/dev/scripts/hud.ts`
(`hud.reloadMeter` tree) and `content/dev/scripts/reference-pistol.ts`; the
per-owner replication seam `owner_private_source_value` (`state_slots.rs:448`),
`descriptor_weapon_cooldown_for_pawn` (`:458`, via `WeaponOwners`), and
`descriptor_health_for_pawn` (`:471`).

Proposed shape:

```rust
// Proposed design (foundation crate — no EntityId, stays in postretro-foundation).
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WeaponResource {
    Ammo(AmmoResource),   // first variant; `heat` / `cell` are sibling variants later
}

#[serde(rename_all = "camelCase")]
pub struct AmmoResource {
    #[serde(rename = "type")]
    pub ammo_type: String,   // reserve-pool key; validated as the credit_source charset
    pub magazine: u32,       // full-magazine capacity (an augmentable stat via effective())
    #[serde(default = "default_cost_per_shot")]
    pub cost_per_shot: u32,  // rounds per activation (augmentable via effective())
    pub reserve: u32,        // starting reserve pooled by ammo type, seeded at spawn
    #[serde(default = "default_reload_ms")]
    pub reload_ms: u32,      // base reload duration; augmentable "reload speed" via effective()
}
```

`WeaponComponent` gains a stored ammo tuning `Option<{ ammo_type, capacity,
cost_per_shot, reload_ms }>` (authored, refreshed on hot reload) plus live
`magazine: u32` and the reload timer `reload_remaining_ms` / `reload_total_ms`
(per-instance state, preserved on hot reload). `EffectiveStats` gains the ammo
capacity/cost/type/reload-duration behind the `effective()` accessor — an
`Option`, absent = unlimited-fire. The reserve is a separate pawn-owned component:

```rust
// Proposed design (entities crate — pawn-owned inventory precursor).
pub struct AmmoReserve {
    pools: HashMap<String, u32>,  // private; ammo type -> rounds
}

impl AmmoReserve {
    // The relocation seam: an inventory-backed store implements these two
    // unchanged, so switching+inventory swaps the backing store, callers as-is.
    pub fn available(&self, ammo_type: &str) -> u32 { /* rounds on hand */ }
    pub fn take(&mut self, ammo_type: &str, n: u32) -> u32 { /* remove ≤ n, return taken */ }
}
```

A reload press starts the timer from `effective().reload_ms`; the timer advances
per tick; at completion it atomically `take`s
`min(capacity - magazine, available)` into the magazine. Firing decrements
`magazine` by the effective `cost_per_shot`; an empty magazine (or an in-flight
reload) blocks the trigger — the empty case yields `WeaponFireAuthorization::Empty`
and a rate-limited `dry_fire` event, never a silent no-op. The reload timer drives
`player.reloadProgress` / `player.reloadActive`.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
| --- | --- | --- | --- | --- | --- |
| Resource discriminant | `WeaponResource` tag | `"kind"` (`"ammo"`) | `components.weapon.resource.kind` | same | n/a |
| Ammo type | `AmmoResource::ammo_type`, `EffectiveStats` ammo type | `"type"` | `resource.type` | same | n/a |
| Magazine capacity | `AmmoResource::magazine`, `EffectiveStats` capacity | `"magazine"` | `resource.magazine` | same | n/a |
| Cost per shot | `AmmoResource::cost_per_shot`, `EffectiveStats` cost | `"costPerShot"` | `resource.costPerShot` | same | n/a |
| Starting reserve | `AmmoResource::reserve` | `"reserve"` | `resource.reserve` | same | n/a |
| Reload duration (base) | `AmmoResource::reload_ms`, `EffectiveStats` reload duration | `"reloadMs"` | `resource.reloadMs` | same | n/a |
| Stored ammo tuning (component) | `WeaponComponent` ammo tuning `Option` (`ammo_type`/`capacity`/`cost_per_shot`/`reload_ms`), refreshed by `refresh_from_descriptor` | n/a | n/a | n/a | n/a |
| Live magazine (HUD) | `player.ammo` slot ← `WeaponComponent::magazine` | `player.ammo` | `getGameState().player.ammo` | same | n/a |
| Reserve pool (HUD) | `player.ammoReserve` slot ← `AmmoReserve` | `player.ammoReserve` | `getGameState().player.ammoReserve` | same | n/a |
| Reload progress (meter) | `player.reloadProgress` slot ← reload timer | `player.reloadProgress` | `getGameState().player.reloadProgress` | same | n/a |
| Reload active (meter) | `player.reloadActive` slot ← `reload_remaining_ms > 0` | `player.reloadActive` | `getGameState().player.reloadActive` | same | n/a |
| Dry-fire event | `WeaponFireEvents::dry_fire` → `event_names()` `"dry_fire"` | event-drain name | reaction/audio consumer | same | n/a |
| Reload delivery / block | `ReloadDelivery` outcome (started / blocked-full / blocked-empty / completed) | event-drain record | reaction/audio consumer | same | n/a |

## Design decisions & rationale

- **Reload speed is an effective stat (Borderlands model).** The authored
  `reloadMs` is the *base* reload duration; it rides the same
  `effective()` / `EffectiveStats` seam as magazine capacity and cost-per-shot,
  so an augment/attachment or a scripting-driven progression system modifies it
  by reading the effective value — never the raw field. A modder can author a
  reload speed per weapon; a progression layer can enhance it later. Duration is
  stored in **milliseconds** (consistent with `cooldown_*_ms`), a concrete engine
  primitive; "reload speed" is the player-facing/progression framing (a
  multiplier over the base), left to the progression spec. The progression system
  is out of scope; only the seat on the effective seam is built here, so the shape
  is additive, not hard to reverse.
- **Timed, non-cancellable, single-transfer reload — not per-shell.** A duration
  is what the shipped reload meter needs to fill over. This is distinct from
  per-shell / incremental / cancellable reload and the `reloadStyle` classifier,
  which stay deferred (they imply a shell-loop state machine this spec does not
  build). The transfer is still atomic — one `take` at completion — just gated by
  a timer.
- **The reload meter is wired for real, retiring the dev driver.** The shipped
  reload-feedback plan built `player.reloadProgress` / `player.reloadActive`, a
  dev-only `DevReloadProgressDriver`, and the `hud.reloadMeter` bar, and stated
  the real-producer spec would "point a real reload state machine at the
  already-defined slot and drop the dev driver — no UI rework." This spec is that
  producer: the reload timer drives both slots, the dev driver is deleted, and the
  bar is untouched. The producer writes the idle `(false, 0)` snapshot every quiet
  frame (unlike the ammo publisher's skip-on-no-pawn) so the retained `exitFade`
  fires — gameplay publishes the lifecycle snapshot and never drives the fade
  timer (`ui.md` §3).
- **Reload slots become owner-private and projected.** The reload-feedback plan
  shipped `player.reloadProgress` / `player.reloadActive` as
  `ReplicationScope::None` and deferred the per-owner projection to the real
  producer, "as `player.weaponCooldownMs` / `player.health` did." This spec flips
  them to `OwnerPrivatePlayer` and adds the per-pawn projection (Task 7), so a
  co-op remote client sees its own reload, not the host's. Client-side prediction
  of the reload timer is a separate follow-up.
- **`player.ammo` is added fresh, not retired from a static proxy.** The Epic 13
  game-state SDK cleanup deleted the fake ammo HUD entirely — no live
  `player.ammo` slot survives, and the committed SDK snapshot + demo tests assert
  its absence. This spec *introduces* the engine-owned `player.ammo` /
  `player.ammoReserve` slots, a publisher, and a restored readout — it does not
  swap a static value.
- **Ammo `type` stays a free-form charset-validated identifier.** It matches the
  shipped `creditSource` precedent — same `[A-Za-z0-9_.:-]` charset, same
  modder-owned-key role; two sibling contracts in one milestone should not
  disagree on how a modder names a category. A generated `AmmoType` union belongs
  to a future cross-cutting "declare-your-categoricals → codegen" spec spanning
  ammo type, credit source, damage type, and status effects. String→union is a
  compatible tightening, so deferral is safe.
- **Reserve lives on the pawn as an inventory precursor.** Pooling by ammo type
  is honored; the durable home is the inventory (`weapon-model.md` §6), out of
  scope here. The `available`/`take` interface (Task 3) makes pawn ownership safe
  — it localizes the later switching+inventory relocation to one seam and keeps
  the immersive-sim inventory-backed-storage use case open. Nothing downstream
  indexes the pool directly.
- **The reserve keeps two use cases open.** The `u32` count is a near-term
  stand-in for inventory-backed storage — an inventory tracking ammo as
  space-occupying items backs the same `available`/`take` interface without
  touching callers. "Takes up space" (weight/volume) is a write-side concern
  enforced when ammo *enters* the reserve — the deferred grant/pickup chokepoint —
  not here. Borderlands-style pooled-by-type ammo is directly expressible today
  via the `type` string plus `costPerShot`; a per-type carry cap is a future
  additive field.
- **One resource kind per weapon.** The single `WeaponResource` tagged union
  means a weapon that consumes ammo *and* builds heat (or ammo + cell)
  simultaneously is not expressible — the one genuinely hard-to-reverse bet. It
  traces to the `weapon-model.md` "resource is one-of" decision, not this spec,
  and is orthogonal to the pooled-ammo and inventory use cases.
