# E16 — Weapon State Machine

> **Status:** draft.
> **Epic:** 16 — Combat. **Milestone:** Weapon Systems.
> **Roadmap item:** `per-shell reload` (`context/plans/roadmap.md`, Epic 16 →
> Weapon Systems). The milestone's items are explicitly not a strict chain, so
> `heat + cell resources` is **not** a prerequisite.
> **Builds on (shipped):** `E16--ammo-resource` (magazine, pawn-owned
> `AmmoReserve`, atomic timed reload, the reload meter slots) and
> `E16--client-authoritative-combat` (the reload level bit, its reliable edge
> lane, the FIRE/HIT authority split).
> **Research notes:** `research.md` (lifecycle diagram, vantage table, file-size
> audit, grounding).

## Goal

Replace the implicit "weapon state is whichever timer is nonzero" scheme with an
explicit state machine on the weapon instance, and validate it against two
deliberately different consumers: an interruptible, cancellable **per-shell
reload** loop (the shotgun case) alongside the existing atomic magazine reload,
and an uninterruptible timed **raise / lower** equip transition. Today reload and
fire are two producers coupled by a one-way boolean, which can express "reload
blocks fire" but structurally cannot express "fire cancels reload"; fusing them
into one machine is what per-shell needs, and what the later switching spec will
consume as its lockout.

## Scope

### In scope

- A `WeaponState` enum on `WeaponComponent` with six states — `Idle`,
  `Reloading`, `ShellLoading`, `Raising`, `Lowering`, `Stowed` — plus one
  generalized timed-state triple (remaining / total / sub-ms carry) replacing the
  three reload-specific timer fields.
- Fusing `sim/reload.rs::tick` and `weapon/mod.rs::apply_weapon_fire_state` into a
  single machine tick that both the local-pawn and host-simulated-remote-pawn
  paths call, so the two vantages cannot diverge.
- A `reloadStyle` classifier on `AmmoResource`: `"magazine"` (default, today's
  atomic behavior) and `"perShell"`. Rust enum, serde, generated SDK typedefs,
  and `docs/scripting-reference.md`.
- Per-shell reload: `reloadMs` becomes the duration of **one reload step**; the
  loop credits one round per step from the pawn `AmmoReserve` until the magazine
  is full or the reserve is empty. Rounds already credited survive cancellation
  and stowing.
- Cancellation: an otherwise-authorized shot cancels a `ShellLoading` loop,
  forfeits only the in-flight step, and fires on that same tick.
- `raiseMs` / `lowerMs` on `WeaponDescriptor`, both defaulting to `0`. Equip-at-spawn
  drives `begin_raise` at both spawn sites (local player start and net-slot pawn).
  `begin_lower` ships as a defined seam with full transition legality and no
  production caller — the switching spec is its driver.
- Zero-duration timed states collapse at entry: no tick is consumed and the fire
  gate never observes them, so the `raiseMs = 0` default is behavior-identical to
  today's instant equip.
- A hot-reload policy for the state field and its timers in
  `refresh_from_descriptor`, including a mid-loop `reloadStyle` flip.
- Script events: `reload_shell_loaded`, `reload_cancelled`, `weapon_raise_started`,
  `weapon_raised`, `weapon_lower_started`, `weapon_lowered`, beside the shipped
  `reload_started` / `reload_completed` / `reload_blocked_full` /
  `reload_blocked_empty` / `dry_fire`.
- Redefining `player.reloadProgress` as the **current reload step's** progress and
  `player.reloadActive` as true across the whole reload (both styles), so the
  shipped meter and the owner-private projection work unchanged.
- A behavior-preserving extraction of the weapon stage out of `sim/mod.rs`
  (1446 production lines) before it is extended.
- Dev content: a `reference_shotgun` archetype (per-shell, non-zero raise/lower)
  wired as the dev player's `defaultWeapon`; an explicit `reloadStyle: "magazine"`
  on the reference pistol.
- Tests across all four vantages named in `research.md` §3.

### Out of scope

- **Weapon switching, inventory, and pickups.** This spec defines the states and
  transition legality switching consumes; it builds no switching, no inventory,
  no `active_wieldable` repointing. `begin_lower` has no production caller here.
- **Multi-pellet firing and spread.** `pellet_count` stays `1` at both
  construction sites. See *Alternatives rejected*.
- Weapon animation. There is no animation system for weapons — no recoil, no
  fire or reload viewmodel clips; the viewmodel's only motion is the borrowed pawn
  `movement.viewFeel` bob/tilt/sway applied in `viewmodel_world_transform`.
  Raise/lower durations are a fire lockout plus event edges, not a clip.
- `Cooling` as a state. Cooldown stays an orthogonal rate limiter that composes
  with `Idle` — a reload may start while cooling, as it does today.
- A rack/finish stage after a cancelled per-shell reload. See *Alternatives rejected*.
- Loading more than one round per shell step (`shellsPerStep`), and the
  `"internal"` / `"energy"` reload styles.
- A `player.weaponState` HUD slot or any new engine-state slot. No new replicated
  slot means no state-slot fingerprint change.
