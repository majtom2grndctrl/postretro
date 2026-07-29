# E16 — Weapon State Machine

> **Status:** draft. **Epic:** 16 — Combat. **Milestone:** Weapon Systems.
> **Roadmap item:** `per-shell reload`. The milestone's items are not a strict chain,
> so `heat + cell resources` is not a prerequisite.
> **Builds on (shipped):** `E16--ammo-resource` (magazine, pawn-owned `AmmoReserve`,
> atomic timed reload, reload meter slots); `E16--client-authoritative-combat` (reload
> level bit, its reliable edge lane, the FIRE/HIT authority split).
> **Research notes:** `research.md` — implicit state today (§1), lifecycle diagram
> (§2), vantages (§3), file sizes (§4), multi-pellet (§5), edge transport (§6),
> descriptor surface (§7), hot-reload precedent (§8), extension points (§9), citation
> trail (§10), alternatives explored (§11), foreclosures (§12), test audit (§13).

## Goal

Replace the implicit "weapon state is whichever timer is nonzero" scheme with an
explicit state machine on the weapon instance, validated against two deliberately
different consumers: an interruptible, cancellable **per-shell reload** loop (the
shotgun case) alongside the existing atomic magazine reload, and an uninterruptible
timed **raise** equip transition. Reload and fire are two producers coupled by a
one-way boolean today; that expresses "reload blocks fire" but not "fire cancels
reload". Fusing them is what per-shell needs, and what the switching spec will consume
as its lockout.

## Scope

### In scope

- A `WeaponState` enum on `WeaponComponent` — `Idle`, `Reloading`, `ShellLoading`,
  `Raising` — plus one generalized timed-state triple (remaining / total / sub-ms
  carry) replacing the three reload-specific timer fields.
- Structural openness for states this spec does *not* ship: enum, transition function,
  legality predicates, and timed triple must absorb `Lowering`, `Stowed`, and a
  switch-driven interrupt as added arms and rows, no reshape.
- Fusing `sim/reload.rs::tick` and `weapon/mod.rs::apply_weapon_fire_state` into one
  machine tick called by both the local-pawn and host-simulated-remote-pawn paths.
- A `reloadStyle` classifier on `AmmoResource`: `"magazine"` (default, today's atomic
  behavior) and `"perShell"`. Rust enum, serde, generated SDK typedefs, docs.
- Per-shell reload: `reloadMs` becomes the duration of **one step**; the loop credits
  one round per step from the pawn `AmmoReserve` until the magazine is full or the
  reserve empty. Credited rounds survive cancellation and preemption.
- Cancellation: an otherwise-authorized shot cancels a `ShellLoading` loop, forfeits
  only the in-flight step, and fires on that same tick.
- `raiseMs` on `WeaponDescriptor`, default `0`. Equip-at-spawn drives `begin_raise` at
  both spawn sites. `begin_raise` is legal from every state and preempts — the shape a
  later `begin_lower` or switch interrupt reuses.
- Zero-duration collapse, so `raiseMs = 0` is behavior-identical to instant equip.
- Hot-reload policy for the state field and its timers in `refresh_from_descriptor`,
  including a mid-loop `reloadStyle` flip.
- Script events `reload_shell_loaded`, `reload_cancelled`, `weapon_raise_started`,
  `weapon_raised`, beside the shipped reload and `dry_fire` events.
- Redefining `player.reloadProgress` as the **current step's** progress and
  `player.reloadActive` as true across the whole reload, both styles.
- A behavior-preserving extraction of the weapon stage out of `sim/mod.rs`.
- Dev content: a `reference_shotgun` archetype as the dev player's `defaultWeapon`;
  explicit `reloadStyle: "magazine"` on the reference pistol.
- Tests across all four vantages (`research.md` §3), on self-constructed fixtures that
  survive a change of dev `defaultWeapon`.

### Out of scope

- **`Lowering` and `Stowed`.** No production driver until switching exists, and
  `Stowed`'s "no weapon is live" semantics are entangled with `active_wieldable`
  repointing, which this spec does not touch. The switching spec owns both. Safe to
  defer only because of the openness requirement above.
- **Weapon switching, inventory, pickups.** No `active_wieldable` repointing.
- **Multi-pellet firing and spread.** `pellet_count` stays `1` (`research.md` §5).
- Weapon animation — none exists; the viewmodel's only motion is the borrowed pawn
  `movement.viewFeel` in `viewmodel_world_transform`. Raise is a lockout plus edges.
- `Cooling` as a state. Cooldown stays an orthogonal rate limiter composing with
  `Idle` — a reload may start while cooling, as today.
