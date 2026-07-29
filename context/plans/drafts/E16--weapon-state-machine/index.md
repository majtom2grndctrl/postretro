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
different reload shapes: an interruptible, cancellable **per-shell reload** loop (the
shotgun case) alongside the existing uninterruptible atomic magazine reload. Reload and
fire are two producers coupled by a one-way boolean today; that expresses "reload blocks
fire" but not "fire cancels reload". Fusing them is what per-shell needs, and what the
switching spec will consume as its lockout.

## Scope

### In scope

- A `WieldableState` enum hosted on `WeaponComponent` — `Idle`, `Reloading`,
  `ShellLoading` — plus one generalized timed-state triple (remaining / total / sub-ms
  carry) replacing the three reload-specific timer fields.
- Structural openness for states this spec does *not* ship: enum, transition function,
  legality predicates, and timed triple must absorb `Raising`, `Lowering`, `Stowed`, and
  a switch-driven interrupt as added arms and rows, no reshape.
- Fusing `sim/reload.rs::tick` and `weapon/mod.rs::apply_weapon_fire_state` into one
  machine tick called by both the local-pawn and host-simulated-remote-pawn paths.
- A `reloadStyle` classifier on `AmmoResource`: `"magazine"` (default, today's atomic
  behavior) and `"perShell"`. Rust enum, serde, generated SDK typedefs, docs.
- Per-shell reload: `reloadMs` becomes the duration of **one step**; the loop credits
  one round per step from the pawn `AmmoReserve` until the magazine is full or the
  reserve empty. Credited rounds survive cancellation.
- Cancellation: an otherwise-authorized shot cancels a `ShellLoading` loop, forfeits
  only the in-flight step, and fires on that same tick.
- Hot-reload policy for the state field and its timers in `refresh_from_descriptor`,
  including a mid-loop `reloadStyle` flip.
- Script events `reload_shell_loaded` and `reload_cancelled`, beside the shipped reload
  and `dry_fire` events.
- Redefining `player.reloadProgress` as the **current step's** progress and
  `player.reloadActive` as true across the whole reload, both styles.
- A behavior-preserving extraction of the weapon stage out of `sim/mod.rs`.
- Dev content: a `reference_shotgun` archetype as the dev player's `defaultWeapon`;
  explicit `reloadStyle: "magazine"` on the reference pistol.
- Tests across all four vantages (`research.md` §3), on self-constructed fixtures that
  survive a change of dev `defaultWeapon`.

### Out of scope

- **The equip lifecycle — `Raising`, `Lowering`, `Stowed`, and any equip-timing field
  such as a `raiseMs`.** No production driver until switching exists, and `Stowed`'s "no
  weapon is live" semantics are entangled with `active_wieldable` repointing, which this
  spec does not touch. The switching spec owns the whole lifecycle, so equip semantics
  get pinned by the spec that owns equip rather than by this one. Safe to defer only
  because of the openness requirement above.
- **Weapon switching, inventory, pickups.** No `active_wieldable` repointing.
- **Multi-pellet firing and spread.** `pellet_count` stays `1` (`research.md` §5).
- Weapon animation — none exists; the viewmodel's only motion is the borrowed pawn
  `movement.viewFeel` in `viewmodel_world_transform`.
- `Cooling` as a state. Cooldown stays an orthogonal rate limiter composing with
  `Idle` — a reload may start while cooling, as today.
- A rack/finish stage after a cancelled per-shell reload (`research.md` §11).
- `shellsPerStep`; the `"internal"` / `"energy"` reload styles.
- Any new HUD or engine-state slot. No new replicated slot, no fingerprint change.
- Client-side prediction of reload or steps. V3 keeps predicting cooldown only and
  reconciles through the shipped `ShotVerdict` path.
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
`AmmoReserve` transfer needs, which no callee below them holds (Task 2 names its home
and pins the borrow discipline). `tick_resolved_component` is the tempting home, since firing visibly
happens there, and it would silently skip the remote-pawn vantage, which runs
`tick_state_only_component`. The axis is *shared-phase vs. per-caller*, not
engine-vs-mod: `reloadStyle` is authored data, the machine reading it is engine floor,
matching the shipped `fireMode` / `resolution` split. Warrant: `research.md` §3.

**Open for extension.** This spec ships three states and defers the whole equip lifecycle
— `Raising`, `Lowering`, `Stowed` — plus the switch interrupt. That is sound only if
adding them later is arms-and-rows work, so openness is a first-class requirement here,
not a hope: one transition function keyed by (state, event); legality as exhaustive
per-variant predicates; a state-agnostic timed triple; and no `_` wildcard arms over
`WieldableState` in production. Task 2 builds it, AC 14 demonstrates it, `research.md` §9
maps each future extension to the piece that absorbs it.
`movement--cross-cutting-policies` D7 settled the same question for movement — per-state
live data owned through one uniform convention, so adding a state never widens the
dispatch — and the state-agnostic timed triple is that answer applied here. With this,
the deferred states cost three variants and their rows when their driver arrives.