- Client-side prediction of reload, per-shell steps, or raise/lower. V3 keeps
  predicting cooldown only and reconciles through the shipped `ShotVerdict` path.
- `heat` / `cell` resource variants, augments, charge, secondary activation.

## Direction

**Problem.** Reload and fire are two producers of weapon state that cannot see
each other's decision. `sim/reload.rs::tick` runs first and hands
`apply_weapon_fire_state` a single boolean, `reload_started_this_tick`; the fire
gate can consult reload but reload can never consult fire. That one-way coupling
is exactly expressive enough for the shipped atomic reload and structurally
incapable of expressing the per-shell interrupt, so per-shell is not a feature
that can be added to the current shape — it is a feature that requires changing
it. The observation that produced this: every candidate implementation of "fire
cancels reload" against today's code has to smuggle the decision back upstream
through a second boolean, and the second boolean is the point at which "state is
whichever timer is nonzero" stops being a shorthand and starts being a bug
surface.

**Placement.** The machine belongs in the shared private callee both simulated
vantages already funnel through, not in either caller. `run_local_weapon_command`
and `run_remote_weapon_commands` both call `reload::tick` and then a
`weapon::tick_*_component`, and both of those delegate the entire gate decision to
the same private `apply_weapon_fire_state`. Placing the machine there serves the
local pawn and the host-simulated remote pawn with one implementation; placing it
in `tick_resolved_component` — the tempting spot, since it is where firing visibly
happens — would silently skip the remote-pawn vantage, which uses
`tick_state_only_component` instead. The axis here is *shared-callee vs. per-caller*,
not engine-vs-mod: the descriptor surface (`reloadStyle`, `raiseMs`, `lowerMs`) is
authored data and the machine that reads it is engine floor, matching how
`fireMode` and `resolution` already split.

**Prior commitments.**
- `weapon-model.md` §3 already names per-shell reload "a cancellable state machine"
  and puts `reloadStyle` on the resource, not on the weapon — honored. It spells
  the value `"per-shell"`; the repo's `#[serde(rename_all = "camelCase")]` enum
  convention (`FireMode::Semi` → `"semi"`) makes that `"perShell"`. Stated
  divergence, argued: two sibling classifiers in one milestone should not disagree
  on casing.
- `E16--ammo-resource` shipped reload as "timed, non-cancellable, single-transfer"
  and explicitly deferred "per-shell / incremental / cancellable reload and the
  `reloadStyle` classifier" to this spec. The atomic style keeps that contract
  exactly; per-shell is a sibling, not a replacement.
- That spec also made reload duration an **effective stat** read through
  `effective()`, never the raw field. Honored, and generalized: `reloadMs` is now
  the duration of one reload step, so a reload-speed modifier scales per-shell
  cadence for free rather than needing a second augmentable number.
- `networking.md` §Combat authority: fire-rate and ammo stay host-authoritative,
  client-side ammo and reload prediction stay out of scope. Honored — V3 is
  untouched.
- The reliable reload edge lane exists because rising edges can be destroyed by
  stale-drop or catch-up trimming. No new edge is introduced, so the lane is not
  extended (`research.md` §6).
- Roadmap: `switching + inventory` "replaces the `active_wieldable` chokepoint."
  This spec deliberately does not touch that chokepoint.
- `plans/done/reload-feedback-ui/` already promised that a later timed-reload spec
  would "point a real reload state machine at the already-defined slot… no UI
  rework." Task 6's re-meaning of `player.reloadProgress` is that promise being
  kept, not a new liberty taken with a shipped scripting slot.
- `plans/done/movement--state-machine/` is the structural precedent this spec
  conforms to: extract the substrate intact, then introduce the state enum with a
  baseline state that is behavior-identical and gated by the existing regression
  suite, then add states. Same shape here, Tasks 1 and 2.
- **Divergence from `plans/done/E10--behavior-state-graph/`, stated.** That epic
  replaced an engine-closed enemy FSM with an *authored* behavior graph. This spec
  builds an engine-closed enum with hardcoded transitions — the shape E10 retired
  for enemies. The divergence is deliberate: `weapon-model.md` §3 commits
  `reloadStyle` to a fixed classifier ("a trait, not a number"), and
  `movement--state-machine` settled the same question the other way for a hot-path
  engine-internal system. Weapons take the movement answer, not the enemy one,
  because the transitions here are timing and resource legality rather than
  authored behavior. Reversible at the cost E10 itself paid: keep the enum, lower
  it to a graph later.

**What this forecloses.** Four things, named deliberately. (1) Making cooldown a
state later would be a behavior change, not an addition — a reload can start while
cooling today and the machine keeps that true, so a future `Cooling` state would
have to break it. (2) One state at a time: a weapon cannot be simultaneously
lowering and reloading. Dual-wield (roadmap, later) generalizes the *active
reference* to a pair, and each instance keeps its own machine, so the pair case is
unaffected — but a single instance with two concurrent activities is now
unrepresentable. (3) `reloadMs` stops being readable on its own: it means "the whole
reload" or "one shell" depending on the sibling `reloadStyle`, so every consumer —
HUD, docs, a future augment tooltip, a modder — must read both. Accepted over a
separate `shellReloadMs` so one reload-speed modifier scales both styles, but it is
a real loss of local readability. (4) The weapon state vocabulary is engine-closed;
see the E10 divergence above for what reopening it would cost.