- A rack/finish stage after a cancelled per-shell reload (`research.md` §11).
- `shellsPerStep`; the `"internal"` / `"energy"` reload styles.
- Any new HUD or engine-state slot. No new replicated slot, no fingerprint change.
- Client-side prediction of reload, steps, or raise. V3 keeps predicting cooldown only
  and reconciles through the shipped `ShotVerdict` path.
- `heat` / `cell` resource variants, augments, charge, secondary activation.

## Direction

**Problem.** Reload and fire are two producers of weapon state that cannot see each
other's decision. `reload::tick` runs first and hands `apply_weapon_fire_state` a single
boolean, `reload_started_this_tick`; the fire gate can consult reload, reload can never
consult fire. That is exactly expressive enough for the shipped atomic reload and
structurally incapable of the per-shell interrupt — per-shell is not a feature that can
be added to this shape, it requires changing it. The observation behind that: every
candidate "fire cancels reload" against today's code smuggles the decision upstream
through a second boolean, and the second boolean is where "state is whichever timer is
nonzero" stops being shorthand and becomes a bug surface.

**Placement.** The machine is one phase both sim call sites run, above
`tick_resolved_component` and `tick_state_only_component` rather than inside either —
it subsumes the gate decision those two currently delegate to the private
`apply_weapon_fire_state`, and it must run under the mutable registry borrow the
`AmmoReserve` transfer needs, which no callee below them holds (Task 2 resolves the
borrow inversion). `tick_resolved_component` is the tempting home, since firing visibly
happens there, and it would silently skip the remote-pawn vantage, which runs
`tick_state_only_component`. The axis is *shared-phase vs. per-caller*, not
engine-vs-mod: `reloadStyle` and `raiseMs` are authored data, the machine reading them is
engine floor, matching the shipped `fireMode` / `resolution` split. Warrant:
`research.md` §3.

**Open for extension.** This spec ships four states and defers `Lowering`, `Stowed`, and
the switch interrupt. That is sound only if adding them later is arms-and-rows work, so
openness is a first-class requirement here, not a hope: one transition function keyed by
(state, event); legality as exhaustive per-variant predicates; a state-agnostic timed
triple; no `_` wildcard arms over `WeaponState` in production; and a preempting entry
point implemented and tested here for the next one to reuse. Task 2 builds it, AC 17
demonstrates it, `research.md` §9 maps each future extension to the piece that absorbs
it. `movement--cross-cutting-policies` D7 settled the same question for movement —
per-state live data owned through one uniform convention, so adding a state never widens
the dispatch — and the state-agnostic timed triple is that answer applied here. With this,
the deferred states cost two variants and their rows when their driver arrives.

**Prior commitments.** `weapon-model.md` §3 already calls per-shell reload "a cancellable
state machine" and puts `reloadStyle` on the resource — honored, with a stated casing
divergence (`"perShell"`, not the sketched `"per-shell"`; the repo's enum serde
convention is camelCase). `E16--ammo-resource` deferred this classifier here and made
reload duration an effective stat — both honored. `networking.md` §Combat authority keeps
ammo and reload host-authoritative — untouched. `reload-feedback-ui` promised a later
spec would point a real machine at the shipped meter slot with no UI rework — Task 6
keeps it. `movement--state-machine` is the structural precedent Tasks 1–2 follow.
**Stated divergence:** `E10--behavior-state-graph` retired engine-closed FSMs for
*enemies* in favor of authored graphs; weapons take the movement answer instead, because
these transitions are timing and resource legality, not authored behavior.

**Second stated divergence — equip machinery lands weapon-named.** `weapon-model.md` §2
holds that identity, inventory, **equip**, switch, and augment belong to the *wieldable*
layer, and says to name the machinery for wieldables, not weapons. `Raising` and
`raiseMs` are an equip transition, and this spec puts both on `WeaponComponent` /
`WeaponDescriptor`. Deliberate: weapon is the only wieldable kind that exists, there is no
wieldable component to host the machine, and inventing one to hold a single field would be
speculative generality. The cost is real and is inherited, not avoided — if the switching
spec needs the machine to serve a second wieldable kind, lifting it off `WeaponComponent`
is exactly the reshape the openness requirement otherwise prevents, and `research.md` §9's
extension table does not cover it. The switching spec inherits the naming question
alongside `Lowering` and `Stowed`; renaming while weapons are the only kind is a
mechanical rename, and it gets more expensive per wieldable kind added. Citations:
`research.md` §10.