One piece of that openness ships **unexercised**, and it is the honest cost of deferring
equip. A preempting entry point — one legal from every state, forfeiting the in-flight
timed step while keeping credited rounds — is the shape `begin_lower` and the switch
interrupt need, and no consumer in this spec reaches it. The transition function's
(state, event) keying admits it as new arms, but this spec neither implements nor tests
it, so the switching spec is the first to exercise a preempt path rather than the second.
`research.md` §9 records it as a known untested extension rather than a validated one.

**Prior commitments.** `weapon-model.md` §3 already calls per-shell reload "a cancellable
state machine" and puts `reloadStyle` on the resource — honored, with a stated casing
divergence (`"perShell"`, not the sketched `"per-shell"`; the repo's enum serde
convention is camelCase). `E16--ammo-resource` deferred this classifier here and made
reload duration an effective stat — both honored. `networking.md` §Combat authority keeps
ammo and reload host-authoritative — untouched. `reload-feedback-ui` promised a later
spec would point a real machine at the shipped meter slot with no UI rework — Task 5
keeps it. `movement--state-machine` is the structural precedent Tasks 1–2 follow.
**Stated divergence:** `E10--behavior-state-graph` retired engine-closed FSMs for
*enemies* in favor of authored graphs; weapons take the movement answer instead, because
these transitions are timing and resource legality, not authored behavior.

**Second stated divergence — wieldable-named, weapon-hosted.** `weapon-model.md` §7
invariant 7 requires that inventory, equip, and switch be named for wieldables, not
weapons, and §2 puts identity, inventory, equip, switch, and augment on the *wieldable*
layer. Naming and hosting are two decisions, not one, and they go different ways here.
Naming is **honored**: the state type is `WieldableState`, living in its own module
beside the other component types, so the switching spec inherits a machine already named
for the layer that will own equip and pays no rename. Hosting **diverges**: the field
lives on `WeaponComponent`, because weapon is the only wieldable kind that exists, no
wieldable component exists to host it, and inventing one to carry a single enum would be
speculative generality. That cost is inherited, not avoided — if the switching spec needs
the machine to serve a second wieldable kind, lifting it off `WeaponComponent` is exactly
the reshape the openness requirement otherwise prevents, and `research.md` §9's extension
table does not cover it. Moving a correctly-named type between host components is
mechanical while weapons are the only kind, and gets more expensive per wieldable kind
added. Citations: `research.md` §10.

**Accepted cost — the dev default becomes a known co-op mispredict.** Task 6 points the
dev mod's `defaultWeapon` at the per-shell shotgun. Per-shell reload is host-authoritative
and `ClientWeaponState` models neither ammo nor reload, so whether a predicted shot is
authorized mid-loop turns on the host's magazine count, which the client cannot compute.
The shotgun makes that window the norm rather than the exception: a magazine of 8 refilled
one shell at a time is seconds of loop per reload, against the reference pistol's 12 rounds
and one 500 ms atomic transfer. So the default dev configuration exercises the
predict-then-roll-back path routinely instead of rarely. Accepted: the per-shell loop is
undemoable without switching or pickups unless it is the default, the rollback path is the
shipped `ShotVerdict` one and is correct, and reverting is a one-line content edit.
Detail: `research.md` §12.

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
them. Detail, and what this hands the switching spec: `research.md` §12.

**Alternatives rejected.** The strongest rival is **shipping the equip lifecycle now** —
`Raising` with a `raiseMs`, and with it `Lowering` and `Stowed`. Case for: an
uninterruptible timed state is a genuinely different shape from an interruptible loop, so
shipping one validates the machine against a second consumer; a draw delay is wanted
gameplay for this genre; and pinning transition legality now means the switching spec
cannot re-open it. Rejected. None of the three states has a production consumer this spec
does not manufacture — `raiseMs` would default to `0` and every existing weapon would
collapse it away, leaving the dev shotgun this spec authors as its only driver. The draw
delay is real gameplay, but it is content, and it arrives with the spec that gives it a
second call site. And pinning equip legality is the objection, not the argument: it hands
the switching spec a settled equip contract written by a spec that owns no part of equip,
while the openness requirement already makes the whole lifecycle additive when its real
driver arrives. What is genuinely lost is the live preempt-from-any-state implementation,
named above and recorded in `research.md` §9.