**What this hands to the switching spec.** `ClientWeaponState` owns no
`WeaponComponent` and models cooldown, fire mode, and range only, so a connected
client cannot see any state this spec adds. That is acceptable here — a shot
predicted during a host-side raise or reload rolls back through the shipped
`ShotVerdict` path, and reloads are rare and player-initiated. It will not be
acceptable for switching, where a raise lockout fires on every swap and a per-swap
mispredict-and-rollback would be visible. The switching spec inherits that choice:
accept the per-swap rollback, or teach the client's prediction state about equip
transitions.

**One-way doors.** The `reloadStyle` **discriminant** is the one hard-to-reverse
piece — same class of bet as the `WeaponResource` tag, and it sits on the same
authored surface, so changing its spelling or shape after content exists costs a
content migration. Everything else reverses cheaply: adding a state is additive,
`raiseMs` / `lowerMs` default to `0` (removing them restores today's behavior
exactly), and the timer generalization is internal to two crates. `begin_lower`
having no caller is reversible in either direction at the cost of one call site.

**Alternatives rejected.**

- **Multi-pellet shotgun spread, in this spec.** The strongest case for it is that
  "shotgun" without pellets is a thin demo. Rejected: per-shell reload is a
  resource/state mechanic that requires no change to shot resolution, and the
  roadmap places spread and resolution shapes in the separate *Resolution Modes*
  milestone. Concretely, `pellet_count` is already consumed generically — hit
  acceptance clamps with `.take(pellet_count)` and rejects `0` — so raising it is
  additive later. Shipping it here would drag a second, unrelated authority
  question (how many hit records a client may declare per shot, and whether spread
  must be deterministic across the prediction seam) into a state-machine spec, and
  a spec answering two authority questions gets neither reviewed properly. The dev
  shotgun is authored as a single-projectile weapon whose distinguishing trait is
  its reload.
- **A rack / finish stage after cancellation.** Classic shotguns play a close-bolt
  animation on cancel, and modelling it would give cancellation a real cost. Rejected
  for v1: it is a third uninterruptible-timed-stage shape, and raise/lower already
  validates that shape. It is purely additive later (one state, one duration field),
  and with no weapon animation system it would be an invisible delay today.
- **Drop `Raising` / `Lowering` / `Stowed` entirely; ship `Idle` / `Reloading` /
  `ShellLoading` and let the switching spec add the equip states when it has a
  driver.** The strongest rival, and it deletes Task 5, part of Task 7, and the
  `WeaponDescriptor` struct-literal churn. Its case is sharp and worth stating
  plainly: raise/lower is a **self-authored consumer**, not independent validation —
  `Lowering` and `Stowed` have no production caller, and `Raising`'s only caller
  passes `0` for every weapon except a dev shotgun this same spec writes. Rejected
  anyway on two grounds. An uninterruptible timed state is a genuinely different
  shape from an interruptible loop, and two of the machine's rules — zero-duration
  collapse, and "preempt legal from every source state" — are exercised by nothing
  else. And the switching spec's own hard problem is converging the divergent
  active-weapon holders; re-opening transition legality inside it is worse than
  pinning it now, when the only cost is two enum variants and their transition rows.
- **Zero-duration `Raising` / `Lowering`, given duration later by the switching
  spec.** A weaker version of the above — it keeps the states but never times them.
  Rejected: a state machine whose timed transitions are never timed is an
  unvalidated abstraction. Real durations with a `0` default gets both: the timed
  path is exercised by the dev shotgun and by tests, and every existing weapon is
  bit-identical to today.
- **Keep the two producers, add a second boolean (`fire_wants_cancel`) back into
  `reload::tick`.** The minimal diff, and it works for exactly this one interrupt.
  Rejected because it re-derives the implicit-state scheme one interrupt later:
  switching adds a third boolean, and the ordering constraints between the three
  become unwritable. The fusion is the whole point.
- **A dev-tools-only input binding to drive `begin_lower`.** It would give
  `Lowering` a live driver. Rejected: it adds an `Action` variant and binding-table
  churn for a demo, and the switching spec removes it weeks later.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A weapon is in exactly one state; the fire gate authorizes a shot only from `Idle` | Task 2 | every state added in Tasks 4 and 5 must extend the gate's reject arm, not bypass it | AC 3, 8, 11 |
