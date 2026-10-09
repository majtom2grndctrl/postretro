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
6. **Plasma bolts: open — see Open questions.**

## Invariants

- **Exactly one burst per contact per peer.** The firing peer's own simulation spawns it: host-local sim, or client prediction (`advance_predicted` already does so for projectiles). Every other peer spawns it from one route only. Hard: a double burst is the regression this contract most expects. Tests assert counts of `ParticleState` entities per contact (`IMPACT_PARTICLE_COUNT` per burst), not mere presence.
- **Particles never cross the wire.** Bursts are client-local cosmetics spawned from facts that already arrive.
- **No wire or version changes.** `WIRE_VERSION`, `SNAPSHOT_VERSION`, the application-protocol constant, and the tuning epoch are unchanged. Gate: `git diff main -- crates/net/src/wire.rs` shows no layout or constant change.
- **FIRE stays host-authoritative.** A refused fire mints no authorized shot and can bind no damage (`networking.md` §`shot_id`). No track may weaken that.
- **The burst normal is the contact's normal.** A zero or non-finite normal takes `impact_frame`'s existing fallback. Only `Emitter::Contacts` anchors spawn bursts; an entity-anchored cue has no contact and spawns none.
- **Layering.** `postretro-net` stays postretro-free, and the binary never reaches below its crates' public surface by copying logic. `layering_invariants_hold` enforces direction.
- Every agent reads `context/lib/context_style_guide.md` and `context/lib/development_guide.md` §2. New logic goes in a focused module rather than growing `crates/postretro/src/main.rs` or `crates/netcode/src/lib.rs`.

## Tracks and file ownership

**Track A — impact parity.** Owns impact-burst spawning on every route above: the client's own fire path (`crates/postretro/src/client_weapon/`), observer cue delivery in the binary, host HIT ingestion's remote contact presentation in `crates/netcode`, the splash world-point burst route, and a shared burst-for-contacts helper in `crates/sim/src/weapon/impact.rs` if one earns its place. Does not touch activation admission, cooldown, or verdict logic.

**Track B — plasma bolt survival.** Brief pending the reproduction. Expected to own host activation admission or cadence in `crates/netcode` and `crates/sim/src/weapon/`, plus the client outcome handling that despawns predicted projectiles.

## Acceptance

Track A:
- A client predicted hitscan `Fire` with N world and entity contacts spawns N bursts. `DryFire` and `Silent` spawn none.
- A client receiving an impact cue with N contacts spawns N bursts. The firing owner never receives its own cue and spawns no observer burst.
- The host ingesting a validated remote hitscan declaration with N contacts spawns N bursts. A remote direct-projectile contact spawns one. A remote splash contact spawns exactly one.
- An observer of a splash detonation spawns exactly one burst.
- `cargo test -p <crate> <filter>` for each new test reports a non-zero passed count.
- `git diff main --stat -- crates/net/` shows no change to `wire.rs` constants.

Track B: pending.

## Open questions

- **Plasma root cause.** Leading hypothesis: during a steady hold, the host refuses some hold restarts that the client issued on time by its own tick clock. Committed waits advance once per host tick, while command playout can deliver several client commands in one host tick. The client then despawns the refused activation's bolt when the outcome arrives, about one round trip into flight. A reproduction is in progress; the fix direction is an owner decision once it is confirmed.
- **Impact producers without a cue.** Emissions with no `shot_id` publish no observer cue. Track A reports any such producer that reaches observers (enemy fire is the candidate) rather than adding a wire route.