Runner-up alternative: keep two producers and add a second boolean (`fire_wants_cancel`)
— minimal diff, works for this one interrupt, rejected because switching adds a third and
the ordering constraints between three become unwritable. The rest: `research.md` §11.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A weapon is in exactly one state; the fire gate authorizes a shot only from `Idle`; a `ShellLoading` cancel must transition to `Idle` before the shot is authorized, never authorize from `ShellLoading` directly | Task 2 | the state added in Task 4 must extend the legality predicates, not bypass them | AC 2, 6, 7, 17 |
| The machine stays open for extension: new states are arms and rows, never a reshape | Task 2 | Task 4 is the first extension and must land as arms and rows itself | AC 14 |
| Rounds credited by a per-shell loop are never rolled back — not by cancel, not by hot reload | Task 4 | Task 2's hot-reload policy | AC 4, 6, 9 |
| The pawn `AmmoReserve` is debited exactly once per credited round, through `AmmoReserve::take` only | Task 4 | the tick where one step completes and the next starts | AC 3, 6, 13 |
| Hot reload never changes the current state or its remaining time; it refreshes durations and style only | Task 2 | every field Task 2 adds must stay out of `refresh_from_descriptor`'s assignment list — preservation is by omission | AC 9 |
| The local-pawn and host-simulated-remote-pawn vantages run the identical machine | Task 2 | Task 4 adding a path only one caller reaches | AC 10 |
| Tests are independent of dev content: no test's outcome depends on which archetype the dev mod equips as `defaultWeapon` | Task 2 (fixture convention) | every task that adds a test; Task 6's `defaultWeapon` flip | AC 15 |
| No wire field, `WIRE_VERSION`, app-protocol, or replicated-slot-fingerprint change | Task 2 | Tasks 4, 5 | AC 13 |

## Acceptance criteria

- [ ] Authors can set `components.weapon.resource.reloadStyle` to `"magazine"` or
  `"perShell"` in TypeScript and Luau; an absent value parses as `"magazine"`; any other
  value is rejected at deserialize, identically in both runtimes. Generated SDK typedefs
  and `docs/scripting-reference.md` carry the field and both values.
- [ ] A weapon authored `reloadStyle: "magazine"` (or with the field absent) reloads
  exactly as today: one timer of `reloadMs`, one atomic transfer at completion,
  uncancellable by firing, same four reload events with the same `transferred` count.
  The shipped atomic-reload *behavior* assertions pass with no change beyond the
  timer-field rename and an added `state` initializer.
- [ ] A `perShell` reload loads one round per `reloadMs`: after N steps the magazine has
  grown by exactly N and the pawn reserve shrunk by exactly N, one `reload_shell_loaded`
  per step, and exactly one `reload_started` for the whole loop rather than one per step.
  `docs/scripting-reference.md` carries the `reload_shell_loaded` name.
- [ ] The loop ends on its own when the magazine reaches capacity, when the reserve
  reaches zero with the magazine still short, and when a credit `take` returns `0`. All
  three emit one `reload_completed` whose `transferred` is the cumulative total for the
  whole reload, and leave the weapon fire-ready. That cumulative count survives a
  descriptor hot reload mid-loop.
- [ ] A reload press with a full magazine, or an empty reserve, emits the blocked
  outcome and starts no loop — both styles.
- [ ] An otherwise-authorized shot during a `perShell` loop cancels it: the shot resolves
  on that same tick, the in-flight step is discarded with no round credited, previously
  credited rounds stay in the magazine, the reserve is not refunded, one
  `reload_cancelled` fires carrying the cumulative credited count, and `reload_feedback`
  clears so the meter reads inactive from the cancel tick onward. Cancelling at the first
  step (zero credited) and at the last possible step both behave this way.
  `docs/scripting-reference.md` carries the `reload_cancelled` name.
- [ ] A trigger pull during a `perShell` loop that would *not* be authorized anyway —
  cooldown not elapsed, or magazine below `costPerShot` — does not cancel the loop and
  emits no `dry_fire`.
- [ ] Reload can still start while the weapon is cooling; cooldown never blocks a reload
  and reload never resets cooldown.
- [ ] Hot-reloading the descriptor mid-`ShellLoading` and mid-`Reloading` preserves the
  state, remaining time, step total, sub-millisecond carry, cumulative credited count,
  magazine, and input-edge flags, while
  adopting the new `reloadMs` and `reloadStyle` from the next decision point onward. A
  `perShell` → `magazine` flip mid-loop lets the in-flight step credit its round, then
  ends the reload rather than looping.
- [ ] A co-op remote client's pawn, simulated host-side, exhibits every behavior above —
  per-shell steps, cancel-on-fire, reload lockout — identically to the single-player pawn,
  driven through the same command path.