| Rounds credited by a per-shell loop are never rolled back — not by cancel, not by `begin_lower`, not by hot reload | Task 4 | Task 5's `begin_lower` preempt path; Task 2's hot-reload policy | AC 7, 9, 13 |
| The pawn `AmmoReserve` is debited exactly once per credited round, through `AmmoReserve::take` only | Task 4 | the tick where a step completes and the next step starts in the same tick | AC 6, 7 |
| Hot reload never changes the current state or its remaining time; it refreshes durations and style only | Task 2 | Tasks 4 and 5 adding fields that must join the preserved set | AC 13 |
| A timed state entered with effective duration `0` resolves at entry: no tick elapses, the fire gate never observes it | Task 2 (rule), Task 5 (raise/lower) | any new timed state | AC 10 |
| The local-pawn and host-simulated-remote-pawn vantages run the identical machine | Task 2 | Tasks 4 and 5 adding a path only one caller reaches | AC 14 |
| No wire field, `WIRE_VERSION`, app-protocol, or replicated-slot-fingerprint change | Task 2 | Tasks 4, 5, 6 | AC 16 |

## Acceptance criteria

- [ ] Authors can set `components.weapon.resource.reloadStyle` to `"magazine"` or
  `"perShell"` in TypeScript and Luau; an absent value parses as `"magazine"`; any
  other value is rejected at deserialize, identically in both runtimes. Generated
  SDK typedefs and `docs/scripting-reference.md` carry the field and both values.
- [ ] Authors can set `raiseMs` and `lowerMs` on `components.weapon`; both default
  to `0`, both reject non-integer and negative values at deserialize.
- [ ] A weapon authored with `reloadStyle: "magazine"` (or with the field absent)
  reloads exactly as it does today: one timer of `reloadMs`, one atomic transfer at
  completion, uncancellable by firing, and the same `reload_started` /
  `reload_completed` / `reload_blocked_full` / `reload_blocked_empty` events with
  the same `transferred` count. The shipped atomic-reload tests pass unchanged.
- [ ] A `perShell` reload loads one round per `reloadMs`: after N steps the magazine
  has grown by exactly N and the pawn reserve has shrunk by exactly N, with one
  `reload_shell_loaded` event per step.
- [ ] The loop ends on its own when the magazine reaches capacity, and separately
  when the reserve reaches zero with the magazine still short; both end states emit
  one `reload_completed` whose `transferred` is the cumulative total for the whole
  reload, and leave the weapon in the fire-ready state.
- [ ] A reload press with a full magazine, or with an empty reserve, emits the
  blocked outcome and starts no loop — for both styles.
- [ ] An otherwise-authorized shot fired during a `perShell` loop cancels it: the
  shot resolves on that same tick, the in-flight step's progress is discarded with
  no round credited for it, every previously credited round remains in the magazine,
  the reserve is not refunded, and one `reload_cancelled` event fires. Cancelling at
  the first step (zero rounds credited) and at the last possible step both behave
  this way.
- [ ] A trigger pull during a `perShell` loop that would *not* be authorized anyway —
  cooldown not elapsed, or magazine below `costPerShot` — does not cancel the loop
  and emits no `dry_fire`.
- [ ] `begin_lower` is legal from every state and preempts: from `Reloading` it
  aborts with no transfer; from `ShellLoading` it forfeits only the in-flight step
  and keeps credited rounds; from `Raising` it preempts the raise. From `Lowering`
  the weapon reaches `Stowed`, where fire and reload are both refused, and only
  `begin_raise` leaves it.
- [ ] With `raiseMs = 0` (the default) a weapon is fire-ready on the same tick it is
  equipped — no tick is lost, and the shipped equip-at-spawn tests pass unchanged —
  while still emitting `weapon_raise_started` and `weapon_raised`. With
  `raiseMs > 0` a shot attempted before the raise completes is refused silently and
  the first authorized shot lands on the first tick after completion.
- [ ] Firing and reloading are both refused during `Raising` and `Lowering`, and the
  refusal is silent (no `dry_fire`, no reload block event).
- [ ] Reload can still start while the weapon is cooling, as it can today; cooldown
  never blocks a reload and reload never resets cooldown.
- [ ] Hot-reloading the weapon descriptor mid-`ShellLoading` and mid-`Reloading`
  preserves the state, its remaining time, the sub-millisecond carry, the magazine,
  and the input-edge flags, while adopting the new `reloadMs`, `raiseMs`, `lowerMs`,
  and `reloadStyle` from the next decision point onward. A style flip from
  `perShell` to `magazine` mid-loop lets the in-flight step credit its round and then
  ends the reload rather than looping.
- [ ] A co-op remote client's pawn, simulated host-side, exhibits every behavior
  above — per-shell steps, cancel-on-fire, raise lockout — identically to the
  single-player pawn, driven through the same command path.
- [ ] A connected client that predicts a shot the host refuses because the weapon is
  raising, lowering, stowed, or reloading sees the shot rolled back through the
  existing verdict path: no authorized shot is minted, predicted cooldown is
  restored, and muzzle/hitmarker feedback is cleared.
- [ ] `player.reloadActive` is true for the entire duration of a reload of either
  style, including across per-shell step boundaries, and false at every other time.
  `player.reloadProgress` ramps `0 → 1` once per reload for `magazine` style and once
  per shell for `perShell` style. `player.ammo` increments once per credited shell.
  The owner-private projection publishes the same values to a remote client's owner.