**Foreclosures and one-way doors.** The one-way door is `reloadMs` becoming
style-dependent: it means "whole reload" or "one shell" depending on the sibling
`reloadStyle`, so flipping a weapon's style silently re-means a shipped authored number —
no parse error, no migration signal. Respelling the `reloadStyle` discriminant itself is
cheaper than it looks while dev content is the only content (a two-file edit), and gets
expensive only once shipped mods exist. Foreclosed, each accepted: cooldown can never
become a state without a behavior change, since a reload can start while cooling today
and the machine keeps that true; one state at a time, so a single instance with two
concurrent activities is unrepresentable (dual-wield unaffected — each instance keeps its
own machine); whole-reload progress becomes unobservable under `perShell`, because
`reload_status()` goes step-scoped and nothing publishes a cumulative ramp, so a HUD
wanting "40% through the whole reload" needs a new slot later; fire-cancels-reload is a
property of the `reloadStyle` discriminant, not an authorable policy surface, deliberately
unlike `E16--impact-policy-substrate`'s engine-chokepoint/policy-meaning split, because
cancellability is what distinguishes the two styles rather than something layered onto
them; and `Raising` refuses reload silently rather than queueing it, a rule every
preempting state switching adds will inherit. Detail, and what this hands the switching
spec: `research.md` §12.

**Alternatives rejected.** The strongest rival is **shipping `Lowering` and `Stowed`
now**: the two halves of an equip transition are conceptually one thing, and pinning
transition legality now means the switching spec cannot re-open it. Rejected — neither
has a production caller, both would ship as test-only states, and the openness
requirement makes adding them additive. What that rival really carried (an
uninterruptible timed state is a different shape from an interruptible loop, and
preempt-from-any-state needs a live implementation) is carried by `Raising` alone, which
is timed, uninterruptible by fire, and preempting from every source state.

**Why `Raising` clears the bar `Lowering` does not**, stated plainly because the
asymmetry is thinner than it looks: `Raising`'s only live consumer is the dev shotgun
this spec authors, since `raiseMs` defaults to `0` and every existing weapon collapses it
away. So "it has a driver" is partly manufactured here. It clears the bar anyway on three
counts `Lowering` fails. A draw delay is wanted gameplay for this genre, not scaffolding —
it ships as content, not as a test hook. Its call site already exists and is unambiguous
(equip-at-spawn, two sites), where `begin_lower`'s caller does not exist in any form.
And preempt-from-any-state is design work the switching spec needs regardless; validating
it against one live caller now is cheaper than inventing it cold, and `raiseMs = 0` keeps
the addition strictly additive for every weapon that does not want it.

Runner-up alternative: keep two producers and add a second boolean (`fire_wants_cancel`)
— minimal diff, works for this one interrupt, rejected because switching adds a third and
the ordering constraints between three become unwritable. Deferring `Raising` too, and
the rest: `research.md` §11.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A weapon is in exactly one state; the fire gate authorizes a shot only from `Idle` | Task 2 | states added in Tasks 4 and 5 must extend the legality predicates, not bypass them | AC 3, 8, 9, 20 |
| The machine stays open for extension: new states are arms and rows, never a reshape | Task 2 | Tasks 4 and 5 are the first two extensions and must land as arms and rows themselves | AC 17 |
| Rounds credited by a per-shell loop are never rolled back — not by cancel, not by a preempting `begin_raise`, not by hot reload | Task 4 | Task 5's preempt path; Task 2's hot-reload policy | AC 7, 10, 12 |
| The pawn `AmmoReserve` is debited exactly once per credited round, through `AmmoReserve::take` only | Task 4 | the tick where one step completes and the next starts | AC 4, 7 |
| Hot reload never changes the current state or its remaining time; it refreshes durations and style only | Task 2 | Tasks 4 and 5 adding fields that must join the preserved set | AC 12 |
| A timed state entered with effective duration `0` resolves at entry: no tick elapses, the fire gate never observes it | Task 2 (rule), Task 5 (raise) | any new timed state | AC 9 |
| The local-pawn and host-simulated-remote-pawn vantages run the identical machine | Task 2 | Tasks 4 and 5 adding a path only one caller reaches | AC 13 |
| Tests are independent of dev content: no test's outcome depends on which archetype the dev mod equips as `defaultWeapon` | Task 2 (fixture convention) | every task that adds a test; Task 7's `defaultWeapon` flip | AC 18 |
| No wire field, `WIRE_VERSION`, app-protocol, or replicated-slot-fingerprint change | Task 2 | Tasks 4, 5, 6 | AC 16 |

## Acceptance criteria