- [ ] A connected client that predicts a shot the host refuses — mid-`magazine` reload, or
  mid-`ShellLoading` with the host magazine still below `costPerShot` — sees it rolled back
  through the existing verdict path: no authorized shot minted, predicted cooldown
  restored, muzzle/hitmarker feedback cleared. The same predicted shot mid-`ShellLoading`
  with the magazine at or above `costPerShot` is authorized instead and cancels the loop.
- [ ] `player.reloadActive` is true for the entire duration of a reload of either style,
  including across step boundaries, and false at every other time except the shipped
  one-frame `ReloadFeedback::Completed` endpoint frame, which keeps today's `(1.0, true)`
  sample. `player.reloadProgress` ramps `0 → 1` once per reload for `magazine` and once per shell
  for `perShell`. `player.ammo` increments once per credited shell. The owner-private
  projection publishes the same values to a remote client's owner.
- [ ] No wire message, `WIRE_VERSION`, app-protocol constant, replicated-slot schema
  entry, or state-slot fingerprint input changes (review/grep gate). No switching,
  inventory, pickup, or multi-pellet behavior is built, and no `Raising`, `Lowering`, or
  `Stowed` variant and no equip-timing descriptor field are added (review/grep gate). The
  pawn `AmmoReserve` is mutated only through `credit` / `take`, and only from the weapon
  stage (review/grep gate). No new `unsafe` (review/grep gate).
- [ ] Openness is demonstrated, not asserted: adding a placeholder timed `WieldableState`
  variant produces compile errors only at the transition function and the three legality
  predicates — no other production site — and needs no new timer field and no change to
  the outcome-to-event mapping. This is the spec's sole extension-openness guarantee, so
  the check is run and its output recorded; the throwaway branch proving it is the
  verification and is not merged.
- [ ] Every test covering weapon state or reload style builds its own
  weapon descriptor or component fixture; no test reads the dev mod's `defaultWeapon` or
  asserts a value that holds only while a particular archetype is the dev default.
  Setting `content/dev/scripts/player.ts`'s `defaultWeapon` to either registered
  archetype leaves the whole suite green, checked both ways.
- [ ] The weapon-stage split is behavior-preserving: the existing suite passes with no
  changes beyond import paths and module placement, and no caller outside the sim module
  changes — the netcode hit-acceptance path still reaches the authorized-impact entry
  point at its current path.
- [ ] A weapon ticked with no owning pawn still advances timers, resolves expiries, and
  gates fire, but refuses every reload silently, emitting no delivery — it can fire and
  cool, it cannot reload or credit a shell.
- [ ] Launching the dev level equips a per-shell weapon, with a viewmodel and third-person
  hand prop rendering as the pistol's do: firing to empty, holding reload,
  watching the magazine climb one round at a time, and firing mid-loop to cancel all work
  from dev content alone, with both classifier values present across the two reference
  weapon archetypes. The combat-demo kill payout and pickup volume credit the equipped
  weapon's ammo type, so both grant walkthroughs still show a reserve the player can load.

## Tasks

### Task 1: Extract the weapon stage out of `sim/mod.rs`

Behavior-preserving split, no functional change. `crates/postretro/src/sim/mod.rs`
carries 1446 production lines and this plan extends its weapon orchestration. Move into
a new `crates/postretro/src/sim/weapon_stage.rs`: `weapon_fire_command`,
`normalize_aim_direction`, `run_remote_weapon_commands`, `run_local_weapon_command`,
`apply_weapon_impact_damage`, `apply_authorized_weapon_impact_damage`,
`apply_weapon_impact_damage_with_source`, and the test-only `deliver_reload_to_weapon`.
Keep `run_death_sweep` in `sim/mod.rs` — it sits inside that address range but is the
death stage. Exactly one function has a consumer outside the sim module:
`apply_authorized_weapon_impact_damage`, called by fully-qualified path
(`crate::sim::apply_authorized_weapon_impact_damage`) from
`crates/postretro/src/netcode/mod.rs`. Re-export it from `sim/mod.rs` so that path still
resolves; the other seven are sim-internal and need no re-export. `sim/mod.rs`'s existing
`pub(crate) use reload::{…}` block keeps its current shape and location — `reload.rs`
stays a sibling module of `weapon_stage.rs` under `sim/`, and Task 2's fusion consumes it
in place rather than relocating it.

Test boundary: move every test whose subject is one of the eight moved functions, plus
every caller of `deliver_reload_to_weapon`. A test spanning both stages stays with the
stage it *ends* in — the one asserting `apply_authorized_weapon_impact_damage` then
`run_death_sweep` is a death-sweep test and stays in `sim/mod.rs`, calling the moved
entry point through its re-export.

The tick-order call sites inside `simulate_tick_with_presentation_aim` stay put and call
into the new module; `simulate_tick` is a thin forwarding wrapper and is untouched.

### Task 2: `WieldableState` + the fused machine tick (thin slice)