- [ ] No wire message, `WIRE_VERSION`, app-protocol constant, replicated-slot schema
  entry, or state-slot fingerprint input changes (review/grep gate). No weapon
  switching, inventory, pickup, or multi-pellet behavior is built (review/grep gate).
  No new `unsafe` (review/grep gate).
- [ ] The weapon-stage split is behavior-preserving: the whole existing test suite
  passes with no changes beyond import paths and module placement, and no caller
  outside the sim module changes — the netcode hit-acceptance path still reaches the
  authorized-impact entry point at its current path.
- [ ] A weapon ticked with no owning pawn still advances its state timers, resolves
  expiries, and gates fire, but refuses every reload as if the reserve were empty —
  it can fire and cool, and it cannot reload or credit a shell.
- [ ] Launching the dev level equips a per-shell weapon: firing to empty, holding
  reload, watching the magazine climb one round at a time, and firing mid-loop to
  cancel it all work from dev content alone, with both classifier values present
  across the two reference weapon archetypes.

## Tasks

### Task 1: Extract the weapon stage out of `sim/mod.rs`

Behavior-preserving split, no functional change. `crates/postretro/src/sim/mod.rs`
carries 1446 production lines and this plan extends its weapon orchestration, so
split first. Move into a new `crates/postretro/src/sim/weapon_stage.rs`:
`weapon_fire_command`, `normalize_aim_direction`, `run_remote_weapon_commands`,
`run_local_weapon_command`, `apply_weapon_impact_damage`,
`apply_authorized_weapon_impact_damage`, `apply_weapon_impact_damage_with_source`,
and the test-only `deliver_reload_to_weapon`. Keep `run_death_sweep` in `sim/mod.rs`
— it is the death stage, not the weapon stage. Re-export whatever `sim/mod.rs` and
`crates/postretro/src/netcode/` already import at their existing paths so no
caller outside the sim module changes; `apply_authorized_weapon_impact_damage` in
particular is consumed by the netcode hit-acceptance path. Move the weapon-stage
tests along with the code. The tick-order call sites in `run_fixed_tick` stay where
they are and simply call into the new module.

### Task 2: `WeaponState` + the fused machine tick (thin slice)

Narrow vertical slice through every seam, reproducing today's exact behavior with
no new authored surface and no new production states. Add to `WeaponComponent`
(`crates/entities/src/components/weapon.rs`) a `state: WeaponState` field
(`#[serde(default)]`, `Default = Idle`) and replace `reload_remaining_ms`,
`reload_total_ms`, and `reload_elapsed_sub_ms` with one generalized timed-state
triple owned by the machine — remaining ms, total ms, and the fractional carry that
prevents per-tick rounding bias. Define all six `WeaponState` variants now (`Idle`,
`Reloading`, `ShellLoading`, `Raising`, `Lowering`, `Stowed`) so later tasks add
transitions rather than variants, but only `Idle` and `Reloading` are reachable in
production after this task. `from_descriptor` materializes `Idle`.

Fuse the two producers. `sim/reload.rs::tick` and the private
`weapon/mod.rs::apply_weapon_fire_state` become one machine tick with a single
ordered decision per fixed tick: advance the state timer, resolve any expiry, then
evaluate the fire and reload intents against the resulting state. It must sit in the
shared private callee that both `tick_resolved_component` and
`tick_state_only_component` already delegate to, so the local-pawn and remote-pawn
vantages get one implementation; a placement inside `tick_resolved_component` would
skip the remote-pawn path entirely.

Two plumbing facts the fusion turns up, both of which must be settled here rather
than by the implementer. First, a **borrow inversion**: the machine needs
`&mut EntityRegistry` for the `AmmoReserve` transfer, which today only `reload::tick`
holds, while `tick_resolved_component` takes `&EntityRegistry` because hitscan
resolution borrows it immutably for the whole call. Resolve it by splitting the call
into two phases at both call sites — run the machine to completion under the mutable
borrow, producing a fire authorization and an outcome list, then release it and run
hitscan resolution under the immutable borrow using that authorization. Do not widen
the resolution signature to `&mut`; the renderer-adjacent read path should stay a
read. Second, **a weapon with no pawn**: `run_local_weapon_command` takes
`pawn: Option<EntityId>` and today skips reload entirely when it is `None` (fly-camera
and headless harnesses). Define that case explicitly — the machine still advances
timers, resolves expiries, and gates fire, but any transition that would touch the
reserve is refused as if the reserve were empty, so a pawnless weapon can fire and
cool but cannot reload.

The `reload_started_this_tick` boolean and its two
`deliveries.iter().any(...)` computations at the call sites disappear; the machine
returns its own outcome list plus the fire authorization. Keep `ReloadDelivery`,
`ReloadOutcome`, and every existing event name and payload byte-identical.