- [ ] Authors can set `components.weapon.resource.reloadStyle` to `"magazine"` or
  `"perShell"` in TypeScript and Luau; an absent value parses as `"magazine"`; any other
  value is rejected at deserialize, identically in both runtimes. Generated SDK typedefs
  and `docs/scripting-reference.md` carry the field and both values.
- [ ] Authors can set `raiseMs` on `components.weapon`; it defaults to `0` and rejects
  non-integer and negative values at deserialize.
- [ ] A weapon authored `reloadStyle: "magazine"` (or with the field absent) reloads
  exactly as today: one timer of `reloadMs`, one atomic transfer at completion,
  uncancellable by firing, same four reload events with the same `transferred` count.
  The shipped atomic-reload tests pass unchanged.
- [ ] A `perShell` reload loads one round per `reloadMs`: after N steps the magazine has
  grown by exactly N and the pawn reserve shrunk by exactly N, one `reload_shell_loaded`
  per step.
- [ ] The loop ends on its own when the magazine reaches capacity, and separately when
  the reserve reaches zero with the magazine still short. Both emit one
  `reload_completed` whose `transferred` is the cumulative total for the whole reload,
  and leave the weapon fire-ready.
- [ ] A reload press with a full magazine, or an empty reserve, emits the blocked
  outcome and starts no loop — both styles.
- [ ] An otherwise-authorized shot during a `perShell` loop cancels it: the shot resolves
  on that same tick, the in-flight step is discarded with no round credited, previously
  credited rounds stay in the magazine, the reserve is not refunded, one
  `reload_cancelled` fires. Cancelling at the first step (zero credited) and at the last
  possible step both behave this way.
- [ ] A trigger pull during a `perShell` loop that would *not* be authorized anyway —
  cooldown not elapsed, or magazine below `costPerShot` — does not cancel the loop and
  emits no `dry_fire`.
- [ ] With `raiseMs = 0` (the default) a weapon is fire-ready on the tick it is equipped
  — no tick lost, shipped equip-at-spawn tests pass unchanged — while still emitting
  `weapon_raise_started` and `weapon_raised`. With `raiseMs > 0` a shot attempted before
  completion is refused silently, no reload block event is emitted either, and the first
  authorized shot lands on the first tick after completion.
- [ ] `begin_raise` is legal from every state and preempts: from `Reloading` it aborts
  with no transfer; from `ShellLoading` it forfeits only the in-flight step and keeps
  every credited round; from `Raising` it restarts the raise.
- [ ] Reload can still start while the weapon is cooling; cooldown never blocks a reload
  and reload never resets cooldown.
- [ ] Hot-reloading the descriptor mid-`ShellLoading` and mid-`Reloading` preserves the
  state, remaining time, sub-millisecond carry, magazine, and input-edge flags, while
  adopting the new `reloadMs`, `raiseMs`, and `reloadStyle` from the next decision point
  onward. A `perShell` → `magazine` flip mid-loop lets the in-flight step credit its
  round, then ends the reload rather than looping.
- [ ] A co-op remote client's pawn, simulated host-side, exhibits every behavior above —
  per-shell steps, cancel-on-fire, raise lockout — identically to the single-player pawn,
  driven through the same command path.
- [ ] A connected client that predicts a shot the host refuses because the weapon is
  raising or reloading sees it rolled back through the existing verdict path: no
  authorized shot minted, predicted cooldown restored, muzzle/hitmarker feedback cleared.
- [ ] `player.reloadActive` is true for the entire duration of a reload of either style,
  including across step boundaries, and false at every other time.
  `player.reloadProgress` ramps `0 → 1` once per reload for `magazine` and once per shell
  for `perShell`. `player.ammo` increments once per credited shell. The owner-private
  projection publishes the same values to a remote client's owner.
- [ ] No wire message, `WIRE_VERSION`, app-protocol constant, replicated-slot schema
  entry, or state-slot fingerprint input changes (review/grep gate). No switching,
  inventory, pickup, or multi-pellet behavior is built, and no `Lowering` or `Stowed`
  variant is added (review/grep gate). No new `unsafe` (review/grep gate).
- [ ] Openness is demonstrated, not asserted: adding a placeholder `WeaponState` variant
  produces compile errors only at the transition function and the two legality predicates
  — no other production site — and needs no new timer field and no change to the
  outcome-to-event mapping. The throwaway branch proving it is the verification; it is
  not merged.
- [ ] Every test covering weapon state, reload style, or equip timing builds its own
  weapon descriptor or component fixture; no test reads the dev mod's `defaultWeapon` or
  asserts a value that holds only while a particular archetype is the dev default.
  Setting `content/dev/scripts/player.ts`'s `defaultWeapon` to either registered
  archetype leaves the whole suite green, checked both ways.