Narrow vertical slice through every seam, reproducing today's exact behavior with no new
authored surface and no new production states. Define `WieldableState` in a new
`crates/entities/src/components/wieldable_state.rs`, declared in the component barrel
(`crates/entities/src/components/mod.rs`) beside its siblings. The name is deliberate:
`weapon-model.md` §7 invariant 7 requires equip and switch machinery be named for
wieldables, and the switching spec will extend this machine with equip states. Hosting it
on the weapon kind is equally deliberate while weapon is the only wieldable kind that
exists — do not invent a wieldable component for it. Derive at least
`Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize`, matching
`WeaponComponent`'s derives and its sibling enums.

Add to `WeaponComponent`
(`crates/entities/src/components/weapon.rs`) a `state: WieldableState` field
(`#[serde(default)]`, `Default = Idle`) and replace `reload_remaining_ms`,
`reload_total_ms`, and `reload_elapsed_sub_ms` with one generalized timed-state triple
owned by the machine: `state_remaining_ms: u32`, `state_total_ms: u32`,
`state_elapsed_sub_ms: f64` — the fractional carry that prevents per-tick rounding bias.
The names are state-agnostic on purpose; a reload-specific spelling is exactly what rule
(c) below exists to avoid, and these three must agree across roughly ninety references.
Add one more field the per-shell loop needs and nothing derives: `reload_credited: u32`,
the cumulative rounds credited by the current reload, reset on entry to a reload. It
cannot be recovered from `magazine`, which also moves on fire and can be at any level when
a loop is entered. Define the three variants this spec ships (`Idle`, `Reloading`,
`ShellLoading`) so Task 4 adds transitions rather than variants; only `Idle` and
`Reloading` are reachable in production after this task. `from_descriptor` materializes
`Idle`. Update the `WeaponComponent` literals across the workspace that the field changes
break — the compiler enumerates them.

The timer rename reaches seven files: `crates/entities/src/components/weapon.rs`,
`crates/postretro/src/sim/mod.rs`, `crates/postretro/src/sim/reload.rs`,
`crates/postretro/src/weapon/mod.rs`, `crates/postretro/src/netcode/state_slots.rs`,
`crates/postretro/src/netcode/lifecycle.rs`, and
`crates/postretro/src/scripting/systems/ui_proxy.rs`. Several shipped tests fabricate
mid-reload state by writing the timer directly (`reload_remaining_ms = 250` and
similar). Those must also set `state`, because `reload_status()` now derives from state
plus timers — renamed and left alone they still compile and assert the wrong thing.

Fuse the two producers. `sim/reload.rs::tick` and the private
`weapon/mod.rs::apply_weapon_fire_state` become one machine tick with a single ordered
decision per fixed tick: advance the state timer, resolve any expiry, then evaluate the
fire and reload intents against the resulting state. Its home is a new function in
`sim/weapon_stage.rs` — Task 1's module — called from both `run_local_weapon_command` and
`run_remote_weapon_commands`, ahead of `weapon::tick_resolved_component` /
`weapon::tick_state_only_component`. No callee below those two can host it:
`apply_weapon_fire_state` takes neither a registry nor a pawn id, and
`tick_state_only_component` takes no registry at all, so the `AmmoReserve` transfer has
nowhere to happen there. A placement inside `tick_resolved_component` would additionally
skip the remote-pawn path entirely. The
`reload_started_this_tick` boolean and its two `deliveries.iter().any(...)` computations
at the call sites disappear; the machine returns its own outcome list plus the fire
authorization. Keep `ReloadDelivery`, `ReloadOutcome`, and every existing event name and
payload byte-identical.

That leaves the two `weapon::tick_*` entry points with different fates.
`tick_state_only_component` did nothing but call `apply_weapon_fire_state`, so it and its
`#[cfg(test)] tick_state_only` wrapper are deleted; `run_remote_weapon_commands` matches
the machine's `WeaponFireAuthorization` directly, keeping its existing
`Empty` → `weapon_events.push("dry_fire")` arm unchanged. `tick_resolved_component`
survives as pure shot resolution: it drops `tick_dt` and `reload_started_this_tick`, takes
the machine's `WeaponFireAuthorization` as an argument, and keeps `command` for aim origin
and direction. Its three-arm match stays exactly where it is, so
`Accepted` → `fire_hitscan` and `Empty` → `WeaponFireEvents { dry_fire: true }` are
unmoved. Its `#[cfg(test)] tick_resolved` wrapper takes the same argument swap.