Define two rules the later tasks depend on. **Zero-duration collapse:** entering a
timed state whose effective duration is `0` resolves it to its successor within the
same call — no tick elapses and no consumer observes the intermediate state, though
its start and end events both fire. **Hot-reload policy:** extend
`refresh_from_descriptor`'s preserved set to include `state` and the timed-state
triple, alongside the cooldown, magazine, and input edges it already preserves;
durations and classifiers refresh and take effect at the next decision point, matching
the existing precedent that reload completion re-reads `effective()`. Rewrite
`reload_status()` to derive `(progress, active)` from the state plus the timers rather
than from `reload_remaining_ms > 0`, keeping today's outputs identical for `Idle` and
`Reloading` including the one-frame `ReloadFeedback` endpoints. Update the
`WeaponComponent` literals across the workspace that the field changes break — the
compiler enumerates them.

### Task 3: `ReloadStyle` classifier on the ammo resource

Add `ReloadStyle { Magazine, PerShell }` to
`crates/foundation/src/data_descriptors/types/combat.rs`, deriving the same trait set
as the sibling `FireMode` / `ResolutionMode` enums and carrying
`#[serde(rename_all = "camelCase")]` so the wire values are `"magazine"` and
`"perShell"`. Add `reload_style: ReloadStyle` to `AmmoResource` with
`#[serde(default, rename = "reloadStyle")]` and a `Default` impl returning `Magazine`,
so every existing authored resource block and every existing struct literal keeps
today's behavior. No new `validate()` rule is needed — serde rejects unknown values —
but state so in the task's test list rather than leaving it implicit. Carry the field
through `WeaponAmmoTuning` and `EffectiveAmmoStats` in
`crates/entities/src/components/weapon.rs` so producers read it through `effective()`,
never off the descriptor, matching how `reload_ms` is already routed. Register it on
the generated SDK surface beside the other `AmmoResource` fields in
`crates/postretro/src/scripting/primitives/mod.rs` and regenerate the committed
`sdk/types/postretro.d.ts` / `.d.luau` fixtures. Document the field and both values in
the `components.weapon` resource row of `docs/scripting-reference.md`, and state there
that `reloadMs` is the duration of one reload *step* — the whole reload under
`magazine`, one shell under `perShell`.

### Task 4: The per-shell loop

Add the `ShellLoading` transitions to the machine. A reload rising edge from `Idle`
consults the effective `reloadStyle`: `Magazine` enters `Reloading` exactly as today,
`PerShell` enters `ShellLoading` with the step timer set to the effective `reloadMs`.
The start guards are shared and unchanged — a full magazine emits the blocked-full
outcome, an empty reserve emits blocked-empty, and neither starts a loop.

On each step expiry, credit exactly one round via `AmmoReserve::take(type, 1)` —
never by indexing the pool — add the returned amount to the magazine, and emit
`reload_shell_loaded`. Then evaluate the loop-continue predicate against live state:
continue when the magazine is below the effective capacity **and** the reserve still
has rounds; otherwise end the reload, emit `reload_completed` carrying the cumulative
transferred count for the whole reload, and return to `Idle`. Continuing restarts the
step timer within the same tick so there is no idle gap between shells; carry the
sub-millisecond remainder across the restart so a step duration that is not a tick
multiple does not accumulate bias. `take` returning `0` — the reserve emptied between
the check and the credit — ends the reload without emitting a shell event.

Add the cancel edge, which is the reason the fuse in Task 2 exists. When the fire
intent would be authorized on this tick — the weapon wants to fire, cooldown has
elapsed, and the magazine holds at least `costPerShot` — a `ShellLoading` state does
not reject the shot; it cancels, discards the in-flight step with no round credited
and no reserve debit, emits `reload_cancelled` carrying the cumulative rounds credited
so far, transitions to `Idle`, and lets the shot resolve on that same tick. A trigger
pull that would not be authorized anyway leaves the loop running and emits no
`dry_fire`, so a player holding the trigger with an empty magazine does not spam the
event drain during a reload. Reload rising edges arriving during `ShellLoading` stay
no-ops, matching the shipped rising-edge dedup. `Reloading` keeps refusing the shot
silently — the atomic style stays uncancellable by fire, per the shipped contract.

Register the two new outcome variants on `ReloadOutcome::event_name` so
`reload_shell_loaded` and `reload_cancelled` reach the same event drain the existing
reload events use.

### Task 5: Raise, lower, stow

Add `raise_ms: u32` and `lower_ms: u32` to `WeaponDescriptor`
(`#[serde(default, rename = "raiseMs" / "lowerMs")]`, both defaulting to `0`), carry
them onto `WeaponComponent` as authored tuning refreshed by `refresh_from_descriptor`,
and surface them through `effective()` / `EffectiveStats` so a later augment can scale
equip speed the same way it scales reload speed. `WeaponDescriptor` is not `Default`,
so every struct literal in the workspace needs the two fields — the compiler
enumerates them.