- [ ] The weapon-stage split is behavior-preserving: the existing suite passes with no
  changes beyond import paths and module placement, and no caller outside the sim module
  changes — the netcode hit-acceptance path still reaches the authorized-impact entry
  point at its current path.
- [ ] A weapon ticked with no owning pawn still advances timers, resolves expiries, and
  gates fire, but refuses every reload as if the reserve were empty — it can fire and
  cool, it cannot reload or credit a shell.
- [ ] Launching the dev level equips a per-shell weapon: firing to empty, holding reload,
  watching the magazine climb one round at a time, and firing mid-loop to cancel all work
  from dev content alone, with both classifier values present across the two reference
  weapon archetypes.

## Tasks

### Task 1: Extract the weapon stage out of `sim/mod.rs`

Behavior-preserving split, no functional change. `crates/postretro/src/sim/mod.rs`
carries 1446 production lines and this plan extends its weapon orchestration. Move into
a new `crates/postretro/src/sim/weapon_stage.rs`: `weapon_fire_command`,
`normalize_aim_direction`, `run_remote_weapon_commands`, `run_local_weapon_command`,
`apply_weapon_impact_damage`, `apply_authorized_weapon_impact_damage`,
`apply_weapon_impact_damage_with_source`, and the test-only `deliver_reload_to_weapon`.
Keep `run_death_sweep` in `sim/mod.rs` — it sits inside that address range but is the
death stage. Re-export whatever `sim/mod.rs` and `crates/postretro/src/netcode/` already
import at their existing paths so no caller outside the sim module changes;
`apply_authorized_weapon_impact_damage` in particular is consumed by the netcode
hit-acceptance path. Move the weapon-stage tests with the code. The tick-order call sites
inside `simulate_tick` (`sim/mod.rs:390`, `:396`) stay put and call into the new module.

### Task 2: `WeaponState` + the fused machine tick (thin slice)

Narrow vertical slice through every seam, reproducing today's exact behavior with no new
authored surface and no new production states. Add to `WeaponComponent`
(`crates/entities/src/components/weapon.rs`) a `state: WeaponState` field
(`#[serde(default)]`, `Default = Idle`) and replace `reload_remaining_ms`,
`reload_total_ms`, and `reload_elapsed_sub_ms` with one generalized timed-state triple
owned by the machine — remaining ms, total ms, and the fractional carry that prevents
per-tick rounding bias. Define the four variants this spec ships (`Idle`, `Reloading`,
`ShellLoading`, `Raising`) so later tasks add transitions rather than variants; only
`Idle` and `Reloading` are reachable in production after this task. `from_descriptor`
materializes `Idle`. Update the `WeaponComponent` literals across the workspace that the
field changes break — the compiler enumerates them.

Fuse the two producers. `sim/reload.rs::tick` and the private
`weapon/mod.rs::apply_weapon_fire_state` become one machine tick with a single ordered
decision per fixed tick: advance the state timer, resolve any expiry, then evaluate the
fire and reload intents against the resulting state. It must sit in the shared private
callee that both `tick_resolved_component` and `tick_state_only_component` already
delegate to, so both simulated vantages get one implementation; a placement inside
`tick_resolved_component` would skip the remote-pawn path entirely. The
`reload_started_this_tick` boolean and its two `deliveries.iter().any(...)` computations
at the call sites disappear; the machine returns its own outcome list plus the fire
authorization. Keep `ReloadDelivery`, `ReloadOutcome`, and every existing event name and
payload byte-identical.

Build it **open for extension** — `Lowering`, `Stowed`, and a switch interrupt land in a
later spec and must not force a reshape. Five rules, all load-bearing: (a) one transition
function keyed by (current state, event), so a new state adds arms, not a dispatch shape;
(b) fire and reload legality expressed as `WeaponState` predicates written per variant,
not inline `state != Idle` tests scattered through the gate; (c) the timed triple stays
state-agnostic, so a new timed state adds no field; (d) no `_` wildcard arm over
`WeaponState` anywhere in production, so a new variant is a compile error at every site
that must decide about it; (e) outcome-to-event mapping stays name-driven through
`ReloadOutcome::event_name`, so a new endpoint pair needs no new plumbing.