Build it **open for extension** — the equip lifecycle (`Raising`, `Lowering`, `Stowed`)
and a switch interrupt land in a later spec and must not force a reshape. Five rules, all
load-bearing: (a) one transition function keyed by (current state, event), so a new state
adds arms, not a dispatch shape, and so a later entry point legal from *every* source
state is arms too; (b) fire and reload legality expressed as three `WieldableState`
predicates written per variant — `allows_fire()`, `allows_reload()`, and
`is_reload_activity()`, the last of which `reload_status()` calls so the meter's state
question stays in the predicate module instead of becoming a fourth match site — not
inline `state != Idle` tests scattered through the gate; (c) the
timed triple stays state-agnostic, so a new timed state adds no field; (d) no `_` wildcard
arm over `WieldableState` anywhere in production, so a new variant is a compile error at
every site that must decide about it; (e) outcome-to-event mapping stays name-driven
through `ReloadOutcome::event_name`, so a new endpoint pair needs no new plumbing.

Two plumbing facts the fusion turns up, both settled here rather than by the implementer.
First, **the registry borrow**: there is nothing to invert. Both call sites already hold
`RefMut<EntityRegistry>` and pass an immutable reborrow into `tick_resolved_component`.
Run the machine under that mutable borrow, producing a fire authorization and an outcome
list, then reborrow immutably for hitscan resolution exactly as the code already does. Do
not widen the resolution signature to `&mut`; the renderer-adjacent read path stays a
read. The single real constraint is the one stated in Direction — the machine must not run
*inside* `tick_resolved_component`. Second, **a weapon with no pawn**:
`run_local_weapon_command` takes `pawn: Option<EntityId>` and today skips reload entirely
when it is `None` (fly-camera and headless harnesses). Keep that behavior. The machine
still advances timers, resolves expiries, and gates fire, but any transition that would
touch the reserve is refused **silently, emitting no delivery** — `ReloadDelivery` carries
a non-optional `pawn`, so a blocked outcome is unconstructible without one. A pawnless
weapon fires and cools; it cannot reload.

Define the **hot-reload policy** the later tasks depend on. `refresh_from_descriptor` has
no preserved set to extend — it assigns the authored tuning and nothing else, and
preservation is by omission, documented in a trailing comment. So: leave `state`, the
timed triple, and `reload_credited` out of the assignment list, extend that trailing
comment to name them alongside the cooldown, magazine, and input edges it already
describes, and extend the existing preserve-all-live-state test to assert them. Durations
and classifiers refresh and take effect at the next decision point, matching the existing
precedent that reload completion re-reads `effective()`.
Rewrite `reload_status()` to derive `(progress, active)` from `is_reload_activity()` plus
the timers rather than from `reload_remaining_ms > 0`, keeping today's outputs identical
for `Idle` and `Reloading` including the one-frame `ReloadFeedback` endpoints.

Establish the fixture convention every later task's tests inherit: machine and component
tests construct their own `WeaponDescriptor` / `WeaponComponent` values in-test. No test
may reach for the dev mod's `defaultWeapon` or encode an assumption about which archetype
dev content equips, so Task 6's `defaultWeapon` flip — and any future one — is a content
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
`crates/postretro/src/scripting/primitives/mod.rs`, and beside that field registration add
a `register_enum("ReloadStyle")` declaration with a doc line per variant, matching the
sibling `FireMode` / `ResolutionMode` registrations. Without it the field's `"ReloadStyle"`
type name is a dangling reference in the emitted typedefs and the drift test fails
opaquely. Regenerate the committed
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
blocked-empty, neither starts a loop. Entry zeroes `reload_credited`, and emits exactly
one `reload_started` for the whole reload under **both** styles — a per-shell loop is one
reload, not N of them, so a mod hooking `reload_started` gets one firing per press
regardless of style. `ReloadFeedback`'s endpoints go step-scoped in Task 5; that is a
meter concern and does not change this firing count.

On each step expiry, credit exactly one round via `AmmoReserve::take(type, 1)` — never by
indexing the pool — add the returned amount to the magazine and to `reload_credited`, and
emit `reload_shell_loaded`. Then evaluate the loop-continue predicate against live state:
continue when the magazine is below the effective capacity **and** the reserve still has
rounds; otherwise end the reload, emit `reload_completed` carrying `reload_credited` as
the cumulative transferred count for the whole reload, and return to `Idle`. `take`
returning `0` — the reserve emptied between check and credit — ends the reload with no
shell event but the same `reload_completed`, cumulative count included, so all three
endings agree on their endpoint.

Continuing restarts the step timer within the same tick so there is no idle gap between
shells, and the restart is where the sub-millisecond carry needs care. Today's timer
advance zeroes both the timer and the carry at expiry, and validates the carry as
`>= 0.0 && < 1.0` — it is a sub-millisecond fraction by contract. At a step restart the
overshoot is bounded by the tick, roughly 16.67 ms at 60 Hz, not by 1 ms; writing it into
the carry field would be silently discarded by that guard and the step cadence would drift.
So: the expiry path returns the whole overshoot as `f64` milliseconds, the restart
subtracts its integer-millisecond part from the new step's remaining time and stores only
the fractional part in the carry, and the restart loops when the overshoot exceeds a full
step — crediting several shells in one tick rather than losing the excess.