Add two engine-internal entry points on the machine, `begin_raise` and `begin_lower`.
`begin_raise` is legal from `Idle` and `Stowed`, sets the timer to the effective
`raiseMs`, emits `weapon_raise_started`, and lands in `Raising`; on expiry it emits
`weapon_raised` and returns to `Idle`. Making it legal from `Idle` as well as `Stowed`
is deliberate: a weapon whose spawn path forgets to raise it stays functional rather
than dead. `begin_lower` is legal from every state and preempts whatever is running —
from `Reloading` it aborts with no transfer, from `ShellLoading` it forfeits only the
in-flight step and keeps every credited round, from `Raising` it preempts the raise —
emits `weapon_lower_started`, and lands in `Lowering`; on expiry it emits
`weapon_lowered` and lands in `Stowed`. `Raising`, `Lowering`, and `Stowed` all refuse
fire and reload silently. Both entry points obey the zero-duration collapse rule from
Task 2, so the `0` defaults make equip instantaneous and still emit both endpoint
events.

Call `begin_raise` at the two equip-at-spawn sites where the pawn, the weapon entity,
and the weapon descriptor all coexist: the default-weapon spawn inside
`spawn_from_player_starts` (`crates/postretro/src/scripting/builtins/data_archetype.rs`,
the arm that resolves `weapon_id` and calls `seed_weapon_reserve`) and the analogous
site in `net_descriptor.rs`'s `spawn_net_slot_pawn`. Both must call it — the net-slot
weapon is a remote player's active weapon host-side even though it is never promoted to
the host's own `active_wieldable`. `begin_lower` gets no production caller in this spec;
it is the seam the switching spec drives, and it is exercised here through unit tests
over every source state.

### Task 6: Reload-meter and projection semantics under per-shell

`player.reloadProgress` and `player.reloadActive` are shipped owner-private slots whose
producers are `weapon_hud_values` in
`crates/postretro/src/scripting/systems/ui_proxy.rs` and `AmmoSlotProjection::for_pawn`
in `crates/postretro/src/netcode/state_slots.rs`, both of which read
`WeaponComponent::reload_status()`. Redefine that accessor so `active` is true for the
whole of `Reloading` and the whole of `ShellLoading` — including the tick where one
shell step ends and the next begins, which must not blink false — and `progress` is the
**current step's** ramp. Under `magazine` style that is unchanged; under `perShell` it
is a repeating fill, one per shell, which is the conventional shotgun readout. Extend
the `ReloadFeedback` one-frame endpoint lifecycle to mark step boundaries rather than
whole-reload boundaries so a sub-tick step still contributes its `0.0` and `1.0`
samples, keeping the single-step `magazine` case bit-identical; when a step's end and
the next step's start land in the same tick the completion sample wins, matching the
existing precedent for a reload shorter than one tick. Neither producer needs a new
input: both already reach the component, and the per-shell round count is separately
visible because `player.ammo` republishes the live magazine every frame. No slot is
added, so the replicated schema and its content-derived fingerprint are untouched.
Update the slot descriptions in `docs/scripting-reference.md` and the engine-state
catalog comments to state the step-scoped meaning.

### Task 7: Dev content, docs, and the cross-vantage test suite

Author `content/dev/scripts/reference-shotgun.ts` — `canonicalName
"reference_shotgun"`, `fireMode: "semi"`, `resolution: "hitscan"`, `damage: 12` (matching
the reference pistol so the `target_dummy` three-shot demo math in
`content/dev/scripts/target-dummy.ts` stays correct), a slow `fireRateMs`, an ammo
resource with `reloadStyle: "perShell"`, a magazine of 8, its own ammo `type`, a
starting reserve, a per-shell `reloadMs`, and non-zero `raiseMs` / `lowerMs` — and
register it from `content/dev/start-script.ts` beside the pistol. Point
`content/dev/scripts/player.ts`'s `defaultWeapon` at it so the per-shell loop and the
raise lockout are demoable, and add an explicit `reloadStyle: "magazine"` to
`content/dev/scripts/reference-pistol.ts` so both classifier values appear in dev
content. The pistol archetype stays registered and the flip is one line to revert.

Add the test suite spanning the four vantages named in `research.md` §3. Descriptor
layer: `reloadStyle` / `raiseMs` / `lowerMs` parse, default, and reject identically in
the QuickJS and Luau runtimes, and appear in the generated SDK typedefs. Machine layer:
each transition in the diagram, the loop-exit-on-full and loop-exit-on-empty-reserve
edges, cancel at the first and last step, a would-be-unauthorized trigger pull not
cancelling, `begin_lower` from every source state, `begin_raise` from `Idle` and
`Stowed`, zero-duration collapse consuming no tick, a step duration that is not a tick
multiple not drifting over many steps, and reload-while-cooling still starting.
Hot-reload layer: state, timers, magazine, and edge flags preserved mid-`Reloading` and
mid-`ShellLoading`; a mid-loop style flip ending the reload after the in-flight step.
Vantage layer: the host-simulated remote pawn reaching the same magazine and reserve
counts as the local pawn from the same command sequence; a connected client's predicted
shot during a host-side raise lockout minting no authorized shot and rolling back
through the verdict path. HUD layer: `reloadActive` not blinking across a step
boundary, `reloadProgress` ramping once per shell, `player.ammo` incrementing per shell,
and the owner-private projection reporting the pawn's own values.

## Sequencing

