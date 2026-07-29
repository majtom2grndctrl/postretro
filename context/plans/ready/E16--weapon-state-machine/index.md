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
  carry) replacing the three reload-specific timer fields, plus a `reload_credited`
  counter for the rounds the current reload has loaded.
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
- Cancellation: an otherwise-authorized shot cancels a `ShellLoading` loop that did not
  start on this tick, forfeits only the in-flight step, and fires on that same tick.
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

**Placement.** The machine is one phase both sim call sites run, above the two
`weapon::tick_*` entry points as they stand today rather than inside either —
it subsumes the gate decision those two currently delegate to the private
`apply_weapon_fire_state`, and it must run under the mutable registry borrow the
`AmmoReserve` transfer needs, which no callee below them holds (Task 2 names its home
and pins the borrow discipline). `tick_resolved_component` is the tempting home, since firing visibly
happens there, but it would silently skip the remote-pawn vantage, which runs
`tick_state_only_component`. The axis is *shared-phase vs. per-caller*, not
engine-vs-mod: `reloadStyle` is authored data, the machine reading it is engine floor,
matching the shipped `fireMode` / `resolution` split. Warrant: `research.md` §3.

**Open for extension.** This spec ships three states and defers the whole equip lifecycle
— `Raising`, `Lowering`, `Stowed` — plus the switch interrupt. That is sound only if
adding them later is arms-and-rows work, so openness is a first-class requirement here,
not a hope: one transition function keyed by (state, event); legality as exhaustive
per-variant predicates; a state-agnostic timed triple; and no `_` wildcard arms over
`WieldableState` in production. Task 2 builds it, Task 6 demonstrates it against AC 14,
`research.md` §9 maps each future extension to the piece that absorbs it.
`movement--cross-cutting-policies` D7 settled the same question for movement — per-state
live data owned through one uniform convention, so adding a state never widens the
dispatch — and the state-agnostic timed triple is that answer applied here. With this,
the deferred states cost three variants and their rows when their driver arrives.

One piece of that openness ships **unexercised**, and it is the honest cost of deferring
equip. A preempting entry point — one legal from every state, forfeiting the in-flight
timed step while keeping credited rounds — is the shape `begin_lower` and the switch
interrupt need, and no consumer in this spec reaches it. The transition function's
(state, event) keying admits it as new arms, but this spec neither implements nor tests
it. Task 4's cancel edge preempts from one state; the switching spec is the first to
exercise preemption from *every* state rather than the second.
`research.md` §9 records it as a known untested extension rather than a validated one.

**Prior commitments.** `context/research/weapon-model.md` §4 already calls per-shell reload
"a cancellable state machine", and §3 puts `reloadStyle` on the resource — honored, with a stated casing
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
| A weapon is in exactly one state; the fire gate authorizes a shot only from `Idle` **and only when no reload started on this tick**; a `ShellLoading` cancel must transition to `Idle` before the shot is authorized, never authorize from `ShellLoading` directly | Task 2 | Task 4's cancel edge must reach `Idle` ahead of the gate rather than relax `allows_fire` for `ShellLoading` | AC 2, 6, 7, 17 behaviorally; AC 13's grep gate for the code shape, since an implementation returning `true` from `allows_fire()` for `ShellLoading` while still emitting `reload_cancelled` passes every behavioral check |
| The machine stays open for extension: new states are arms and rows, never a reshape | Task 2 | Task 2 ships the arms and rows; Task 4 fills the `ShellLoading` bodies without touching a predicate row; Task 6 runs the demonstration and records its output | AC 14 |
| Rounds credited by a per-shell loop are never rolled back — not by cancel, not by hot reload | Task 4 | Task 2's hot-reload policy; the tick where a step expiry and a cancel land together | AC 4, 6, 9; the ordering matrix |
| The pawn `AmmoReserve` is debited exactly once per credited round, through `AmmoReserve::take` only | Task 4 | the tick where one step completes and the next starts; the working reserve copy a multi-credit tick drains | AC 3, 6, 13; the ordering matrix |
| A reload level held across a pawnless window still produces a rising edge on the tick the pawn returns | Task 2 | the fusion runs the machine unconditionally, where `reload::tick` was skipped wholesale | AC 17; the ordering matrix |
| Hot reload never changes the current state or its remaining time; it refreshes durations and style only | Task 2 | every field Task 2 adds must stay out of `refresh_from_descriptor`'s assignment list — preservation is by omission | AC 9 |
| The local-pawn and host-simulated-remote-pawn vantages run the identical machine | Task 2 | Task 4 adding a path only one caller reaches | AC 10 |
| Tests are independent of dev content: no test's outcome depends on which archetype the dev mod equips as `defaultWeapon` | Task 2 (fixture convention) | every task that adds a test; Task 6's `defaultWeapon` flip | AC 15 |
| No wire field, `WIRE_VERSION`, app-protocol, or replicated-slot-fingerprint change | Task 2 | Task 3 above all — the only task adding descriptor- and wire-facing serde fields and regenerating the committed SDK typedef fixtures; then Tasks 4, 5 | AC 13 |

## Acceptance criteria

- [ ] Authors can set `components.weapon.resource.reloadStyle` to `"magazine"` or
  `"perShell"` in TypeScript and Luau; an absent value parses as `"magazine"`; any other
  value is rejected at deserialize, identically in both runtimes. Generated SDK typedefs
  and `docs/scripting-reference.md` carry the field and both values, and that file's
  `resource` row and its trailing `reloadMs` paragraph both state that `reloadMs` times one
  reload *step* — the whole reload under `magazine`, one shell under `perShell`. Both values appear in
  dev content: `content/dev/scripts/reference-pistol.ts` authors `"magazine"` explicitly
  and `content/dev/scripts/reference-shotgun.ts` authors `"perShell"` — a content grep, not
  a launch observation, and authored by Task 6 rather than by the task that adds the field.
- [ ] A weapon authored `reloadStyle: "magazine"` (or with the field absent) reloads
  exactly as today: one timer of `reloadMs`, one atomic transfer at completion,
  uncancellable by firing, same four reload events with the same `transferred` count.
  The shipped atomic-reload *behavior* assertions pass with no change beyond the
  timer-field rename, an added `state` initializer, the reload test harness re-pointed at
  the machine entry point, and the weapon-module fire tests re-pointed at it too (their
  subject is the gate, which moves). Four tests pin the fusion's riskiest behaviors and
  must pass unmodified except for those mechanical edits:
  `immediate_local_reload_still_blocks_fire_for_start_tick`,
  `immediate_remote_reload_still_blocks_fire_for_start_tick`,
  `local_reload_completion_refills_before_same_tick_fire`, and
  `reload_start_tick_advances_and_can_complete_immediately`. No fire-gate test is dropped
  in the fusion: every `weapon/mod.rs` test whose assertions are on gate outputs exists by
  name in `sim/weapon_stage.rs`'s test module afterwards, and no test retained in
  `weapon/mod.rs` asserts on a `WeaponComponent`'s `cooldown_remaining_ms` or `magazine`, or
  on a `WeaponFireAuthorization` returned by a `weapon::tick*` call (scoped review gate, not
  a grep). The `ClientWeaponState` prediction tests are exempt and stay put:
  `ClientWeaponState` declares its own `cooldown_remaining_ms`, so the field name alone
  separates nothing, and the retained resolution tests carry the literal
  `WeaponFireAuthorization::Accepted` in their text by construction.
- [ ] A `perShell` reload loads one round per `reloadMs`: after N steps the magazine has
  grown by exactly N and the pawn reserve shrunk by exactly N, one `reload_shell_loaded`
  per step, and exactly one `reload_started` for the whole loop rather than one per step.
  `docs/scripting-reference.md` carries the `reload_shell_loaded` name beside the four
  shipped ones — `reload_started`, `reload_completed`, `reload_blocked_full`,
  `reload_blocked_empty` — none of which that file names today.
- [ ] The loop ends on its own when the magazine reaches capacity, when the reserve
  reaches zero with the magazine still short, when a credit `take` returns `0`, when a
  mid-loop hot reload flips the effective `reloadStyle` to `magazine`, and when a hot reload
  has removed the ammo block so an expiry has nothing to credit and no style to read (AC 9
  owns the last two). All five emit one `reload_completed` whose `transferred` is the
  cumulative total for the whole reload, and leave the weapon fire-ready. That cumulative count survives a
  descriptor hot reload mid-loop. The `take`→`0` ending is unreachable from ordinary play —
  the loop-continue predicate already checked the reserve — so it is exercised from a
  fabricated `ShellLoading` fixture whose reserve is zeroed between check and credit. It is
  a guard, not dead code. A last ending is silent: an expiry reached with no owning pawn
  returns to `Idle` crediting nothing and emitting nothing, because `ReloadDelivery` carries
  a non-optional `pawn` and no outcome is constructible without one. A `reload_started`
  therefore has no terminator across a pawn loss, and mods must treat the loss as an
  implicit one. Every transition to `Idle`, this one included, zeroes `reload_credited`.
- [ ] A reload press from `Idle` with a full magazine, or an empty reserve, emits the
  blocked outcome and starts no loop — both styles, for a weapon with an owning pawn. (A
  pawnless weapon emits nothing at all: AC 17.) A reload edge arriving during an in-flight
  reload of either style is a silent no-op instead — no blocked outcome, no second loop —
  matching the shipped rising-edge dedup; matrix row 19 pins it.