Two plumbing facts the fusion turns up, both settled here rather than by the implementer.
First, a **borrow inversion**: the machine needs `&mut EntityRegistry` for the
`AmmoReserve` transfer, which today only `reload::tick` holds, while
`tick_resolved_component` takes `&EntityRegistry` because hitscan resolution borrows it
immutably for the whole call. Split the call into two phases at both call sites — run the
machine to completion under the mutable borrow, producing a fire authorization and an
outcome list, then release it and run hitscan resolution under the immutable borrow using
that authorization. Do not widen the resolution signature to `&mut`; the
renderer-adjacent read path stays a read. Second, **a weapon with no pawn**:
`run_local_weapon_command` takes `pawn: Option<EntityId>` and today skips reload entirely
when it is `None` (fly-camera and headless harnesses). The machine still advances timers,
resolves expiries, and gates fire, but any transition that would touch the reserve is
refused as if the reserve were empty — a pawnless weapon fires and cools, cannot reload.

Define two rules the later tasks depend on. **Zero-duration collapse:** entering a timed
state whose effective duration is `0` resolves it to its successor within the same call —
no tick elapses, no consumer observes the intermediate state, both endpoint events still
fire. **Hot-reload policy:** extend `refresh_from_descriptor`'s preserved set to include
`state` and the timed triple, alongside the cooldown, magazine, and input edges it
already preserves; durations and classifiers refresh and take effect at the next decision
point, matching the existing precedent that reload completion re-reads `effective()`.
Rewrite `reload_status()` to derive `(progress, active)` from the state plus the timers
rather than from `reload_remaining_ms > 0`, keeping today's outputs identical for `Idle`
and `Reloading` including the one-frame `ReloadFeedback` endpoints.

Establish the fixture convention every later task's tests inherit: machine and component
tests construct their own `WeaponDescriptor` / `WeaponComponent` values in-test. No test
may reach for the dev mod's `defaultWeapon` or encode an assumption about which archetype
dev content equips, so Task 7's `defaultWeapon` flip — and any future one — is a content
edit, not a test edit.

### Task 3: `ReloadStyle` classifier on the ammo resource

Add `ReloadStyle { Magazine, PerShell }` to
`crates/foundation/src/data_descriptors/types/combat.rs`, deriving the same trait set as
the sibling `FireMode` / `ResolutionMode` enums and carrying
`#[serde(rename_all = "camelCase")]` so wire values are `"magazine"` and `"perShell"`.
Add `reload_style: ReloadStyle` to `AmmoResource` with
`#[serde(default, rename = "reloadStyle")]` and a `Default` impl returning `Magazine`, so
every existing authored resource block and struct literal keeps today's behavior. No new
`validate()` rule is needed — serde rejects unknown values — but state that in the task's
test list rather than leaving it implicit. Carry the field through `WeaponAmmoTuning` and
`EffectiveAmmoStats` in `crates/entities/src/components/weapon.rs` so producers read it
through `effective()`, never off the descriptor, matching how `reload_ms` is already
routed. Register it beside the other `AmmoResource` fields in
`crates/postretro/src/scripting/primitives/mod.rs` and regenerate the committed
`sdk/types/postretro.d.ts` / `.d.luau` fixtures. Document the field and both values in the
`components.weapon` resource row of `docs/scripting-reference.md`, stating there that
`reloadMs` is the duration of one reload *step* — the whole reload under `magazine`, one
shell under `perShell`.

### Task 4: The per-shell loop

Add the `ShellLoading` transitions as new arms on the existing transition function and new
rows on the legality predicates — not a reshape. A reload rising edge from `Idle` consults
the effective `reloadStyle`: `Magazine` enters `Reloading` exactly as today, `PerShell`
enters `ShellLoading` with the step timer set to the effective `reloadMs`. Start guards
are shared and unchanged — a full magazine emits blocked-full, an empty reserve emits
blocked-empty, neither starts a loop.

On each step expiry, credit exactly one round via `AmmoReserve::take(type, 1)` — never by
indexing the pool — add the returned amount to the magazine, and emit
`reload_shell_loaded`. Then evaluate the loop-continue predicate against live state:
continue when the magazine is below the effective capacity **and** the reserve still has
rounds; otherwise end the reload, emit `reload_completed` carrying the cumulative
transferred count for the whole reload, and return to `Idle`. Continuing restarts the step
timer within the same tick so there is no idle gap between shells; carry the
sub-millisecond remainder across the restart so a step duration that is not a tick
multiple does not accumulate bias. `take` returning `0` — the reserve emptied between
check and credit — ends the reload without emitting a shell event.