**Phase 1 (sequential):** Task 1 — split before extend; every later task edits the
weapon stage it creates.
**Phase 2 (sequential):** Task 2 — thin slice, falsifies the fusion and placement
assumptions across both simulated vantages before any new authored surface exists.
**Phase 3 (sequential):** Task 3 — the `reloadStyle` discriminant and its boundary
crossings; Task 4 consumes it.
**Phase 4 (sequential):** Task 4 — the per-shell loop and the cancel edge, over the
fused machine and the classifier.
**Phase 5 (sequential):** Task 5 — raise/lower/stow; shares the machine module and the
descriptor struct literals with Task 4.
**Phase 6 (sequential):** Task 6 — meter and projection semantics, over the completed
state set.
**Phase 7 (sequential):** Task 7 — dev content, docs, and the cross-vantage suite,
which assert everything above.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Reload style classifier | `AmmoResource::reload_style`, `EffectiveAmmoStats` | `"reloadStyle"` | `resource.reloadStyle` | same | n/a |
| Atomic style value | `ReloadStyle::Magazine` (default) | `"magazine"` | `"magazine"` | same | n/a |
| Per-shell style value | `ReloadStyle::PerShell` | `"perShell"` | `"perShell"` | same | n/a |
| Raise duration | `WeaponDescriptor::raise_ms`, `EffectiveStats` | `"raiseMs"` (default `0`) | `components.weapon.raiseMs` | same | n/a |
| Lower duration | `WeaponDescriptor::lower_ms`, `EffectiveStats` | `"lowerMs"` (default `0`) | `components.weapon.lowerMs` | same | n/a |
| Step duration (reused) | `AmmoResource::reload_ms` | `"reloadMs"` | `resource.reloadMs` | same | n/a |
| Weapon state | `WeaponComponent::state` (`WeaponState`) | not replicated, not authored | n/a | n/a | n/a |
| Shell credited | `ReloadOutcome` variant → `event_name` | `"reload_shell_loaded"` | reaction / audio consumer | same | n/a |
| Reload cancelled | `ReloadOutcome` variant → `event_name` | `"reload_cancelled"` | reaction / audio consumer | same | n/a |
| Raise endpoints | machine outcome → `event_name` | `"weapon_raise_started"`, `"weapon_raised"` | reaction / audio consumer | same | n/a |
| Lower endpoints | machine outcome → `event_name` | `"weapon_lower_started"`, `"weapon_lowered"` | reaction / audio consumer | same | n/a |
| Reload progress (re-meaning) | `reload_status()` step progress | `player.reloadProgress` | `getGameState().player.reloadProgress` | same | n/a |
| Reload active (re-meaning) | `reload_status()` whole-reload active | `player.reloadActive` | `getGameState().player.reloadActive` | same | n/a |

## Script syntax examples

```typescript
import { defineEntity } from "postretro";

export const referenceShotgunEntity = defineEntity({
  canonicalName: "reference_shotgun",
  components: {
    weapon: {
      damage: 12.0,
      range: 48.0,
      fireRateMs: 700.0,
      fireMode: "semi",
      resolution: "hitscan",
      // Equip transitions. Both default to 0 (instant), which is what every
      // existing weapon gets. Non-zero values lock out fire and reload for
      // their duration.
      raiseMs: 300,
      lowerMs: 200,
      resource: {
        kind: "ammo",
        type: "shells.buck",
        magazine: 8,
        reserve: 32,
        // One shell per step. Firing cancels the loop and keeps loaded shells.
        reloadStyle: "perShell",
        reloadMs: 450,
      },
    },
  },
});
```

Omitting `reloadStyle` gives `"magazine"` — the shipped atomic reload, where
`reloadMs` times the whole reload rather than one step. Omitting `raiseMs` /
`lowerMs` gives instant equip, identical to today.

## Open questions

- **`Lowering` and `Stowed` have no production caller in this spec.** Argued above
  and covered by unit tests, but it means their first production exercise happens in
  the switching spec. If the owner would rather not carry two test-only states, the
  fallback is to ship `Raising` alone and let switching add the mirror — at the cost
  of re-opening the machine's transition legality then rather than settling it now.
- **Concurrent edits with `plans/in-progress/E21--coop-avatar-weapon-presentation/`.**
  That plan adds `thirdPersonModel` / `viewmodel` to the same non-`Default`
  `WeaponDescriptor` and edits `sim/mod.rs`, which Task 1 splits. "The compiler
  enumerates the literals" is a weaker promise while two plans enumerate at once, so
  Task 1 and Task 5 want scheduling against E21 rather than beside it. E21 also
  already builds weapon attach/detach "when a player's active weapon changes" — a
  switch mechanic that does not exist yet — which is worth the owner confirming as
  intended ordering.
- **`defaultWeapon` flips to the shotgun in dev content.** This changes the default
  dev-loop feel (semi, 700 ms cadence, 8 rounds) and is a one-line revert. If the
  pistol should stay the default, the shotgun becomes reachable only once the pickup
  or switching spec lands, and the per-shell loop has no manual demo until then.