- [ ] An otherwise-authorized shot during a `perShell` loop that started on an earlier tick
  cancels it: the shot resolves on that same tick, the in-flight step is forfeited with no
  round credited for it,
  previously credited rounds stay in the magazine, the reserve is not refunded, one
  `reload_cancelled` fires carrying the cumulative credited count, `reload_press_consumed`
  clears, and `reload_feedback` clears so the meter reads inactive from the cancel tick
  onward. The one exception to that clear is a `Completed` marker written earlier in the
  same tick, which survives it, so the meter's last sample is the shell that landed. The
  cumulative count rides on the Rust `ReloadOutcome` variant and is asserted through
  `TickEvents.reload_deliveries`; the script-visible event carries the name only, since the
  drain calls `fire_named_event(delivery.outcome.event_name(), …)` with no payload.
  Cancelling at the first step (zero credited) and cancelling while the loop's final step is
  in flight — the fire arriving on a tick before that step expires — both behave this way.
  A step that expires on the cancel tick resolves first, because expiry precedes fire
  intent: its shell is credited, counted in `reload_credited`, and included in the count
  `reload_cancelled` carries; only the restarted step is forfeited. When that same-tick
  expiry fills the magazine, completion wins outright — `reload_completed`, `Idle`, no
  `reload_cancelled` — and the shot is authorized by the ordinary gate rather than by a
  cancel. A shot on the tick the loop itself starts is refused instead, by the
  started-this-tick latch: no `reload_cancelled`, no `dry_fire`, identical under both
  styles. A shot that becomes legal only because the cooldown reached zero within the same
  tick does cancel and fire on that tick. A held `Semi` press cancels and fires exactly
  once — on the following tick it neither cancels again nor fires, because
  `shoot_press_consumed` is still set. Because `reload_press_consumed` clears, a reload
  level still held through any cancel produces a rising edge on the following tick and
  restarts the loop.
  `docs/scripting-reference.md` carries the `reload_cancelled` name.
- [ ] A trigger pull during a `perShell` loop that would *not* be authorized anyway —
  cooldown not elapsed, or magazine below `costPerShot` — does not cancel the loop, emits
  no `dry_fire`, and leaves `cooldown_remaining_ms` untouched beyond its ordinary per-tick
  decrement: the `Empty` path's cooldown reset does not run.
- [ ] Reload can still start while the weapon is cooling; cooldown never blocks a reload
  and reload never resets cooldown.
- [ ] Hot-reloading the descriptor mid-`ShellLoading` and mid-`Reloading` preserves the
  state, remaining time, step total, sub-millisecond carry, cumulative credited count,
  magazine, and input-edge flags, while
  adopting the new `reloadMs` and `reloadStyle` from the next decision point onward. A
  `perShell` → `magazine` flip mid-loop lets the in-flight step credit its round, then
  ends the reload rather than looping; the reverse flip, `magazine` → `perShell`
  mid-`Reloading`, lets the in-flight timer complete atomically and starts no loop. Both
  directions land in one refresh — no partial state, no lost credit. A refresh that removes
  the ammo block entirely is the same shape one step further: with no ammo there is no type
  to credit and no style to read,
  so the next expiry credits nothing and ends the reload with `reload_completed` carrying the
  cumulative count, both styles.
- [ ] A co-op remote client's pawn, simulated host-side, matches the single-player pawn on
  four points, driven through the same command path: per-shell steps credit one round
  each; an authorized shot cancels a `ShellLoading` loop; a `magazine` reload locks fire
  out; and an identical command sequence leaves the magazine and the pawn reserve at the
  same counts as the local vantage reaches. Nothing beyond this list is claimed for the
  remote vantage.
- [ ] A connected client that predicts a shot the host refuses — mid-`magazine` reload, or
  mid-`ShellLoading` with the host magazine still below `costPerShot` — sees it rolled back
  through the existing verdict path: no authorized shot minted, muzzle/hitmarker feedback
  cleared, and the predicted cooldown restored **unless a fresher authoritative cooldown
  has since landed**, per `ClientPredictedShots::apply_verdict`'s
  `cooldown_authority_generation` guard. The same predicted shot mid-`ShellLoading`
  with the magazine at or above `costPerShot` is authorized instead and cancels the loop.