Add the cancel edge, the reason Task 2's fusion exists. When the fire intent would be
authorized on this tick — the weapon wants to fire, cooldown has elapsed, the magazine
holds at least `costPerShot` — `ShellLoading` does not reject the shot: it cancels,
discards the in-flight step with no round credited and no reserve debit, emits
`reload_cancelled` carrying the cumulative rounds credited so far, transitions to `Idle`,
and lets the shot resolve on that same tick. A trigger pull that would not be authorized
anyway leaves the loop running and emits no `dry_fire`, so a player holding the trigger
with an empty magazine does not spam the event drain during a reload. Reload rising edges
during `ShellLoading` stay no-ops, matching the shipped rising-edge dedup. `Reloading`
keeps refusing the shot silently — the atomic style stays uncancellable by fire, per the
shipped contract. Register the two new outcome variants on `ReloadOutcome::event_name` so
both events reach the existing event drain.

### Task 5: Raise

Add `raise_ms: u32` to `WeaponDescriptor` (`#[serde(default, rename = "raiseMs")]`,
default `0`), carry it onto `WeaponComponent` as authored tuning refreshed by
`refresh_from_descriptor`, and surface it through `effective()` / `EffectiveStats` so a
later augment can scale equip speed the way it scales reload speed. `WeaponDescriptor` has
no `Default` impl, so every struct literal in the workspace needs the field — the compiler
enumerates them. Add no `validate()` rule: `u32` deserialization already rejects negative
and non-integer values, and `0` must stay legal — do not copy `reloadMs`'s `>= 1` rule
onto `raiseMs`.

Add one engine-internal entry point, `begin_raise`, as new arms on the existing transition
function. It is **legal from every state and preempts whatever is running**: from `Idle`
it simply starts; from `Reloading` it aborts the reload with no transfer; from
`ShellLoading` it forfeits only the in-flight step and keeps every credited round; from
`Raising` it restarts the raise. It sets the timer to the effective `raiseMs`, emits
`weapon_raise_started`, and lands in `Raising`; on expiry it emits `weapon_raised` and
returns to `Idle`. `Raising` refuses both fire and reload silently — no `dry_fire`, no
reload block event. Preempt-from-any-state is deliberate: it is the shape the switching
spec's `begin_lower` and switch interrupt reuse, so its forfeit rule (credited rounds
survive, in-flight step does not) is the general rule, not a raise-specific one.
`begin_raise` obeys Task 2's zero-duration collapse, so the `0` default makes equip
instantaneous and still emits both endpoint events.

Call `begin_raise` at the two equip-at-spawn sites where the pawn, the weapon entity, and
the weapon descriptor all coexist: the default-weapon spawn inside `spawn_from_player_starts`
(`crates/postretro/src/scripting/builtins/data_archetype.rs`, the arm that resolves
`weapon_id` and calls `seed_weapon_reserve`) and the analogous site in
`net_descriptor.rs`'s `spawn_net_slot_pawn`. Both must call it — the net-slot weapon is a
remote player's active weapon host-side even though it is never promoted to the host's own
`active_wieldable`.

### Task 6: Reload-meter and projection semantics under per-shell

`player.reloadProgress` and `player.reloadActive` are shipped owner-private slots whose
producers are `weapon_hud_values` in
`crates/postretro/src/scripting/systems/ui_proxy.rs` and `AmmoSlotProjection::for_pawn` in
`crates/postretro/src/netcode/state_slots.rs`, both reading
`WeaponComponent::reload_status()`. Redefine that accessor so `active` is true for the
whole of `Reloading` and the whole of `ShellLoading` — including the tick where one step
ends and the next begins, which must not blink false — and `progress` is the **current
step's** ramp. Under `magazine` that is unchanged; under `perShell` it is a repeating fill,
one per shell, the conventional shotgun readout. Extend the `ReloadFeedback` one-frame
endpoint lifecycle to mark step boundaries rather than whole-reload boundaries so a
sub-tick step still contributes its `0.0` and `1.0` samples, keeping the single-step
`magazine` case bit-identical; when a step's end and the next step's start land in the same
tick the completion sample wins, matching the existing precedent for a reload shorter than
one tick. Neither producer needs a new input — both already reach the component
(`research.md` §3, V4 warrant) — and no slot is added, so the replicated schema and its
content-derived fingerprint are untouched. Update the slot descriptions in
`docs/scripting-reference.md` and the engine-state catalog comments to state the
step-scoped meaning.

### Task 7: Dev content, docs, and the cross-vantage test suite

