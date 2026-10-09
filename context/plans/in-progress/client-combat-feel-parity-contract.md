# Client combat-feel parity — design contract

Every track reads this file before its own brief. Amended by the integrator only.

## Goal

A connected client's combat should read the way the listen host's does. Two playtest symptoms: clients never see the impact burst at hitscan contacts, and a client's plasma bolts vanish part-way through flight during a steady stream while the host's fly their full range. Fix both without widening the wire or weakening FIRE authority.

## Decisions

1. **Impact bursts at every vantage.** The built-in impact burst (`spawn_impact_effect_at`, the spark particles) appears once per contact on every peer that sees the shot. Today only the firing simulation's own path spawns it. Consequence: four routes gain a burst — the client's own predicted hitscan, the client observing another peer's hitscan or projectile contact, and the host observing a client's hitscan or direct-projectile contact.
2. **Observers burst from the observer impact cue.** The reliable `WeaponCueKind::Impact` cue already reaches every non-firing participating peer, carrying each contact's point and normal. Non-firing clients spawn bursts from its `Emitter::Contacts`. Consequence: no wire field, message kind, or version bump.
3. **The splash world-point presentation spawn stops being a burst route.** Splash detonations already raise an impact cue, so the `BUILTIN_SPLASH_IMPACT_TEMPLATE_ID` burst would double the cue's. Observers take the splash burst from the cue only. If retiring that burst leaves its producer and template id with no remaining consumer, remove them in the same change.
4. **The host bursts remote contacts at ingest.** Where host HIT ingestion raises the remote shot's one `impact` emission, it also spawns one burst per validated contact. Remote splash already spawns its burst there and must not gain a second.
5. **A client's own hitscan burst follows the `impact` address.** It plays from predicted contacts only under `ClientPullPresentation::Fire`, the same gate that raises `impact`. A dry or silent pull shows none. A later FIRE or HIT rejection does not retract it, just as the impact sound is not retracted. The burst lasts about 0.18 s, so it usually ends before the verdict arrives.
6. **The plasma cutoff is a general input-playout fault, not a plasma fault.** Reproduced: when a lost packet or a host hitch stalls the reliable Input stream, the host's catch-up trim keeps only the newest commands and jumps its cursor. Client-named activation starts inside the trimmed range are never seen, so the shot is denied later at HIT. Starts that survive land too few host ticks after the previous shot, and the host refuses them as still cooling, so the client deletes the bolt about one round trip into flight. It affects every weapon shape: hold and press, projectile and hitscan, primary and secondary. Hitscan loses its muzzle flash, hitmarker and damage. The host player never takes this path.
7. **Cadence is judged by the client's spacing, capped by host time** (owner). A client start is cadence-eligible when its client tick is at least the weapon's recovery after the client tick at which the previous execution's recovery began. It must also arrive no earlier than host time since that recovery began, plus a 150 ms tolerance, would allow. Consequence: over any host window, authorized shots stay at or below window ÷ recovery plus a constant, so stamping client ticks cannot raise the fire rate. This mirrors the charge rule already in `networking.md`.
8. **Starts survive the playout trim and stale-drop** (owner). They are retained and delivered in order through the independent edge lane that release and cancel edges already use. Admission then applies the decision-7 rule, the existing per-client bound of 64, and the 2 s expiry. Consequence: the `networking.md` rule "A start discarded by playout cannot become a delayed activation" is revised. Observers see a late-admitted shot late, which is acceptable.
9. **Use and drop presses survive the trim** (owner). They are retained as rising edges through the same kind of lane reload presses already use, and delivered once. Movement trimming is unchanged.
Track B landed (2c29fad): a start arriving during a live execution now waits in the lane rather than being refused, and an admitted execution's outcome may only shorten the client's predicted recovery. Both are recorded in `networking.md`.
10. **The client's recovery reset stops lagging the host's cadence.** Today an outcome resets the client's predicted recovery to the host's remaining value without subtracting transit time, so a connected client fires slower than the host player. The client's predicted cadence at the authored rate must match the host player's and must never run ahead of decision 7's admission. Integrator's call, inside the owner's parity goal.

11. **Cadence records are per weapon** (owner, after landing review). Today one record per client covers only its last weapon. An A→B→A switch falls back to the host's own clock for A and can refuse an on-time A start after lag. A weapon passed between players can carry a stale record that skips one host cooldown. Each weapon the client holds keeps its own record, keyed by weapon and firing slot, and it is cleared when that weapon leaves the client's inventory (drop, hand-over, despawn). Consequence: swap-heavy play gets steady-hold lag tolerance, and the hand-over skip closes. The window bound applies per weapon; across weapons the bound is the sum, as before.
12. **A late-admitted start fires along its own aim** (owner). A retained start captures the aim of the command that carried it at intake and fires the shot with that aim, not the aim of the later command that delivers it. Only the shot's fire origin and direction use it; the pawn's movement facing and that tick's movement are untouched. Consequence: after a stall, a client's projectile and hitscan land where its prediction showed them. Host FIRE still reconstructs origin and direction against the live pawn and world as today; only the aim input changes.
13. **Silent drop at lane overflow stays** (owner). A start dropped because 64 are already retained, or a replay of a settled start, gets no `InitiationRejected`. It mints nothing. Documented as a known edge in `networking.md`, not fixed.