Add the cancel edge, the reason Task 2's fusion exists. When the fire intent evaluates to
authorized against the current gate on this tick — `can_fire` included, since the remote
path repurposes it to mean "pawn has a `NetworkId`" and a remote pawn with no `shot_id`
would otherwise cancel its own loop every tick while firing nothing — `ShellLoading` does
not reject the shot: it cancels, discards the in-flight step with no round credited and no
reserve debit, emits `reload_cancelled` carrying `reload_credited`, transitions to `Idle`,
and lets the shot resolve on that same tick. Name that condition once and reuse it, rather
than restating the gate's terms, so the two cannot drift apart.

A trigger pull that would not be authorized anyway leaves the loop running and emits no
`dry_fire`. The warrant is parity with `Reloading`, which already rejects before the
magazine check and so already emits none; trigger-hold spam is not the reason, having been
fixed by setting the cooldown on the `Empty` path. The suppressed path therefore leaves
cooldown untouched, matching `Rejected` rather than `Empty`. Reload rising edges
during `ShellLoading` stay no-ops, matching the shipped rising-edge dedup. `Reloading`
keeps refusing the shot silently — the atomic style stays uncancellable by fire, per the
shipped contract. Register the two new outcome variants on `ReloadOutcome::event_name` so
both events reach the existing event drain, and document `reload_shell_loaded` and
`reload_cancelled` in `docs/scripting-reference.md`'s `components.weapon` section. No
reload event name is documented there today, so list the four shipped names with them —
two of six documented reads as an omission.

### Task 5: Reload-meter and projection semantics under per-shell

`player.reloadProgress` and `player.reloadActive` are shipped owner-private slots whose
producers are `weapon_hud_values` in
`crates/postretro/src/scripting/systems/ui_proxy.rs` and `AmmoSlotProjection::for_pawn` in
`crates/postretro/src/netcode/state_slots.rs`, both reading
`WeaponComponent::reload_status()`. Redefine that accessor so `active` is true for the
whole of `Reloading` and the whole of `ShellLoading` — including the tick where one step
ends and the next begins, which must not blink false — and `progress` is the **current
step's** ramp. Under `magazine` that is unchanged; under `perShell` it is a repeating fill,
one per shell, the conventional shotgun readout. Reach that decision through Task 2's
`WieldableState::is_reload_activity()` predicate, not a fresh `match` on the enum: rule (d)
forbids a `_` arm, and a fourth match site is exactly what AC 14's guarantee is counting.

Extend the `ReloadFeedback` one-frame
endpoint lifecycle to mark step boundaries rather than whole-reload boundaries so a
sub-tick step still contributes its `0.0` and `1.0` samples, keeping the single-step
`magazine` case bit-identical; when a step's end and the next step's start land in the same
tick the completion sample wins, matching the existing precedent for a reload shorter than
one tick. This is a meter lifecycle only — it is independent of Task 4's one
`reload_started` per whole reload, and neither constrains the other. A cancel clears
`reload_feedback` outright: left at `Some(Started)` it would report `(0.0, true)` on an
`Idle` weapon.

Neither producer needs a new input — both already reach the component
(`research.md` §3, V4 warrant) — and no slot is added, so the replicated schema and its
content-derived fingerprint are untouched. No primitive registration changes either, so
unlike Task 3 this task regenerates no SDK typedefs. Update the slot descriptions in
`docs/scripting-reference.md` and the engine-state catalog comments to state the
step-scoped meaning.

### Task 6: Dev content, docs, and the cross-vantage test suite

Author `content/dev/scripts/reference-shotgun.ts` — `canonicalName "reference_shotgun"`,
`fireMode: "semi"`, `resolution: "hitscan"`, `damage: 12` (matching the reference pistol so
the `target_dummy` three-shot demo math in `content/dev/scripts/target-dummy.ts` stays
correct), a slow `fireRateMs`, `thirdPersonModel` and `viewmodel` both set to
`models/smg/model.gltf` as the pistol sets them — omitting either ships the dev default
with no viewmodel and no hand prop, a visible regression in the launch AC 18 gates on — an
ammo resource with `reloadStyle: "perShell"`, a magazine
of 8, its own ammo `type`, a starting reserve, and a per-shell `reloadMs` — and register it
from `content/dev/start-script.ts` beside the pistol. Point
`content/dev/scripts/player.ts`'s `defaultWeapon` at it, replacing `reference_pistol`, so
the per-shell loop is demoable without switching or pickups. That flip is decided, not
proposed; it is the only production consumer of the per-shell path in this spec, and it is
one line to revert. It also makes a co-op mispredict the default dev configuration —
accepted, and argued in the plan's Direction section, so do not re-litigate it here or
add prediction machinery to compensate. Add an explicit `reloadStyle: "magazine"` to
`content/dev/scripts/reference-pistol.ts` so both classifier values appear in dev content;
the pistol archetype stays registered.