- [ ] The reload meter carries two guarantees at two cadences, because the machine ticks at
  60 Hz while both consumers publish once per rendered frame, after the whole catch-up tick
  loop.

  **Machine-level, per tick, assertable directly against `reload_status()`:**
  `active` is true for the entire duration of a reload of either style, including across
  step boundaries, and false at every other time except endpoint frames, which keep today's
  two samples: `(0.0, true)` for a `Started` marker, `(1.0, true)` for a `Completed` one.
  Under `perShell` a `Completed` endpoint now lands at every step
  boundary, not only at the end of the reload. `progress` ramps `0 → 1` once per reload for
  `magazine`. For `perShell` the sample sequence is: an exact `0.0` on the start tick (the
  `Started` endpoint), a rising ramp to the first step's expiry, `1.0` on each step-boundary
  tick (the `Completed` endpoint, which wins over the next step's start in the same tick),
  then a ramp rising from just above `0` to the next boundary — so steps 2..N publish no
  exact `0.0`, and a step shorter than one tick produces endpoint samples only.

  **Publication-level, weaker, for `player.reloadProgress` / `player.reloadActive` /
  `player.ammo` at the HUD and at the owner-private projection:** every endpoint the machine
  produces is published at least once, quantized to the publishing cadence, and may arrive up
  to one snapshot interval after the tick it describes. `player.ammo` follows the live
  magazine, so it rises by however many shells were credited since the last publication —
  one per tick at ordinary step durations, more when several steps complete inside one tick.
  The projection carries every endpoint to a remote client's owner under that same
  quantization; it does not carry the per-tick sample sequence.

  The bound on "at least once" is real and stated rather than hidden: `reload_feedback` holds
  one endpoint, and `clear_all_reload_feedback` runs only on
  `host_owner_state_projection_due()` — every second tick. Two step boundaries inside one
  snapshot window therefore coalesce and one endpoint never reaches a remote owner. Reaching
  it needs an effective `reloadMs` at or below roughly one snapshot interval (two fixed
  ticks, ~33 ms at 60 Hz). No shipped or dev content reaches it; authored content is expected
  to. The fix seam is the marker: it would have to carry a count of endpoints rather than a
  single endpoint. This spec does not build it.

  `docs/scripting-reference.md` documents both slots, where it documents neither today:
  `player.reloadProgress` as the current step's progress, `player.reloadActive` as true
  across the whole reload.
- [ ] No wire message, `WIRE_VERSION`, app-protocol constant, replicated-slot schema
  entry, or state-slot fingerprint input changes (review/grep gate). `DefaultWeaponFirePayload`
  and `TUNING_PAYLOAD_EPOCH` (`crates/postretro/src/netcode/tuning_payload.rs`) are unchanged:
  the client models no reload state, so nothing this spec adds belongs on the host tuning
  payload, and an implementer who rode `reloadStyle` to clients would trip its committed JSON
  fixture rather than this gate (review/grep gate). No switching,
  inventory, pickup, or multi-pellet behavior is built, and no `Raising`, `Lowering`, or
  `Stowed` variant and no equip-timing descriptor field are added (review/grep gate).
  `allows_fire()` returns true for `Idle` only, so a `ShellLoading` cancel reaches `Idle`
  before the shot is authorized and no site authorizes from `ShellLoading` directly
  (review/grep gate). The reload path debits the pawn `AmmoReserve` only through
  `AmmoReserve::take`, never by indexing the pool (review/grep gate) — a debit-side,
  reload-path-scoped gate, because credits legitimately originate outside the weapon stage
  today (`seed_weapon_reserve` at spawn, `grantAmmo` from reactions). No new `unsafe`
  (review/grep gate).
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
- [ ] The weapon-stage split is behavior-preserving, verified at the end of Task 1: the
  existing suite passes with no changes beyond import paths and module placement, and no
  caller outside the sim module changes — the netcode hit-acceptance path still reaches the
  authorized-impact entry point at its current path. Later tasks change the suite
  substantially; this criterion is not re-run at epic close.
- [ ] A weapon ticked with no owning pawn still advances timers, resolves expiries, and
  gates fire, but refuses every reload silently, emitting no delivery — it can fire and
  cool, it cannot reload or credit a shell. A pawnless window consumes no input edge: a
  reload level held across a death, a respawn, or a fly-camera stretch still produces a
  rising edge on the tick the pawn returns, and starts a loop there.
- [ ] The per-shell loop is demoable from dev content alone on `combat-demo`. Compile it
  (`cargo run -p postretro-level-compiler -- content/dev/maps/combat-demo.map -o
  content/dev/maps/combat-demo.prl`) and launch it (`cargo run -p xtask -- run
  content/dev/maps/combat-demo.prl`); the dev player equips `reference_shotgun`, its
  viewmodel renders in first person exactly as the pistol's does, and firing to empty,
  holding reload, watching the magazine climb one round at a time, and firing mid-loop to
  cancel are all observable in that session. The cancel has two observable shapes and both
  are walked: release reload before firing and the weapon stays idle with its loaded shells;
  hold reload through the shot and the loop restarts on the next tick, because the cancel
  clears the consumed-press flag. The kill payout and the pickup volume on that
  map credit the shotgun's ammo type, so both grant walkthroughs still show a reserve the
  player can load. The map choice is deliberate: those two grants exist only on
  `combat-demo`, not on CLAUDE.md's default `campaign-test`.
- [ ] `reference_shotgun` declares `thirdPersonModel` and the preload sweep resolves it
  (grep the archetype, confirm the model loads at boot). It is not observable in a
  single-player launch — the third-person model is consumed only by remote-pawn
  materialization, and the dev player mesh is `shadowOnly: true` — so it renders on a
  remote pawn in co-op only, and this criterion is discharged by declaration and preload
  rather than by looking at it.
- [ ] Script-visible event order inside one frame is fixed. On a cancel tick a handler
  observes the fire or `dry_fire` event before `reload_cancelled`, because
  `pending_weapon_events` drains ahead of `pending_reload_deliveries`. Both drains run once,
  after every fixed tick in the frame has completed, so a catch-up frame delivers its events
  in tick order and every handler in it reads the same final ammo state — several
  `reload_shell_loaded` handlers in one frame all see the magazine and reserve the last tick
  left.

## Ordering matrix

The invariants above say what holds. This says in what order, for every case where two
producers could resolve a tick differently. Each row is one test. Task 6's test list defers
to this table rather than restating it; where a row and an AC disagree, the row is the
defect report and the AC is wrong. Two rows are matrix-only in part: the `Idle` zeroing of
the three timer fields and the discarded overshoot are internal-state hygiene, unobservable
to a mod, so no AC states them and their absence from the criteria is not a gap. The
`reload_credited` half of that zeroing is mod-observable — it is the `transferred` count on
`reload_completed` and the count `reload_cancelled` carries — and AC 4 owns it.

Stage order within one machine tick, from Task 2: **1** reload intent → **2** advance the
state timer → **3** resolve any expiry → **4** fire intent.

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Cooldown decrement vs. the state-blind verdict | The per-tick `cooldown_remaining_ms` decrement is the first act of stage 4, ahead of the state-blind verdict; the real gate reads the decremented value and does not decrement again | A `ShellLoading` weapon whose cooldown reaches `0` within this tick cancels and fires on this tick. Left inside the gate, the shot is swallowed on the exact tick it became legal |
| Reload edge + authorized fire, same tick, `magazine` | Stage 1 enters `Reloading` and sets the started-this-tick latch; stage 4 consults it | `reload_started` emitted, fire `Rejected`, no `dry_fire`, cooldown untouched beyond the decrement, magazine unchanged — including when `reloadMs` is shorter than one tick and the weapon is already back at `Idle` by stage 4 |
| Reload edge + authorized fire, same tick, `perShell` | Identical stages | Identical outcome. The latch refuses the shot, not `allows_fire(state)`; the two styles must not diverge on a start tick, and no `reload_cancelled` is emitted |
| Step expiry + authorized fire, same tick, loop would continue | Stage 3 credits the expiring step and restarts; stage 4 cancels | The credited shell is kept and counted: magazine +1, reserve −1, `reload_shell_loaded`, `reload_credited` +1. Only the restarted step is forfeited. `reload_cancelled` carries the count including that shell. The shot resolves this tick |
| Step expiry + authorized fire, same tick, expiry fills the magazine | Stage 3 credits, sees the magazine at capacity, emits `reload_completed`, lands `Idle` — before stage 4 | Completion wins. No `reload_cancelled`. The shot is authorized by the ordinary gate from `Idle`, not by a cancel |
| Fire gate internal check order | The real gate applies the started-this-tick latch → `allows_fire(state)` ahead of the state-blind verdict's own terms (`can_fire` → `wants_fire` → cooldown → cost), returning `Rejected` when either fails, otherwise returning that verdict unchanged, then magazine debit → cooldown reset | A below-cost trigger pull during a reload of either style returns `Rejected`: no `dry_fire`, no cooldown reset. The same pull from `Idle` returns `Empty`. Folding the cost term in ahead of the state term collapses the two and breaks AC 7 |
| `shoot_press_consumed` and the state-blind verdict | The verdict is computed first and reads the pre-write flag; the gate then writes or clears it unconditionally, ahead of the latch and `allows_fire(state)` | One `Semi` press cancels a loop and fires exactly once. Held into the next tick it neither cancels nor fires. A verdict omitting the flag re-means semi fire. A tick refused for state or latch reasons still does the bookkeeping, so a release after it re-arms the press |
| Reload level held across a pawnless window | `reload_press_consumed` is written inside the pawn guard, not ahead of it | Pawnless ticks leave the flag untouched. The first tick with a pawn sees a rising edge and starts the loop |
| Pawnless expiry | Stage 2 advances, stage 3 resolves the expiry with no pawn | Transition to `Idle`, no transfer, no delivery, `reload_credited` zeroed. The `reload_started` gets no terminator |
| Hot reload removes the ammo block mid-loop | Refresh preserves state and timers; the next expiry reads `effective().ammo == None` | Credits nothing, emits `reload_completed { transferred: reload_credited }`, returns to `Idle`. Same for `ShellLoading` and `Reloading` |
| Style flip mid-loop | Refreshed classifiers take effect at the next decision point | `perShell` → `magazine`: the in-flight step credits its round, the loop-continue predicate fails on style, `reload_completed`. `magazine` → `perShell` mid-`Reloading`: the in-flight timer completes atomically, as today |
| `reload_credited` on the atomic path | The atomic completion adds its single `reserve.take` result to `reload_credited` before emitting | `reload_completed { transferred: reload_credited }` is one expression serving both endings. A `magazine` reload reports its real transfer, never `0` |
| Every transition to `Idle` | The transition zeroes as it lands, not the caller afterwards | `state_remaining_ms`, `state_total_ms`, `state_elapsed_sub_ms`, and `reload_credited` all `0` — for cancel, capacity end, reserve end, `take → 0`, pawnless expiry, and ammo removed alike |
| Non-restarting expiry and the sub-ms carry | An ending that does not restart a step discards the overshoot rather than storing it | Remaining time and carry both `0`, so no surviving fraction biases the next reload's first tick |
| N steps inside one tick | Expiry returns the whole overshoot in ms; the restart subtracts its integer part and loops while the overshoot is `>=` a full step; the loop-continue predicate and the credit `take` read one working `AmmoReserve` copy, written back once after the loop | N shells credited, N `reload_shell_loaded`, `reload_credited` +N, reserve −N. The HUD publishes one `+N` step and one progress sample. The `take → 0` guard does not fire |
| Catch-up frame containing both a start and an expiry | Events accumulate across every fixed tick and drain once, after the loop | `reload_started` then `reload_shell_loaded`, in tick order, in one frame. Every handler reads the same final ammo state |
| Step boundary between snapshots | `clear_all_reload_feedback` runs only when `host_owner_state_projection_due()` | The `Completed` marker survives to the next projection, so a remote owner sees the endpoint, up to one snapshot interval late. Two boundaries inside one window coalesce to one published endpoint — the bound AC 12 states |
| Cancel with the reload level still held | The cancel clears `reload_press_consumed` | The following tick sees a rising edge and restarts the loop from `Idle` |
| Reload edge on the tick a loop ends by capacity | Stage 1 evaluates the edge against the pre-expiry state, where `allows_reload()` is false | The edge is a no-op, matching the shipped rising-edge dedup. Stage 3 still ends the loop with `reload_completed`. The consumed-press flag is set, so no second loop starts until the key is released |
| Event drain order on a cancel tick | `main.rs` drains `pending_weapon_events` before `pending_reload_deliveries` | Scripts observe `activate` (or `dry_fire`) before `reload_cancelled` |

## Tasks

### Task 1: Extract the weapon stage out of `sim/mod.rs`

Behavior-preserving split, no functional change. `crates/postretro/src/sim/mod.rs`
carries 1445 production lines and this plan extends its weapon orchestration. Move into
a new `crates/postretro/src/sim/weapon_stage.rs`: `weapon_fire_command`,
`normalize_aim_direction`, `run_remote_weapon_commands`, `run_local_weapon_command`,
`apply_weapon_impact_damage`, `apply_authorized_weapon_impact_damage`,
`apply_weapon_impact_damage_with_source`, and the test-only `deliver_reload_to_weapon`.
Keep `run_death_sweep` in `sim/mod.rs` — it sits inside that address range but is the
death stage, and it has two consumers outside the sim module (`main.rs` and
`impact_policy.rs`), so moving it would widen the re-export surface for no gain. Exactly
one moved function has a consumer outside the sim module:
`apply_authorized_weapon_impact_damage`, called by fully-qualified path
(`crate::sim::apply_authorized_weapon_impact_damage`) from
`crates/postretro/src/netcode/mod.rs`. Re-export it from `sim/mod.rs` so that path still
resolves; the other seven are sim-internal and need no re-export.

`reload.rs` stays a sibling module of `weapon_stage.rs` under `sim/`, and keeps its types
and helpers: `ReloadDelivery`, `ReloadOutcome` and its `event_name`, the timer-advance
helper, and the `clear_all_feedback` / `clear_feedback_for_weapon` pair. `sim/mod.rs`'s
existing `pub(crate) use reload::{…}` block keeps its current shape and location —
`main.rs` consumes the renamed feedback-clear exports through it. Only the orchestrating
`reload::tick` entry point is at stake, and Task 2 fuses it away.

Test boundary: move every test whose subject is one of the eight moved functions, plus
every caller of `deliver_reload_to_weapon`. A test spanning both stages stays with the
stage it *ends* in — the one asserting `apply_authorized_weapon_impact_damage` then
`run_death_sweep` is a death-sweep test and stays in `sim/mod.rs`, calling the moved
entry point through its re-export. Override on that rule: a test whose assertions are on
`TickEvents.reload_deliveries`, `TickEvents.authorized_shots`, or the `WeaponComponent`
moves to `weapon_stage.rs` regardless of which stage its helper runs last — those are
weapon-stage outputs wherever the tick ends.

The `sim/mod.rs` test module's fixture helpers are shared across the split and **stay
where they are**, widened to `pub(super)`: `weapon_component`, `ammo_weapon_component`,
`spawn_reload_pair`, `sim_command`, `zero_movement`, `trigger_movement`, `remote_command`,
`run_remote_only_tick`, `run_local_only_tick`. `weapon_stage.rs`'s new test module imports
them through `crate::sim::tests`. Do not duplicate them; two drifting copies of
`spawn_reload_pair` is the failure mode this rule exists to prevent.

The tick-order call sites inside `simulate_tick_with_presentation_aim` stay put and call
into the new module; `simulate_tick` is a thin forwarding wrapper and is untouched.

### Task 2: `WieldableState` + the fused machine tick (thin slice)

Narrow vertical slice through every seam, reproducing today's exact behavior with no new
authored surface and no new production states. Define `WieldableState` in a new
`crates/entities/src/components/wieldable_state.rs`, declared in the component barrel
(`crates/entities/src/components/mod.rs`) beside its siblings. The name is deliberate:
`context/research/weapon-model.md` §7 invariant 7 requires equip and switch machinery be named for
wieldables, and the switching spec will extend this machine with equip states. Hosting it
on the weapon kind is equally deliberate while weapon is the only wieldable kind that
exists — do not invent a wieldable component for it. Derive at least
`Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize`, with an explicit
`#[default] Idle`. `InterruptPolicy`
(`crates/entities/src/components/animation/state.rs`) is the precedent — it carries exactly
that set including `#[default]` — and `FadeSourceKind` beside it does too. The enums that
carry no default are the descriptor ones in `crates/foundation/src/data_descriptors/`,
`FireMode` among them, because none of them has a defaultable variant.
`WeaponComponent` itself derives only `Debug, Clone, PartialEq, Serialize,
Deserialize`, so it is not the source of `Copy`, `Eq`, or `Default` here. The serialized
variant spelling is unobserved — the enum rides on `WeaponComponent`, nothing reads it
across a boundary, and the boundary inventory's component-local rows say so — so it carries
no `rename_all` and variants serialize as written.

Add to `WeaponComponent`
(`crates/entities/src/components/weapon.rs`) a `state: WieldableState` field
(`#[serde(default)]`, `Default = Idle`) and replace `reload_remaining_ms`,
`reload_total_ms`, and `reload_elapsed_sub_ms` with one generalized timed-state triple
owned by the machine: `state_remaining_ms: u32`, `state_total_ms: u32`,
`state_elapsed_sub_ms: f64` — the fractional carry that prevents per-tick rounding bias.
All three carry `#[serde(default)]`, as `state` does, so a persisted component written
before this task deserializes at zero rather than failing.
The names are state-agnostic on purpose; a reload-specific spelling is exactly what rule
(c) below exists to avoid, and these three must agree across roughly ninety references.
Add one more field the per-shell loop needs and nothing derives: `reload_credited: u32`
(`#[serde(default)]` too), the cumulative rounds credited by the current reload, reset on
entry to a reload. It
cannot be recovered from `magazine`, which also moves on fire and can be at any level when
a loop is entered. Define the three variants this spec ships (`Idle`, `Reloading`,
`ShellLoading`) so Task 4 fills transition bodies rather than adding variants or arms; only `Idle` and
`Reloading` are reachable in production after this task. `from_descriptor` materializes
`Idle`. Update the `WeaponComponent` literals across the workspace that the field changes
break — the compiler enumerates them.

The timer rename reaches seven files: `crates/entities/src/components/weapon.rs`,
`crates/postretro/src/sim/mod.rs`, `crates/postretro/src/sim/reload.rs`,
`crates/postretro/src/weapon/mod.rs`, `crates/postretro/src/netcode/state_slots.rs`,
`crates/postretro/src/netcode/lifecycle.rs`, and
`crates/postretro/src/scripting/systems/ui_proxy.rs`. Several shipped tests fabricate
mid-reload state by writing the timer directly (`reload_remaining_ms = 275`, `= 300`,
`= 600` and similar). Those must also set `state`, because `reload_status()` now derives
from state plus timers — renamed and left alone they still compile and assert the wrong
thing. The fabrication sites span five files:
`crates/entities/src/components/weapon.rs`,
`crates/postretro/src/scripting/systems/ui_proxy.rs`,
`crates/postretro/src/netcode/state_slots.rs`,
`crates/postretro/src/netcode/lifecycle.rs`, and
`crates/postretro/src/weapon/mod.rs`. Grep the timer names rather than working to a count.

Fuse the two producers. `sim/reload.rs::tick` and the private
`weapon/mod.rs::apply_weapon_fire_state` become one machine tick with a single ordered
decision per fixed tick. **The order is the shipped one and must not be rearranged:**
reload intent (rising edge, start guards, entry) → advance the state timer → resolve any
expiry → fire intent. `reload::tick` evaluates the rising edge before calling its
timer-advance helper today, and three shipped tests AC 2 requires to pass depend on that
ordering: `held_reload_starts_once_and_release_still_advances_to_completion` (a start tick
at `dt 0.25` against a 1000 ms reload must leave the timer at `750`, not `1000`),
`reload_start_tick_advances_and_can_complete_immediately`, and
`reload_completion_atomically_transfers_partial_live_reserve_only_at_zero`. Advancing
before the start edge breaks all three.

The fire stage's *internal* order is the shipped one too, and Task 4's cancel edge sits
inside it, so pin it here: the per-tick `cooldown_remaining_ms` decrement first, then
`can_fire` → `wants_fire` → cooldown → the started-this-tick latch → `allows_fire(state)` →
cost → magazine debit → cooldown reset. Hoisting the decrement out of the gate to the top of
the stage is the one deliberate change, so Task 4's state-blind verdict and the real
gate read the same decremented value and the gate does not decrement twice. Task 4 splits
that chain without reordering its outcomes: every check above the cost term yields
`Rejected` when it fails, so the verdict may carry `can_fire`, `wants_fire`, cooldown, and
cost while the real gate applies the latch and `allows_fire(state)` ahead of that answer. The state check
sitting *above* the cost check is why a below-cost trigger pull during a reload returns
`Rejected` today — no `dry_fire`, no cooldown reset. Place `allows_fire()` after the resource
checks and that case returns `Empty` instead, firing `dry_fire` and arming the cooldown, and
AC 7 fails.

`reload_started_this_tick` disappears as a *parameter*, not as a *rule*. The fire gate
must still refuse a shot on a tick where a reload started, even when that reload was
shorter than the tick and the weapon is already back at `Idle` by the time fire is
evaluated — pinned by `immediate_local_reload_still_blocks_fire_for_start_tick` and
`immediate_remote_reload_still_blocks_fire_for_start_tick`. Keying legality purely on
`allows_fire()` authorizes that shot and breaks both. The mirror constraint is equally
pinned: a reload *completing* on a tick must leave the shot authorized, per
`local_reload_completion_refills_before_same_tick_fire`, so a blanket "any reload activity
this tick blocks fire" is wrong in the other direction. Replacement: a machine-internal
"a reload started on this tick" latch, set on the entry transition, consulted by the fire
gate alongside `allows_fire()`, cleared at tick end. It is a local of the machine tick, not
a `WieldableState` variant and not a component field, so rule (d) below is unaffected.

The machine's home is a new function in
`sim/weapon_stage.rs` — Task 1's module — called from both `run_local_weapon_command` and
`run_remote_weapon_commands`, ahead of `weapon::tick_resolved_component` /
`weapon::tick_state_only_component`. No callee below those two can host it:
`apply_weapon_fire_state` takes neither a registry nor a pawn id, and
`tick_state_only_component` takes no registry at all, so the `AmmoReserve` transfer has
nowhere to happen there. A placement inside `tick_resolved_component` would additionally
skip the remote-pawn path entirely. The
`reload_started_this_tick` parameter and the two `deliveries.iter().any(...)` computations
that feed it at the call sites disappear, replaced by the machine-internal latch above; the
machine returns its own outcome list plus the fire
authorization. Keep `ReloadDelivery`, `ReloadOutcome`, and every existing event name and
payload byte-identical.

The machine calls the helpers that stay in `reload.rs` rather than re-implementing them:
the timer-advance helper, `ReloadDelivery` / `ReloadOutcome` construction, and the
feedback-clear pair. Only `reload::tick`'s orchestration is absorbed. Concretely,
`advance_timer` in `sim/reload.rs` widens from private to `pub(super)` (and `tick_ms` with
it, if the machine needs it directly) and is **not** duplicated into `weapon_stage.rs` —
Task 4 extends that one helper's return rather than forking a second copy that would drift
on the carry contract. The `#[cfg(test)]
deliver_reload_to_weapon` harness — moved to `sim/weapon_stage.rs` by Task 1, together with
its roughly twenty call sites — re-points at the machine entry point with a no-fire command,
so those call sites keep their shape and their assertions.

That leaves the two `weapon::tick_*` entry points with different fates.
`tick_state_only_component` did nothing but derive the fire mode, cooldown, and per-shot
cost from `weapon.effective()` and hand them to `apply_weapon_fire_state` — a derivation
the machine must reproduce — so it and its
`#[cfg(test)] tick_state_only` wrapper are deleted;
`run_remote_weapon_commands` matches
the machine's `WeaponFireAuthorization` directly, keeping its existing
`Empty` → `weapon_events.push("dry_fire")` arm unchanged. `tick_resolved_component`
survives as pure shot resolution: it drops `tick_dt` and `reload_started_this_tick`, takes
the machine's `WeaponFireAuthorization` as an argument, and keeps `command` for aim origin
and direction. Its three-arm match stays exactly where it is, so
`Accepted` → `fire_hitscan` and `Empty` → `WeaponFireEvents { dry_fire: true }` are
unmoved. Its `#[cfg(test)] tick_resolved` wrapper takes the same argument swap, and so does
`#[cfg(test)] weapon::tick`, the snapshot-and-camera wrapper above it that the
`weapon/mod.rs` test helper `fire_tick` calls.

`weapon/mod.rs`'s test module is the largest consequence of the fusion and needs a rule,
not a count. Roughly twenty-five `fire_tick` / `fire_tick_with` call sites plus four direct
`tick_resolved` / `tick_resolved_component` calls route through the gate today. **The
rule:** a test whose assertions are on gate outputs — `WeaponFireAuthorization`,
`cooldown_remaining_ms`, `magazine`, `shoot_press_consumed`, or event presence that follows
from the gate's verdict — **migrates to the machine's test module in
`sim/weapon_stage.rs`**. A test whose assertions are on resolution outputs — impact point,
normal, damage payload, world-vs-entity hit selection, target health — stays in
`weapon/mod.rs` and passes a literal `WeaponFireAuthorization::Accepted`. A test asserting
both migrates and keeps both halves, calling the machine and then the resolution entry
point. Do **not** leave a migrated test in `weapon/mod.rs` calling
`crate::sim::weapon_stage::…`; that inverts the production dependency direction, which runs
sim → weapon.

The tests the rule moves, verified against source: `semi_weapon_fires_once_per_press`,
`auto_weapon_fires_repeatedly_when_held_after_cooldown`,
`below_cost_is_empty_at_state_seam_and_emits_only_dry_fire` (it calls
`apply_weapon_fire_state` directly), `empty_auto_weapon_emits_once_per_fire_interval`,
`reload_in_flight_silently_blocks_without_cancelling_or_spending`,
`resourceless_weapon_fires_without_magazine_gating_or_consumption`,
`ammo_shot_consumes_effective_cost_once_and_resolves_normally`,
`ammo_shot_spends_cost_on_open_space_miss`,
`open_space_shot_consumes_cooldown_without_impact`, and
`state_only_fire_advances_cooldown_without_hitscan_events` (the deleted
`tick_state_only` wrapper's only caller). `inactive_or_missing_wieldable_does_not_fire`
stays: its assertions are on the resolution wrapper's early return for a missing or
non-weapon wieldable, and a literal `Accepted` makes it strictly stronger. Everything else
in that module is resolution or `ClientWeaponState` and needs only the literal.

Those ten tests have no fixture driver in their new home, so state the disposition rather
than leaving it to discovery. They run on `fire_tick` / `fire_tick_with` plus `spawn_weapon`,
`weapon_descriptor`, `input_system`, `shoot_snapshot`, and `wall_world` — all private
helpers of `weapon/mod.rs`'s own `mod tests`, none of which exists in `weapon_stage.rs`.
`fire_tick` and `fire_tick_with` are gate drivers: they **move**, re-pointed at the machine
entry point followed by the resolution entry point. The other five still serve retained
resolution tests: they **stay**, widen to `pub(super)`, and `weapon_stage.rs`'s test module
imports them by path through `crate::weapon::tests`, the same shape Task 1 uses for
`crate::sim::tests`.

Two names collide and the winner is decided here. `weapon_component` and
`ammo_weapon_component` exist in **both** test modules with incompatible signatures:
`sim/mod.rs`'s take `(credit_source)` and `(credit_source, capacity, reserve, reload_ms)`
returning `(WeaponComponent, AmmoReserve)`; `weapon/mod.rs`'s take `(fire_mode, cooldown_ms)`
and `(fire_mode, cooldown_ms, magazine, cost_per_shot)` returning a bare `WeaponComponent`.
In `weapon_stage.rs`'s test module the `sim/mod.rs` pair keeps the bare names — Task 1
already routes that module through `crate::sim::tests`, and the reload fixtures outnumber
the gate fixtures there. Import the `weapon/mod.rs` pair aliased, `gate_weapon_component` and
`gate_ammo_weapon_component`, and rewrite the ten migrated bodies to the aliases. Leaving the
pair to resolve by import order is the failure this rule exists to prevent.

Build it **open for extension** — the equip lifecycle (`Raising`, `Lowering`, `Stowed`)
and a switch interrupt land in a later spec and must not force a reshape. Six rules, all
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
through `ReloadOutcome::event_name`, so a new endpoint pair needs no new plumbing;
(f) **what a step expiry means is decided inside the transition function**, as (state,
event) arms returning an outcome type, and callers match that outcome — never
`WieldableState`. A machine that matches on state to interpret its own expiry is a fifth
decision site, and AC 14's four-site guarantee fails at epic close.

Write the three predicates for all three variants now, including the one Task 4 gives
meaning to, because rule (d) forbids a `_` arm and the enum ships complete:
`Idle` → `allows_fire` true, `allows_reload` true, `is_reload_activity` false;
`Reloading` → false, false, true; `ShellLoading` → **false, false, true**. Task 4 changes
none of these — it adds the cancel edge ahead of the gate, so a `ShellLoading` weapon
reaches `Idle` before `allows_fire()` is consulted.

The `ShellLoading` transition arms ship here for the same reason, and with the same
placeholder honesty: rule (d) forbids a `_` arm, so every arm must exist before Task 4 gives
it meaning. They return the no-op outcome and hold the state. Not `unreachable!()` — nothing
in production reaches `ShellLoading` after this task, but a mis-sequenced merge that does
should hold state rather than panic.

**Resets belong to the transition, not to its callers.** Every transition to `Idle` zeroes
`state_remaining_ms`, `state_total_ms`, `state_elapsed_sub_ms`, and `reload_credited`. Every
ending that does not restart a step zeroes both the remaining time and the sub-millisecond
carry, so a surviving fraction cannot bias the next reload's first tick.

Cooldown is **not** one of these predicates and never becomes a state. A reload can start
while the weapon is cooling today — `reload::tick` never reads `cooldown_remaining_ms` —
and the machine keeps that true: `allows_reload()` is a function of state alone, the reload
intent is evaluated without consulting cooldown, and starting or completing a reload never
writes `cooldown_remaining_ms`. Cooldown stays an orthogonal rate limiter composing with
`Idle`.

Two plumbing facts the fusion turns up, both settled here rather than by the implementer.
First, **the registry borrow**: there is nothing to invert. Both call sites already hold
`RefMut<EntityRegistry>`. `run_local_weapon_command` passes an immutable reborrow into
`tick_resolved_component`; `run_remote_weapon_commands` passes no registry at all, because
`tick_state_only_component` takes none.
Run the machine under that mutable borrow, producing a fire authorization and an outcome
list, then reborrow immutably for hitscan resolution exactly as the code already does. Do
not widen the resolution signature to `&mut`; the renderer-adjacent read path stays a
read. The single real constraint is the one stated in Direction — the machine must not run
*inside* `tick_resolved_component`. Second, **a weapon with no pawn**:
`run_local_weapon_command` takes `pawn: Option<EntityId>` and today skips reload entirely
when it is `None` (fly-camera and headless harnesses). Keep that behavior. The machine
still advances timers, resolves expiries, and gates fire, but any transition that would
touch the reserve is refused **silently, emitting no delivery** — `ReloadDelivery` carries
a non-optional `pawn`, so a blocked outcome is unconstructible without one. That refusal
must not strand the state: an expiry reached with no pawn returns to `Idle` with no
transfer and no delivery. Timers advance pawnlessly under the machine where today
`reload::tick` is skipped wholesale, so a weapon that entered a reload with a pawn and
then ticks without one would otherwise sit in `Reloading` forever with fire locked. A
pawnless weapon fires and cools; it cannot reload.

One piece of that stays behind the pawn guard, against the pull to hoist it. `reload::tick`'s
first statement is edge bookkeeping — `reload_press_consumed = reload`, written ahead of
every guard — and it is only reached today because the whole call is skipped without a pawn.
The fused machine runs unconditionally, so that write must sit **inside** the pawn guard.
Run it pawnlessly and a reload level held across a death, a respawn, or a fly-camera stretch
is consumed by ticks that could never serve it, and produces no rising edge when the pawn
returns.

Define the **hot-reload policy** the later tasks depend on. `refresh_from_descriptor` has
no preserved set to extend — it assigns the authored tuning and nothing else, and
preservation is by omission, documented in a trailing comment. The one exception is
`credit_source`, assigned under an `if let Some(...)` so an absent authored value keeps the
spawn-time resolution; it is not a counterexample to the rule, but a literal reader will
trip on it. So: leave `state`, the
timed triple, and `reload_credited` out of the assignment list, and extend that trailing
comment to name them alongside the cooldown, magazine, and input edges it already
describes. Three tests in `crates/entities/src/components/weapon.rs` assert refresh
preservation and all three must be updated:
`refresh_updates_ammo_tuning_and_preserves_all_live_state` and
`refresh_from_descriptor_updates_stats_and_preserves_live_state` extend to assert the new
fields, a mechanical addition; `refresh_can_remove_ammo_tuning_without_aborting_live_reload`
needs a semantic edit rather than a rename, because it fabricates mid-reload state by
writing the timer directly and then asserts `reload_status() == (0.625, true)` — a tuple
that no longer follows once `reload_status()` derives from `state`. The edit: set
`state = WieldableState::Reloading` in the fixture alongside the timer writes, and keep the
existing `(0.625, true)` expectation. The assertion is the point of the test — that removing
the ammo block does not abort a live reload — and it stays true, it just now needs the state
to say so. Durations
and classifiers refresh and take effect at the next decision point, matching the existing
precedent that reload completion re-reads `effective()`.
Rewrite `reload_status()` to derive `(progress, active)` from `is_reload_activity()` plus
the timers rather than from `reload_remaining_ms > 0`, keeping today's outputs identical
for `Idle` and `Reloading` including the one-frame `ReloadFeedback` endpoints. **The
`reload_feedback` arms stay first, and the state term is the `None` arm.** Matching the
one-frame endpoint markers ahead of everything else is what produces the `(1.0, true)`
completion frame: by then the state is already `Idle`, so a state-first accessor reports
`(0.0, false)` and drops the endpoint sample the shipped meter depends on.

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
`Eq` is not optional in that set: `AmmoResource` and `WeaponAmmoTuning` both derive it, so
a non-`Eq` member breaks them.
Add `reload_style: ReloadStyle` to `AmmoResource` with
`#[serde(default = "default_reload_style", rename = "reloadStyle")]` and a free
`const fn default_reload_style() -> ReloadStyle { ReloadStyle::Magazine }` beside the
existing `default_cost_per_shot` / `default_reload_ms` in that file, so every existing
authored resource block keeps today's behavior. Use that form, not `#[serde(default)]` plus
a `Default` impl — the file's convention is the named-function form, and a `Default` impl on
a public descriptor enum invites callers to rely on it. `AmmoResource` itself has
no `Default` impl, so exhaustive Rust struct literals still break: nine of them across
seven files — three in `crates/foundation/src/data_descriptors/types/combat.rs`, one each
in `crates/entities/src/components/weapon.rs`, `crates/postretro/src/sim/mod.rs`,
`crates/postretro/src/weapon/mod.rs`,
`crates/postretro/src/scripting/systems/ui_proxy.rs`,
`crates/postretro/src/scripting/builtins/data_archetype.rs`, and
`crates/postretro/src/scripting/builtins/net_descriptor.rs`. The compiler enumerates them;
each takes `ReloadStyle::Magazine`. No new
`validate()` rule is needed — serde rejects unknown values — but state that in the task's
test list rather than leaving it implicit.

Carry the field through `WeaponAmmoTuning` and
`EffectiveAmmoStats` in `crates/entities/src/components/weapon.rs` so producers read it
through `effective()`, never off the descriptor, matching how `reload_ms` is already
routed. Both are derived types with their own literal sites, and one is off the
`AmmoResource` file list above: `WeaponAmmoTuning` is constructed in `ammo_tuning` and in
two tests in that same file, and once more in a fixture in
`crates/postretro/src/netcode/state_slots.rs`; `EffectiveAmmoStats` is constructed in
`effective()` and in one test there. `WeaponAmmoTuning` also derives
`Serialize, Deserialize` and rides on `WeaponComponent`, so its new field needs a serde
default too — a persisted component missing it must deserialize as `Magazine`. The
descriptor crate's `default_reload_style` is private to `combat.rs`, so give
`weapon.rs` its own one-line `const fn` of the same shape rather than exporting it.
`EffectiveAmmoStats` carries no serde. Register the field
beside the other `AmmoResource` fields in
`crates/postretro/src/scripting/primitives/mod.rs`, and beside that field registration add
a `register_enum("ReloadStyle")` declaration with a doc line per variant, matching the
sibling `FireMode` / `ResolutionMode` registrations. Without it the field's `"ReloadStyle"`
type name is a dangling reference in the emitted typedefs and the drift test fails
opaquely. Rewrite the neighbouring `reloadMs` registration doc line in the same pass — it
reads "Reload duration in milliseconds" and lands verbatim in the committed typedefs, the
surface a modder reads in-editor, so leaving it stale ships the re-meaning everywhere
except where it is read. Regenerate the committed
`sdk/types/postretro.d.ts` / `.d.luau` fixtures with
`cargo run -p postretro --bin gen-script-types` and commit the result — the drift test
`committed_sdk_types_match_current_registry`
(`crates/postretro/src/scripting/typedef/tests/committed.rs`) fails until you do, and its
message names that command.

Two `docs/scripting-reference.md` edits, both under `## components.weapon`: document the
field and both values in the `resource` row, and rewrite the trailing paragraph that
begins "The authored `reloadMs` is the base reload duration" — it sits outside the row and
carries the same stale meaning. Both must state that `reloadMs` is the duration of one
reload *step* — the whole reload under `magazine`, one shell under `perShell` — while the
trailing paragraph keeps its point about the effective-stat seam.

### Task 4: The per-shell loop

Task 2 shipped the `ShellLoading` arms on the transition function as placeholders that hold
state, and its three legality-predicate rows (`false, false, true`). This task gives those
arms their real bodies. The predicates are unchanged and must stay unchanged: the cancel
edge runs ahead of the gate, so a `ShellLoading` weapon reaches `Idle` before
`allows_fire()` is consulted, and relaxing `allows_fire` for `ShellLoading` instead trips
AC 13's grep gate. A reload rising edge from `Idle` consults
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
emit `reload_shell_loaded`. Then evaluate the loop-continue predicate against live state,
re-reading all three terms each step: continue when the magazine is below the effective
capacity, **and** the reserve still has rounds, **and** the effective `reloadStyle` is
still `PerShell` — a mid-loop flip to `magazine` therefore ends the reload after the
in-flight step credits its round, and the reverse flip mid-`Reloading` completes atomically
as the shipped atomic reload does, both following Task 2's rule that refreshed classifiers
take effect at the next decision point. Otherwise end the reload, emit `reload_completed`
carrying `reload_credited` as
the cumulative transferred count for the whole reload, and return to `Idle`. `take`
returning `0` — the reserve emptied between check and credit — ends the reload with no
shell event but the same `reload_completed`, cumulative count included, so all three
endings agree on their endpoint. That third ending is unreachable from ordinary play, since
the loop-continue predicate already checked the reserve; it is exercised from a fabricated
`ShellLoading` fixture whose reserve is zeroed between check and credit. Write it as a
guard, not as dead code, and say so in a comment.

Two more endings share that endpoint. `Reloading`'s atomic completion accumulates its single
`reserve.take` result into `reload_credited` before emitting, so
`reload_completed { transferred: reload_credited }` is one expression serving both styles;
unify the two endings without that accumulation and every `magazine` reload reports
`transferred: 0`. And a hot reload can remove the ammo block outright —
`refresh_from_descriptor` assigns `self.ammo` from the descriptor's resource, which is `None`
for a resourceless one, and `refresh_can_remove_ammo_tuning_without_aborting_live_reload`
pins that a live reload survives it. The per-shell credit then has no `ammo_type` and the
loop-continue predicate has no `reloadStyle` to read. Outcome, both styles: an expiry with
`effective().ammo == None` credits nothing and ends the reload with
`reload_completed { transferred: reload_credited }`, returning to `Idle`.

Continuing restarts the step timer within the same tick so there is no idle gap between
shells, and the restart is where the sub-millisecond carry needs care. `advance_timer` in
`crates/postretro/src/sim/reload.rs` is the helper in question: today its expiry path
zeroes both the timer and the carry, and it validates the carry as
`>= 0.0 && < 1.0` — it is a sub-millisecond fraction by contract. At a step restart the
overshoot is bounded by the tick, roughly 16.67 ms at 60 Hz, not by 1 ms; writing it into
the carry field would be silently discarded by that guard and the step cadence would drift.
So: the expiry path returns the whole overshoot as `f64` milliseconds, the restart
subtracts its integer-millisecond part from the new step's remaining time and stores only
the fractional part in the carry, and the restart loops while the overshoot is `>=` a full
step — crediting several shells in one tick rather than losing the excess. The comparison
is inclusive on purpose: at an overshoot of exactly `reloadMs` a strict `>` leaves the new
step at `0` remaining without looping, deferring that shell a whole tick — precisely the
drift the anti-drift test in Task 6 targets.

That loop reads the reserve more than once per tick, and the shipped transfer shape makes
that a trap. It clones the pawn `AmmoReserve`, takes from the clone, and writes the clone
back. Within one tick's multi-credit loop the loop-continue predicate and the credit `take`
must read the **same working copy**, with a single write-back after the loop. Hoist the clone
but leave the predicate on the registry's copy and the predicate re-reads a stale reserve
while the clone drains: the `take → 0` guard then fires from ordinary play, contradicting the
premise that makes it a guard.

The multi-credit loop is where the per-shell HUD guarantees stop being per-shell. AC 12's
machine-level guarantee is **per tick**, not per shell, and its publication guarantee is
weaker again: both consumers publish once per rendered frame from the live magazine, so two
shells credited inside one tick surface as a single `+2` step and a single progress sample.
That is intended and is what AC 12 states. Do not add a per-shell publish to compensate.

Add the cancel edge, the reason Task 2's fusion exists. The decision must be made
**side-effect-free**. Today's gate mutates while it decides — it decrements the
cooldown, sets `shoot_press_consumed`, debits the magazine, then resets the cooldown — so
asking it "would this shot be authorized?" cannot be done by running it. Factor the
authorization question into the **state-blind verdict**: one function over
(command, fire mode, `shoot_press_consumed`, cooldown, cost, magazine) that writes nothing,
consults neither `state` nor the started-this-tick latch, and answers as if the state were
`Idle`. Name it once and use that name; both callers below are the only callers it has.
It returns a full `WeaponFireAuthorization` — `Accepted`, `Empty`, `Rejected` — not a
boolean, because the gate's answer is three-way and a boolean cannot carry it.

The two callers compose it differently. The cancel edge takes the shot when the verdict is
`Accepted` and the latch is clear. The real gate runs a fixed order: compute the verdict
first — it writes nothing, so computing it on every path costs nothing — then write or clear
`shoot_press_consumed` **unconditionally**, set on a `Semi` press and cleared when
`button.active` is false, then apply the latch and `allows_fire(state)`, returning
`Rejected` when either fails, and otherwise return the verdict unchanged before running the
debit and the cooldown reset. The bookkeeping write is unconditional because the shipped
gate writes it ahead of every one of its returns, which AC 2 preserves. Gate it behind the
latch or the state instead and a tick refused for either reason skips it, stranding the
flag: fire, reload, release, re-press then needs a second release before the weapon fires
again. State still loses before cost is ever consulted: a below-cost pull during
`ShellLoading` is `Rejected`, while the same pull from
`Idle` is `Empty` — the distinction AC 7 protects — out of one shared computation with no
duplicated legality. Conjoin a single boolean with `allows_fire(state)` instead and the cost
term folds in ahead of the state term, collapsing the two into `Empty` and breaking AC 7.

`state` is deliberately **not** a verdict term.
Task 2 gives `ShellLoading` `allows_fire = false`, so a verdict consulting `state` answers
`Rejected` for every `ShellLoading` weapon and the cancel edge can never fire.
`shoot_press_consumed`
*is* a term, because `wants_fire` under `FireMode::Semi` is
`command.button.pressed && !weapon.shoot_press_consumed`; a verdict omitting it silently
re-means semi fire. The verdict reads the pre-write flag, matching the shipped gate where
`wants_fire` reads it before setting it; write it ahead of the verdict and the verdict
answers a question the gate has already altered. The
per-tick cooldown decrement is hoisted to the top of the fire stage per Task 2, ahead of the
verdict, so verdict and gate read the same value and the gate does not decrement twice —
without that, a `ShellLoading` weapon whose cooldown reaches zero within a tick fails to
cancel and the shot is swallowed on the exact tick it became legal.

The verdict includes `can_fire`, since the
remote path repurposes it to mean "pawn has a `NetworkId`" and a remote pawn with no
`shot_id` would otherwise cancel its own loop every tick while firing nothing. On an
`Accepted` verdict `ShellLoading` does not reject the shot: it cancels, forfeits the in-flight step with
no round credited for it and no reserve debit, emits `reload_cancelled` carrying
`reload_credited`, clears `reload_press_consumed` so a held reload key restarts the loop on
the following tick, clears `reload_feedback`, transitions to `Idle`,
and lets the shot resolve on that same tick. The one thing the feedback clear leaves alone is
a `Completed` marker written earlier in the same tick by a step that expired at stage 3: it
survives, so the meter's last sample is the shell that landed rather than a blank. The
feedback clear lands
here rather than in Task 5 because phases are sequential: without it, the end of this task
leaves a cancelled weapon `Idle` with `Some(ReloadFeedback::Started)`, so `reload_status()`
reports `(0.0, true)` and AC 6 fails until Task 5 ships.

Two drain-order facts are contract, not incident, and the cancel is where they become
script-visible. `main.rs` drains `pending_weapon_events` before `pending_reload_deliveries`,
so on a cancel tick a script observes the fire or `dry_fire` event before `reload_cancelled`.
Both drains run only after every catch-up tick in the frame has completed, so several
`reload_shell_loaded` handlers firing in one frame all read the same final ammo state.
AC 20 gates both.

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
forbids a `_` arm, and a fifth match site is exactly what AC 14's four-site guarantee
rules out.

**Keep the `reload_feedback` arms first.** `reload_status()` matches the one-frame endpoint
markers before it consults anything else, and that ordering is what produces the
`(1.0, true)` completion frame — by then the state is already `Idle`, so a state-first
accessor would report `(0.0, false)` and drop the endpoint sample the shipped meter depends
on. Task 2 preserves that ordering; this task must not invert it while adding the state
term.

Extend the `ReloadFeedback` one-frame
endpoint lifecycle to mark step boundaries rather than whole-reload boundaries, so every
step ends on a `Completed` marker and a step shorter than one tick still contributes its
endpoint samples; when a step's end and the next step's start land in the same tick the
completion sample wins, matching the existing precedent for a reload shorter than one tick.
The single-step `magazine` case is bit-identical, because there one step is the whole
reload. The consequence, which AC 12 states and this task owns: the per-shell sample
sequence is an exact `0.0` on the reload's start tick (the `Started` marker, fired once for
the reload, not once per step), a ramp to the first expiry, then `1.0` on each step-boundary
tick followed by a ramp rising from just above `0`. Steps 2..N publish no exact `0.0`,
because completion wins at the boundary they would have published it on. `active` stays true
across every boundary. Assert that sequence, not "a `0.0` per shell". This is a meter
lifecycle only, on a channel disjoint from the event one: the markers are `#[serde(skip)]`
one-frame component state read by `reload_status()`, while `reload_started` is a
`ReloadOutcome` on `TickEvents.reload_deliveries`. So the step-scoped markers do not drive
event firing — Task 4's one `reload_started` per whole reload stands, and neither
constrains the other. Wire the markers to the events and the loop emits N `reload_started`s
and breaks AC 3. Task 4's cancel
already clears `reload_feedback`; the rationale belongs to this task's subject, which is
that a stale `Some(Started)` would report `(0.0, true)` on an `Idle` weapon. The one marker
that clear spares is a `Completed` written earlier in the same tick — the meter's last sample
must be the shell that landed.

**The clear cadence does not change.** `clear_reload_feedback_for_weapon` runs per frame for
the local active wieldable and `clear_all_reload_feedback` stays gated on
`host_owner_state_projection_due()`, true only every second tick. That gate is the reason a
marker reaches a remote owner at all — it must outlive the tick that wrote it until every
consumer has sampled it — so tightening it to fix the sample sequence would break the
projection instead. What it costs is the bound AC 12 states: two step boundaries inside one
snapshot window coalesce, and one endpoint never publishes to a remote owner. The seam that
would fix it is the marker's shape — a count of endpoints rather than a single endpoint — and
this spec does not build it. Assert the per-tick sequence against `reload_status()` directly;
assert only "every endpoint publishes at least once, cadence-quantized" against the two
producers.

Neither producer needs a new input — both already read `reload_status()` off the component —
and no slot is added, so the replicated schema and its
content-derived fingerprint are untouched. No primitive registration changes either, so
unlike Task 3 this task regenerates no SDK typedefs. `docs/scripting-reference.md` carries
**no** description of `player.reloadProgress` or `player.reloadActive` today — the slots are
undocumented there. Add one, following that file's "The readonly `player.health` slot"
section as the precedent for shape and voice: engine-owned, read-only from scripts, what
the engine publishes into it, and here the step-scoped meaning of progress against the
whole-reload meaning of active. Update the engine-state catalog comments beside the two
slot writes to match.

### Task 6: Dev content and the cross-vantage test suite

Author `content/dev/scripts/reference-shotgun.ts` — `canonicalName "reference_shotgun"`,
`fireMode: "semi"`, `resolution: "hitscan"`, `damage: 12` (matching the reference pistol so
the `target_dummy` three-shot demo math in `content/dev/scripts/target-dummy.ts` stays
correct), `fireRateMs: 700` (slow, against the pistol's 180, so a shot is visibly a shot),
`thirdPersonModel` and `viewmodel` both set to
`models/smg/model.gltf` as the pistol sets them — omitting `viewmodel` ships the dev default
with nothing in first person, the visible regression AC 18 gates on, and omitting
`thirdPersonModel` fails AC 19 — an
ammo resource with `reloadStyle: "perShell"`, `type: "shells.buck"`, a magazine
of 8, `reserve: 32`, and `reloadMs: 450` as the per-shell step — and register it
from `content/dev/start-script.ts` beside the pistol. Those three numbers are what the AC 18
walkthrough rests on: a 700 ms fire rate makes the mid-loop cancel observable, a reserve of
32 covers four full reloads, and a 450 ms step makes the magazine climb visibly rather than
snapping. `shells.buck` is the identifier the archetype, both dev ammo grants, and the
`combat-demo` README must all agree on — it appears nowhere in the codebase today, so it is
introduced by this task and must be spelled identically at every site. Point
`content/dev/scripts/player.ts`'s `defaultWeapon` at it, replacing `reference_pistol`, so
the per-shell loop is demoable without switching or pickups. That flip is decided, not
proposed; it is the only production consumer of the per-shell path in this spec, and it is
one line to revert. It also makes a co-op mispredict the default dev configuration —
accepted, because the rollback path is the shipped `ShotVerdict` one and is correct. Do not
re-litigate it here or
add prediction machinery to compensate. Add an explicit `reloadStyle: "magazine"` to
`content/dev/scripts/reference-pistol.ts` so both classifier values appear in dev content;
the pistol archetype stays registered.

The flip strands the dev mod's ammo grants and its combat prose, and the shotgun keeps its
own ammo `type` — a shared
pool would make the type meaningless — so the content moves rather than the descriptor.
Retarget both ammo grants to `shells.buck`, each together with the comment that
describes it: `content/dev/scripts/combat-demo-reaction.ts`'s
24-round `ammoPickup` trigger reaction, whose file-header item 3 names the same 24-round
`bullets.light` grant, and `content/dev/scripts/combat-lifecycle.ts`'s
8-round `dev:ammo-on-kill` payout. Both currently credit `bullets.light`, which no equipped
weapon would draw from. Then update the prose that names the pistol as the
equipped weapon: `content/dev/scripts/target-dummy.ts`'s header comment,
`content/dev/scripts/combat-lifecycle.ts`'s finisher-overshoot comment, and
`content/dev/maps/combat-demo.README.md`. In the README, update every mention of the
stranded ammo type and every prose reference to the pistol as the equipped weapon — do not
work to a site count, the two are interleaved. Its shot-count walkthroughs survive
unchanged because the shotgun matches the pistol's `damage: 12`, and so does the
three-shot dummy math. One intended addition rather than a discovery: a magazine of 8
against the pistol's 12 means the walkthrough runs dry between engagements rather than
inside one — the `reference_enemy` takes six shots and a dummy four, both inside a
magazine, where the pistol carried the pair on one — so the README gains a reload beat
between them.

Add the test suite spanning four vantages: **V1** the single-player / listen-host local
pawn, **V2** the host-simulated remote pawn, **V3** the connected client's local
prediction, and **V4** the owner-private replication projection. Every fixture is
constructed in-test — no test reads dev content or assumes which archetype is the dev
`defaultWeapon`; verify by running the suite with `defaultWeapon` set to each of the two
archetypes in turn.

Descriptor layer: `reloadStyle` parses, defaults, and rejects
identically in the QuickJS and Luau runtimes, and appears in the generated SDK typedefs.

Machine layer — one test per transition, seven of them:
materialization landing `Idle`; `Idle → Reloading` on a reload edge with style `magazine`
and guards passing; `Idle → ShellLoading` on the same edge with style `perShell`;
`Reloading → Idle` on expiry with one atomic transfer; `ShellLoading → ShellLoading` on
expiry, crediting one round and continuing; `ShellLoading → Idle` on expiry with the
magazine full or the reserve empty; and `ShellLoading → Idle` on an authorized fire, the
in-flight shell forfeited. The two `magazine` arms are not optional — they are the shipped
behavior AC 2 protects, re-asserted against the new state names. Five more sit here rather
than with a transition: a pawnless weapon firing and cooling normally while refusing a
reload press silently, emitting no delivery (AC 17); the `take → 0` ending, driven from
a fabricated `ShellLoading` fixture whose reserve is zeroed between the loop-continue check
and the credit (AC 4); a reload starting while the weapon is cooling, with the cooldown
neither blocked nor reset (AC 8); a `perShell` reload press from `Idle` blocked by a full
magazine, emitting blocked-full and starting no loop; and one blocked by an empty reserve,
emitting blocked-empty and starting no loop (AC 5) — the `magazine` half of AC 5 rides on
shipped tests, the `perShell` half is new. The matrix owns the pawnless *expiry* and the
held-edge cases; the pawnless entry here is the refusal itself.

Ordering layer: one test per row of the ordering matrix, which owns those cases — do not
restate them here and do not write them from the ACs, since the matrix is what the ACs were
reconciled against. Two more the matrix does not cover: the loop still *running* and the
verdict still `Rejected` after a suppressed trigger pull, for both of AC 7's triggers —
cooldown not elapsed as well as magazine below `costPerShot` — since row 6 asserts the
below-cost verdict but not the surviving loop, and no row covers the cooldown trigger at
all; and step cadence holding over many steps at a `reloadMs` that is not a tick multiple,
where row 15 pins one tick's worth.

Hot-reload layer: state, timers, cumulative credited
count, magazine, and edge
flags preserved mid-`Reloading` and mid-`ShellLoading`; a style flip in each direction,
`perShell` → `magazine` ending the reload after the in-flight step and
`magazine` → `perShell` mid-`Reloading` completing atomically, each landing in one refresh;
a mid-loop refresh leaving the eventual
`reload_completed`'s `transferred` unchanged.

V2 layer — AC 10 lists four claims, one test each: that pawn crediting one round per
per-shell step, cancelling a `ShellLoading` loop on an authorized shot, observing the
`Reloading` lockout, and reaching the same magazine and reserve counts as the local pawn
from the same command sequence. The equivalence test discharges the last claim only — it
compares end states and would pass on a remote path that credited N rounds in one
transfer.

V3, the prediction seam — **the client half is already shipped and tested.**
`shot_verdict_reject_rolls_back_local_presentation_and_cooldown` and
`stale_reject_does_not_overwrite_fresh_authoritative_cooldown` in
`crates/postretro/src/weapon/mod.rs` already cover `apply_verdict`'s rollback and its
`cooldown_authority_generation` guard; do not reimplement them. The new work is the
**host-refusal half**: `run_remote_weapon_commands` mints no `AuthorizedShot` for a remote
command arriving mid-`Reloading`, and none for one arriving mid-`ShellLoading` while the
host magazine is still below `costPerShot`. Assert on the absent shot at the host, which is
unconditional. If a test also walks the client rollback, the cooldown-restore half is
conditional — `apply_verdict` restores `cooldown_before_ms` only while
`cooldown_authority_generation` still matches, and `reconcile_cooldown` bumps it on every
fresh authoritative sample — so pin the generation by interleaving no `reconcile_cooldown`
across the in-flight window, or assert the guard rather than the restored value.

V4 / HUD layer, split the way AC 12 is: against `reload_status()` per tick, `active` not
blinking across a step boundary and `progress` following Task 5's per-step sample sequence;
against the two producers, `player.ammo` rising by the shells credited since the last
publication and the owner-private projection carrying every endpoint at least once for the
pawn's own values, cadence-quantized. Do not assert a producer sample per tick — the
producers run per frame.

Close the epic by discharging AC 14, the spec's sole extension-openness guarantee, on a
throwaway branch: add a placeholder timed `WieldableState` variant, run
`cargo check -p postretro-entities -p postretro`, capture the resulting error list, and
confirm it names only the transition function and the three legality predicates — no timer
field, no change to `ReloadOutcome::event_name`, no fifth site. Record that error list in the
epic's completion notes. Discard the branch; it is the verification, not a deliverable. This
lands in Task 6 because Task 5 is the last task to touch the predicates.

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
| Reload style, component copy | `WeaponAmmoTuning::reload_style` | `"reload_style"` — `WeaponAmmoTuning` carries no `rename_all`, so its fields persist snake_case; component-local only | n/a | n/a | n/a |
| Atomic style value | `ReloadStyle::Magazine` (default) | `"magazine"` | `"magazine"` | same | n/a |
| Per-shell style value | `ReloadStyle::PerShell` | `"perShell"` | `"perShell"` | same | n/a |
| Step duration (reused) | `AmmoResource::reload_ms` | `"reloadMs"` | `resource.reloadMs` | same | n/a |
| Wieldable state | `WeaponComponent::state` (`WieldableState`) | `#[serde(default)]`, component-local only | n/a | n/a | n/a |
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