## Invariants

- **Exactly one burst per contact per peer.** The firing peer's own simulation spawns it: host-local sim, or client prediction (`advance_predicted` already does so for projectiles). Every other peer spawns it from one route only. Hard: a double burst is the regression this contract most expects. Tests assert counts of `ParticleState` entities per contact (`IMPACT_PARTICLE_COUNT` per burst), not mere presence.
- **Particles never cross the wire.** Bursts are client-local cosmetics spawned from facts that already arrive.
- **No wire or version changes in Track A.** `WIRE_VERSION`, `SNAPSHOT_VERSION`, the application-protocol constant, and the tuning epoch are unchanged by it. Track B should avoid a wire change too. If one proves necessary, it bumps versions under `networking.md` §Version gates and the track reports it.
- **Tick domains are explicit.** Every recovery or spacing value in Track B names its domain, client ticks or host ticks. Recovery in ticks is `ceil(recovery_ms / tick_ms)` at 60 Hz, matching the client's predicted cooldown, which counts down by `dt` until it reaches zero or below. Client-tick comparisons are wrap-aware serial arithmetic, like `client_tick_le`.
- **A refused or expired start mints no authorized shot**, and a retained start is still refused if decision 7 is not met when it is admitted.
- **FIRE stays host-authoritative.** A refused fire mints no authorized shot and can bind no damage (`networking.md` §`shot_id`). No track may weaken that.
- **The burst normal is the contact's normal.** A zero or non-finite normal takes `impact_frame`'s existing fallback. Only `Emitter::Contacts` anchors spawn bursts; an entity-anchored cue has no contact and spawns none.
- **Layering.** `postretro-net` stays postretro-free, and the binary never reaches below its crates' public surface by copying logic. `layering_invariants_hold` enforces direction.
- Every agent reads `context/lib/context_style_guide.md` and `context/lib/development_guide.md` §2. New logic goes in a focused module rather than growing `crates/postretro/src/main.rs` or `crates/netcode/src/lib.rs`.

## Tracks and file ownership

**Track A — impact parity.** Owns impact-burst spawning on every route above: the client's own fire path (`crates/postretro/src/client_weapon/`), observer cue delivery in the binary, host HIT ingestion's remote contact presentation in `crates/netcode`, the splash world-point burst route, and a shared burst-for-contacts helper in `crates/sim/src/weapon/impact.rs` if one earns its place. Does not touch activation admission, cooldown, or verdict logic.

**Track B — activation cadence and playout edges.** Owns host command playout and edge retention (`crates/netcode/src/command_queue.rs`, `activation_edges.rs`, `activation_ledger.rs`), host activation admission (`crates/postretro/src/host_activations.rs`, and the shared weapon machine in `crates/sim/src/weapon/` only where admission must take a client-domain recovery), and client recovery reconcile (`crates/postretro/src/client_weapon/reconcile.rs`). Owns the conditioned harness tests in `crates/postretro/src/host_activations/conditioned_tests.rs`. Does not touch impact presentation. Track B does not edit `context/lib/`; the integrator folds its results in.

## Acceptance

Track A:
- A client predicted hitscan `Fire` with N world and entity contacts spawns N bursts. `DryFire` and `Silent` spawn none.
- A client receiving an impact cue with N contacts spawns N bursts. The firing owner never receives its own cue and spawns no observer burst.
- The host ingesting a validated remote hitscan declaration with N contacts spawns N bursts. A remote direct-projectile contact spawns one. A remote splash contact spawns exactly one.
- An observer of a splash detonation spawns exactly one burst.
- `cargo test -p <crate> <filter>` for each new test reports a non-zero passed count.
- `git diff main --stat -- crates/net/` shows no change to `wire.rs` constants.

Track B:
- The four reproduction tests in `conditioned_tests.rs` pass: `conditioned_steady_hold_keeps_every_resourced_predicted_bolt_to_its_natural_end`, `conditioned_steady_hold_host_hitch_on_clean_link_keeps_resourced_bolts`, `conditioned_steady_hitscan_hold_is_never_refused_with_ample_ammo`, and `conditioned_rapid_press_taps_are_never_refused_with_ample_ammo`. Their assertions are not weakened.
- `conditioned_steady_hold_reference_cell_drain_refuses_only_for_resource` still passes: real resource exhaustion still refuses.
- A clean-link hold at the authored rate authorizes the same shot count over 6 s as the host player's own weapon (45 at 130 ms).
- A new test shows a client that stamps its commands faster than recovery cannot exceed window ÷ recovery plus the decided constant in authorized shots.
- Use and drop presses inside a trimmed range are each delivered exactly once.
- `cargo test -p postretro --bin postretro conditioned_` and the focused `postretro-netcode` command-queue and activation tests pass, with counts reported.

## Open questions

- **Burst (multi-step) restarts and the rocket** are expected to be covered by decisions 7 and 8 but were not reproduced directly. Track B confirms or reports.
- **Impact producers without a cue.** Emissions with no `shot_id` publish no observer cue. Track A reports any such producer that reaches observers (enemy fire is the candidate) rather than adding a wire route.