Author `content/dev/scripts/reference-shotgun.ts` — `canonicalName "reference_shotgun"`,
`fireMode: "semi"`, `resolution: "hitscan"`, `damage: 12` (matching the reference pistol so
the `target_dummy` three-shot demo math in `content/dev/scripts/target-dummy.ts` stays
correct), a slow `fireRateMs`, an ammo resource with `reloadStyle: "perShell"`, a magazine
of 8, its own ammo `type`, a starting reserve, a per-shell `reloadMs`, and a non-zero
`raiseMs` — and register it from `content/dev/start-script.ts` beside the pistol. Point
`content/dev/scripts/player.ts`'s `defaultWeapon` at it, replacing `reference_pistol`, so
the per-shell loop and raise lockout are demoable without switching or pickups. That flip
is decided, not proposed; it is the only production consumer of the per-shell path in this
spec, and it is one line to revert. Add an explicit `reloadStyle: "magazine"` to
`content/dev/scripts/reference-pistol.ts` so both classifier values appear in dev content;
the pistol archetype stays registered.

Add the test suite spanning the four vantages in `research.md` §3. Every fixture is
constructed in-test — no test reads dev content or assumes which archetype is the dev
`defaultWeapon`; verify by running the suite with `defaultWeapon` set to each of the two
archetypes in turn. Descriptor layer: `reloadStyle` / `raiseMs` parse, default, and reject
identically in the QuickJS and Luau runtimes, and appear in the generated SDK typedefs.
Machine layer: every transition in the `research.md` §2 diagram, loop-exit on full and on
empty reserve, cancel at the first and last step, a would-be-unauthorized trigger pull not
cancelling, `begin_raise` from every source state including mid-`Reloading` and
mid-`ShellLoading`, zero-duration collapse consuming no tick, a step duration that is not a
tick multiple not drifting over many steps, and reload-while-cooling still starting.
Hot-reload layer: state, timers, magazine, and edge flags preserved mid-`Reloading` and
mid-`ShellLoading`; a mid-loop style flip ending the reload after the in-flight step.
Vantage layer: the host-simulated remote pawn reaching the same magazine and reserve counts
as the local pawn from the same command sequence; a connected client's predicted shot during
a host-side raise lockout minting no authorized shot and rolling back through the verdict
path. HUD layer: `reloadActive` not blinking across a step boundary, `reloadProgress`
ramping once per shell, `player.ammo` incrementing per shell, and the owner-private
projection reporting the pawn's own values.

## Sequencing

**Phase 1 (sequential):** Task 1 — split before extend; every later task edits the weapon
stage it creates.
**Phase 2 (sequential):** Task 2 — thin slice; falsifies the fusion, the placement, and the
extension shape across both simulated vantages before any authored surface exists.
**Phase 3 (sequential):** Task 3 — the `reloadStyle` discriminant and its boundary
crossings; Task 4 consumes it.
**Phase 4 (sequential):** Task 4 — the per-shell loop and cancel edge, over the fused
machine and the classifier.
**Phase 5 (sequential):** Task 5 — raise; shares the machine module and the descriptor
struct literals with Task 4.
**Phase 6 (sequential):** Task 6 — meter and projection semantics, over the completed state
set.
**Phase 7 (sequential):** Task 7 — dev content, docs, and the cross-vantage suite, which
assert everything above.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Reload style classifier | `AmmoResource::reload_style`, `EffectiveAmmoStats` | `"reloadStyle"` | `resource.reloadStyle` | same | n/a |
| Atomic style value | `ReloadStyle::Magazine` (default) | `"magazine"` | `"magazine"` | same | n/a |
| Per-shell style value | `ReloadStyle::PerShell` | `"perShell"` | `"perShell"` | same | n/a |
| Raise duration | `WeaponDescriptor::raise_ms`, `EffectiveStats` | `"raiseMs"` (default `0`) | `components.weapon.raiseMs` | same | n/a |
| Step duration (reused) | `AmmoResource::reload_ms` | `"reloadMs"` | `resource.reloadMs` | same | n/a |
| Weapon state | `WeaponComponent::state` (`WeaponState`) | not replicated, not authored | n/a | n/a | n/a |
| Shell credited | `ReloadOutcome` variant → `event_name` | `"reload_shell_loaded"` | reaction / audio consumer | same | n/a |
| Reload cancelled | `ReloadOutcome` variant → `event_name` | `"reload_cancelled"` | reaction / audio consumer | same | n/a |
| Raise endpoints | machine outcome → `event_name` | `"weapon_raise_started"`, `"weapon_raised"` | reaction / audio consumer | same | n/a |
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
      // Equip lockout. Defaults to 0 (instant) — what every existing weapon gets.
      raiseMs: 300,
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

Omitting `reloadStyle` gives `"magazine"` — the shipped atomic reload, where `reloadMs`
times the whole reload rather than one step. Omitting `raiseMs` gives instant equip,
identical to today.