The flip strands five content sites, and the shotgun keeps its own ammo `type` — a shared
pool would make the type meaningless — so the content moves rather than the descriptor.
Retarget both ammo grants to the shotgun's ammo type: `content/dev/scripts/combat-demo-reaction.ts`'s
24-round `ammoPickup` trigger reaction, and `content/dev/scripts/combat-lifecycle.ts`'s
8-round `dev:ammo-on-kill` payout. Both currently credit `bullets.light`, which no equipped
weapon would draw from. Then update the three prose sites that name the pistol as the
equipped weapon: `content/dev/scripts/target-dummy.ts`'s header comment,
`content/dev/scripts/combat-lifecycle.ts`'s finisher-overshoot comment, and
`content/dev/maps/combat-demo.README.md` — the last of which also names the grants' ammo
type and assumes the pistol's `fireRateMs: 180` cadence, so its walkthrough needs the
shotgun's slower cadence. The three-shot dummy math survives unchanged because the shotgun
matches the pistol's `damage: 12`.

Add the test suite spanning the four vantages in `research.md` §3. Every fixture is
constructed in-test — no test reads dev content or assumes which archetype is the dev
`defaultWeapon`; verify by running the suite with `defaultWeapon` set to each of the two
archetypes in turn. Descriptor layer: `reloadStyle` parses, defaults, and rejects
identically in the QuickJS and Luau runtimes, and appears in the generated SDK typedefs.
Machine layer: every transition in the `research.md` §2 diagram, loop-exit on full and on
empty reserve, cancel at the first and last step, a would-be-unauthorized trigger pull not
cancelling, a step duration that is not a tick multiple not drifting over many steps, and
reload-while-cooling still starting. Hot-reload layer: state, timers, cumulative credited
count, magazine, and edge
flags preserved mid-`Reloading` and mid-`ShellLoading`; a mid-loop style flip ending the
reload after the in-flight step; a mid-loop refresh leaving the eventual
`reload_completed`'s `transferred` unchanged. Vantage layer — AC 10 claims every behavior
above holds for the host-simulated remote pawn, so the magazine/reserve equivalence test
alone does not discharge it: that pawn reaching
the same magazine and reserve counts as the local pawn from the same command sequence,
cancelling a `ShellLoading` loop on an authorized shot, and observing the `Reloading`
lockout. Then the prediction seam: a
connected client's predicted shot during a host-side `magazine` reload minting no
authorized shot and rolling back through the verdict path, and the same outcome for a shot
predicted mid-`ShellLoading` while the host magazine is still below `costPerShot`. HUD
layer: `reloadActive` not blinking across a step boundary, `reloadProgress`
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
**Phase 5 (sequential):** Task 5 — meter and projection semantics, over the state set Task
4 completes.
**Phase 6 (sequential):** Task 6 — dev content, docs, and the cross-vantage suite, which
assert everything above.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Reload style classifier | `AmmoResource::reload_style`, `EffectiveAmmoStats` | `"reloadStyle"` | `resource.reloadStyle` | same | n/a |
| Atomic style value | `ReloadStyle::Magazine` (default) | `"magazine"` | `"magazine"` | same | n/a |
| Per-shell style value | `ReloadStyle::PerShell` | `"perShell"` | `"perShell"` | same | n/a |
| Step duration (reused) | `AmmoResource::reload_ms` | `"reloadMs"` | `resource.reloadMs` | same | n/a |
| Wieldable state | `WeaponComponent::state` (`WieldableState`) | not replicated, not authored | n/a | n/a | n/a |
| Timed-state triple (rename) | `WeaponComponent::state_remaining_ms` / `state_total_ms` / `state_elapsed_sub_ms` | `#[serde(default)]`, component-local only | n/a | n/a | n/a |
| Cumulative credited count | `WeaponComponent::reload_credited` | `#[serde(default)]`, component-local only | n/a | n/a | n/a |
| Shell credited | `ReloadOutcome` variant → `event_name` | `"reload_shell_loaded"` | reaction / audio consumer | same | n/a |
| Reload cancelled | `ReloadOutcome` variant → `event_name` | `"reload_cancelled"` | reaction / audio consumer | same | n/a |
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
      thirdPersonModel: "models/smg/model.gltf",
      viewmodel: "models/smg/model.gltf",
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
times the whole reload rather than one step, identical to today.
