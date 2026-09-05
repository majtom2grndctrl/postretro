# Networking

> **Read this when:** working on multiplayer, replication, the wire/codec format, the netcode transport, or the host/client role model.
> **Key invariant:** the net crate is registry-blind — it moves typed snapshots and never mutates entity state. `crate::netcode` (engine) is the sole replication path that touches the `EntityRegistry`.
> **Related:** [Architecture Index](./index.md) · [Entity Model](./entity_model.md) §6 · [Development Guide](./development_guide.md) §4.2 · [Scripting](./scripting.md) §11

---

Authoritative client-server co-op uses client-side prediction and reconciliation. General-purpose multiplayer is a non-goal (see `index.md` §4).

## Crate boundary and ownership

Netcode lives in the `postretro-net` crate (`crates/net/`): the wire codec, the polled transport, the wire-side replication and state-slot trackers, time sync, and the dev-only latency harness. The dependency arrow points **one way** — `postretro → postretro-net`. The net crate never depends on the engine.

`postretro-net` is **glam-free and postretro-free by construction.** Wire types use plain `[f32; N]` / `f32` / `bool` — never glam or engine types. The crate is never handed an `EntityRegistry` and has no notion of entities, components, or game state. It moves opaque, typed messages.

The engine owns the other half of the contract in `crate::netcode`. That module is the *only* engine code that touches the registry on behalf of replication, and it owns everything that must know both sides: the role model, the `NetworkId↔EntityId` maps, and the wire↔engine type conversions (the glam-aware `Transform`↔`WireTransform` translation, the `ComponentKind→u16` mapping). The split is deliberate: the net crate stays a reusable, engine-agnostic transport, and all registry mutation stays in game logic.

## Transport contract — polled, non-blocking

The transport is **synchronous and frame-polled** — no async runtime, no tokio, no spawned threads. It builds on renet 2.0 + renet_netcode over a non-blocking `std::net::UdpSocket`. The crate pins `default-features = false` on both renet deps specifically to keep the dependency tree free of tokio/async-std/smol; `cargo tree` verifies the no-async-runtime invariant.

The caller advances the transport once per frame: it drives renet, drains the socket to `WouldBlock`, processes events, and flushes outbound packets — then returns. It never blocks. This honors the event-loop ownership invariant (`development_guide.md` §4.2): winit owns the loop, and the netcode poll slots into the frame's Game-logic stage without stalling it.

**Every frame, not every gameplay frame.** The poll is keyed on endpoint presence, not on boot state: a frame that runs no world — frontend, a level load in flight, a resumed splash — still advances the transport. Two things depend on that. A level install longer than the netcode timeout would otherwise drop every peer mid-load, and a client must be joinable before either peer has a level installed at all, which is the ordinary state between maps. A world-less poll does transport advance, keepalive, gate evaluation, and the reliable control drain — never snapshot apply, state-crossing detection, or a simulation tick, none of which mean anything without a world. One limit is honest and accepted: a suspension longer than the timeout drops peers regardless, because no frames run at all while suspended. The rule is only that no *running* frame leaves a live endpoint unpolled.

renet 2.0 separates two layers, and the transport wraps both: a **connection layer** (owns channels, produces/consumes opaque packet payloads) and a **netcode transport** (encrypts payloads, moves them over UDP). The connection-layer packet I/O is also re-exposed directly so the in-memory harness can drive the same payloads without a socket.

## Channel model

Four channels, fixed layout, agreed by both peers (the layout is folded into the protocol gate, so it cannot drift between versions):

| Channel | Delivery | Carries |
|---------|----------|---------|
| Control | reliable-ordered | join traffic both ways: compatibility declarations and join seed client→server; level changes, divergence causes, and the replicated tuning payload server→client |
| Snapshot | unreliable | server snapshots: entity records, state-slot records, server tick metadata |
| Input | reliable-ordered | client input commands, replication acks, baseline-refresh requests, state-refresh requests, time-sync probes |
| Presentation | unreliable | host-addressed passive presentation events (damage numbers and future cosmetic facts) |

Reliability is matched to the data: control state and client→server repair/ack traffic must arrive ordered; snapshots are disposable because missing entity or state baselines are repaired by explicit refresh requests.
Presentation is a separate event lane: it is addressed to one client, fire-and-forget,
and never participates in ack, resend, reconciliation, or participation-epoch
bookkeeping. A lost packet simply produces no cosmetic, and a late joiner receives no
buffered event.

## Wire/codec invariants

The wire codec is **bitcode**, pinned to an exact version. bitcode owns endianness and bit-packing — wire types do no manual byte layout. Two hard rules follow from bitcode's unstable byte format across majors:

1. **Never persist bitcode bytes.** The format is not a storage format. It exists only between two live, version-matched peers.
2. **Every connection is gated on the handshake** before any bitcode payload is decoded (see *Handshake* below). A version-mismatched peer is refused before a single message is interpreted.

**No serde-internally-tagged enum crosses the wire.** The engine's `ComponentValue` is a `#[serde(tag = "kind")]` enum, which bitcode cannot round-trip (`DeserializeAnyNotSupported`). So replication does not send engine types: it sends dedicated **wire-mirror** types that derive bitcode's native `Encode`/`Decode`. The component payload carries an explicit `u16` discriminant **numeric-equal to the engine `ComponentKind`**. Current entity payloads cover `Transform`, `PlayerMovementState`, `MeshAnimationState`, and `KinematicMoverState`. `MeshAnimationState` carries only current state name; descriptor mesh data stays local. The engine↔wire conversion lives in `crate::netcode`; the mirror types know nothing about glam component order or serde tags.

This discriminant equality is a load-bearing contract across the crate boundary: the net side and the engine side independently assert it (drift-guard tests on both sides), because a divergence silently mis-tags components on the wire. New payload variants are added in engine `ComponentKind` numeric order.

**Snapshot envelope:** server tick metadata plus bitcode length-prefixed record lists. Entity records are per-client replication records: `FullBaseline`, `Delta`, or `Despawn`. `FullBaseline` establishes or refreshes the client's per-entity baseline; `Delta` applies only against the named baseline; `Despawn` is a tombstone and carries no components. State-slot records follow the same baseline/delta repair model for non-entity replicated state. Empty record lists are valid.

`Despawn.reason == 1` is reserved for presentation-only projectile contact.
It does not change gameplay authority or damage. A client that already mapped the
descriptor-backed projectile retires it and materializes that descriptor's impact
flash at the last applied Transform. Default reason 0 remains ordinary retirement,
including travel/range expiry, and produces no flash. The server retains a terminal
projectile Transform until every intended recipient acknowledges its current
baseline, then sends the durable tombstone. Attempting one unreliable endpoint
snapshot is not delivery. Excluded recipients never mapped the visual and do not
hold this acknowledgment gate open.

Clients acknowledge replication progress over the reliable Input channel. Acks are monotonic and additive: omitted entities or state slots leave prior server-side ack state intact. A client that receives a delta for an unknown baseline does not guess; it requests a full baseline refresh and waits for repair.

State-slot baseline ids use one non-recycled namespace for the server endpoint's
lifetime. A schema rebuild retires earlier ids and clears per-client state without
restarting the allocator. A delayed pre-rebuild ack therefore cannot suppress a
fresh baseline for a rebuilt slot, even when participation itself did not change.

Shared state-slot replication serializes one retained global scalar to every client.
Per-owner mod slots therefore use owner-private replication or stay host-local; they
never enter the shared-global scope.

The codec surface is two functions (encode, decode) over these types. Decode of a short, corrupted, or over-long buffer is always a typed `Err`, never a panic — the transport must survive a hostile or truncated packet.

**One payload is opaque to the net crate and variable-length.** The replicated tuning values (see *What gates, and what replicates instead*) cross as bytes the crate never decodes, compares, or validates; every other opaque value on the wire is a fixed-size digest. A typed mirror would make the crate learn the engine's descriptor vocabulary, breaking the registry-blindness the whole boundary rests on — so the payload is engine-serialized at both ends and the crate is a courier. The cost is real and accepted: a malformed payload is the engine's to detect, because the crate cannot validate what it forwards.

## Two-gate handshake

Version compatibility is enforced **twice**, because the two gates catch different failures at different layers. Both gates derive from the same two build constants — an app protocol id and a wire-format version. The app id bumps when the message *vocabulary* changes (a new control message, a changed channel layout); the wire version bumps when any wire type's bitcode byte layout changes (added field, reordered enum, bumped bitcode major).

**Gate 1 — transport `protocol_id` (u64).** Both constants are packed into the netcode `protocol_id`. A peer whose `(protocol_id, wire_version)` pair differs fails the *encrypted netcode handshake itself* — the connection never establishes. This catches wire-incompatible peers before any app code runs.

**Gate 2 — the app gate**, carried over the reliable Control channel and evaluated in two stages, below. It proves app compatibility and shared content before prediction can arm.

The two gates are not redundant. Gate 1 stops wire-incompatible peers cheaply at the encryption layer, before a single bitcode payload is interpreted. Gate 2 reasons about content, which the encryption layer cannot see. **No entity state is sent or applied to a client that has not cleared both** — the snapshot send path refuses it.

### Admission and content parity

The app gate splits by **mutability**, not by subject. A value belongs to the earlier stage only if a mismatch on it can *never* later become a match.

**Admission** carries what cannot change for a live connection: the two build constants and the mod's declared id. Nothing can make a mismatch here true later, so a mismatch is terminal — the slot closes immediately, the typed cause is sent reliably, and transport teardown waits for its acknowledgement. Without that delivery gate a player on the wrong mod cannot distinguish a refusal from an unreachable host.

**Content parity** carries everything derived from loaded content: a mod compatibility digest, the identity of the installed level, and that level's content digest. Every one of these is *designed* to become true later — a level digest at the next install, a mod digest at the next reload. So a parity mismatch **never closes the connection**. It holds the slot below participating, names which of the three diverged, and clears itself when the values agree, whichever peer moved.

This overturns the earlier rule that a connection was bound to its content fingerprint for its lifetime, with a content change closing it. **Content divergence is a diagnostic to a still-connected peer, not a disconnect.** Closing would also race the design's own timing: a client's declaration for one level can still be in flight when the host installs the next, so a host that closed on mismatch would tear down a peer it had already demoted a frame earlier.

Putting a mutable value in admission is the failure this split exists to prevent — it converts a recoverable content difference into an unrecoverable disconnect. That is the question to ask of any value added later.

The two stages queue independently. Each is evaluated once the value it compares against is installed, and the reliable channel is what holds a message that arrives early — neither stage waits on the other, and coupling them re-creates the ordering inversion where a peer cannot join until a map exists.

## Slot lifecycle

A connection moves through four stages: **pending** (connected, nothing proven), **admitted** (the immutable values match — the connection is live and receives no entity state), **participating** (content parity holds; snapshots flow), and **closed**. Only a participating slot receives entity records.

**Participation is a predicate, not a pair of transitions.** A slot participates if and only if its last declaration matches the host's currently installed parity values, re-evaluated for every slot after every parity source is reinstalled. Demotion and promotion are two readings of one comparison, and there is one place that comparison happens — a later parity source cannot implement half of it.

Specifying this as transitions is the trap, and the level path hides it. A client re-declares at every level install, so a demote-only implementation looks correct there. The mod-digest path has no re-declaration and therefore no recovery: a slot held by a host-side reload would stay held forever even after the host reverted the edit, because the client's declaration never moves and it has no reason to re-send.

Two rules derive every per-slot effect from the state pair rather than from whichever method ran:

- **Any exit from participating clears that slot's state** — pawn, replication, ownership, command, state-slot, and combat — whatever the destination. A demotion clears exactly what a close clears because both are the same edge, and a slot demoted and then closed clears once, not twice.
- **Any entry to participating registers the slot and spawns its pawn** — first admission and re-promotion alike, so a re-promoted slot needs no special case and no "must re-emit" rider.

The connection survives; its state does not. A client id is stable within one connection but not across a rejoin — a relaunching peer arrives on a freshly minted id — so player identity keys to a durable seat, never to the connection.

**A held slot is gated in both directions.** It is sent no entity state, and its inbound traffic is drained and discarded. The drain is not an optimization: an undrained reliable channel overflows its memory budget and the transport disconnects the peer — which would break the never-close guarantee through a path that never decided anything.

**Participation traffic is generation-scoped.** Every entry to participating
allocates a new monotonic epoch for that slot. Snapshot and client Input payloads
carry it in a transport-owned frame. A holding Control frame retires the old epoch
client-side, including when no snapshot from that epoch arrived. Both peers drop
traffic outside the current epoch. This prevents a delayed snapshot from restoring
retired client state, and prevents old Input from reaching a newly spawned pawn
after re-promotion. A transport-only reliable marker arms the new epoch; it is not
an engine promotion message.

**The host names the next map; clients follow.** A level change demotes every participating slot rather than closing it, and the host sends the next map's catalog id over Control. A late joiner is told the current map on admission, so it does not wait for the next transition. Map authority is server-owned; a client never asks for a level change. A host running an uncatalogued level sends nothing — a catalog is the only namespace in which one string resolves on both peers — and its clients stay admitted until they install a matching level themselves.

A held slot is bounded by the transport, not by the gate: a peer that never reaches parity holds an admitted slot for as long as its keepalive survives. Accepted, because the case it covers is a peer on the *right* mod whose content diverged, which is recoverable by construction. A genuinely wrong mod still closes, on the id, at admission.

## What gates, and what replicates instead

**Hash only what cannot be replicated.** A digest is a fallback, not a first instrument: replication makes two peers *agree*, where a digest only lets them refuse each other. Every value a client simulates against that the host can send is sent — at the participation transition the host resolves that slot's pawn tuning and the client installs it instead of reading its own registry.

The client predicts with the host's numbers, never its own, and the sites that resolve tuning keep **no fallback to the local registry** for a replicated value. A fallback fires only on the peers whose content differs, which is precisely the case replication exists to fix. This is a behavior semantic, not just a mechanism: a modder testing a movement change in co-op sees the host's values, not their own. First-person weapon placement also rides this payload because later fire authority consumes it. Pure render feel stays local, so a player's own view-feel settings survive a join.

What stays hashed is what replication cannot reach: a *computation* both peers run independently over the same replicated state, and content too large to send. Reaching for a hash on a value the host could have sent produces a false refusal — the mistake a later widening is most likely to make.

**The covered set is partial by decision, and the gap is named.** Script-declared reaction and event lanes are not hashed. One carries runtime allocation handles rather than content; in the other, whether a declaration is prediction-relevant is keyed by an open string namespace, so no compile-time guarantee is reachable for "someone added a new prediction-relevant primitive." Both escapes are mechanisms rejected elsewhere: hashing the lanes wholesale demotes every peer when a sound argument changes, and hashing a chosen subset is the same fail-open pattern that once left static collision geometry unchecked. Level-local script content, declared at level setup, falls between the two digests' schedules and is uncovered for a third reason — neither digest looks there. Two mods differing only in these lanes pass both stages and can diverge on locally-simulated state. A total-coverage claim would be false.

Within a type the digest does reach, the recipe is a **denylist**: bind every field and name any skip, so a field added later is a compile error rather than a silent omission. An allowlist is what produced the fail-open above — a new field defaults to unhashed, and no test catches the one you forgot. The guarantee covers fields inside reached types, not whole lanes; a new lane still escapes, which is why the uncovered set is enumerated rather than assumed empty.

### Level identity and level content digest

Two values answering two questions. **Identity** says *which map* — the catalog id, falling back to a normalized content-root-relative path for an uncatalogued level. **The content digest** says *is the content the same*.

The digest's membership rule is one line: **a deterministic input to client prediction belongs in the hash.** That rule is what put mover identity, path, motion, carry policy, and mover collision geometry in it, and what later added the static world collision the client builds its own trimesh from — the same fail-open recurring on a second surface, because movement prediction and client-authoritative hit declaration both run against that trimesh. Anything a client *simulates against* and the host cannot send is a candidate; presentation is not.

They stay separate rather than folded together because same-map-different-content is a different diagnosis from wrong-map, and a player can act on the difference. Identity alone cannot discriminate: a catalog id is **mod-scoped**, so two mods may each declare the same id over different files. Admission closes that case, not identity.

The digest is deliberately not a hash of the compiled level bytes. That would turn a cross-platform bake difference into a hard connection failure.

### Presentation events vs. replicated state

Combat feedback the player reads and forgets — floating damage numbers and damaged-enemy health or shield facts — is **presented, not replicated**. The host sends it as transient events on a dedicated unreliable channel to the client that earned it; loss and reordering are acceptable. Enemy health and state stay host-only. Clients display the pushed facts without simulating them, so cosmetics never enter a digest or block a join.

Damaged-enemy overlays are private per recipient. The host renderer owns only
host-local feedback; each remote recipient has an independent cap and linger
lifecycle. Equal-time cap decisions use the stable non-recycled `NetworkId`, so
unordered fact arrival cannot select a different retained target set.

### Mod identity

The manifest declares a stable id and a version. **The id gates** — it is the namespace that makes a catalog id resolvable, so peers must declare the same one. **The version never gates.** It rides the same message and serves display and diagnostics only; exact-version equality would refuse a friend on the previous build over a change no client simulates. Because a gating and a non-gating value share one message, the no-compare rule is commented at the comparison site rather than left to inference.

Both are **frozen at first commit** and do not move across a hot reload, diverging from the atomic-replace discipline most manifest lanes follow. Admission is terminal, so a mid-session id change would invalidate decisions already made, with no state to demote those connections to. The compatibility digest is the opposite case and re-hashes on every reload, because parity has a recovery path.

Identity is declared, not proven — tamper resistance is a non-goal, and neither field is a security mechanism.

## Session-state ledger

State that survives a level change, enumerated rather than accreted:

- **The connection** — its id, lifecycle stage, and last parity declaration.
- **The seat** — the host-minted durable player key, its asserted claim when one exists, its carried state, and its level-independent placement-assignment cursor. A seat sits above participation and is never released by a level transition.
- **The roster** — the host's seat-keyed projection of host-minted seat ids, current connection state, and remaining fresh-seat count. Claims remain host-local for rejoin; neither player ids nor display names cross the roster wire.

The rest is defined by subtraction. Everything level-scoped and everything per-slot clears on demotion, so what survives a session is exactly what a demotion does not touch.

Seat ids are non-recycled within one session. The `u16` namespace therefore has no
recovery path after exhaustion: new remote admissions remain unavailable until a
new session starts.

The pawn a seat currently owns is one fact stored in two directions. The host's seat table keeps seat→pawn; the entity registry keeps the pawn→seat reverse index, as a sparse map parallel to the component columns — never a `ComponentKind`, which would make it a wire discriminant. That reverse index is what lets owner-addressed layers holding only a registry and an `EntityId` — impact effects, impact policy, reaction dispatch — resolve a pawn's owner. The seat table is its sole writer, through one binding call that updates both directions together, so an entry exists exactly while a seat is bound to a live pawn: a rebind clears the outgoing pawn, a rejoin hold clears the held seat's pawn, and `despawn` clears the entry.

Three constraints bind the seat wherever it lands. It sits **above** the participation lifecycle — the exit sweep clears a slot's state, never its seat, or a level change would churn the identity the seat exists to preserve. Its type belongs in `foundation`, the only crate the binary, `entities`, and a later floor-crate consumer can all name; `net` is postretro-free by contract and cannot depend on `foundation`, so a seat minted in `net` forces a duplicate the first time per-seat storage reaches the floor slot table. Seat *ids* may cross the wire as a bare integer, but seat *contents* never do — that is what keeps the transport registry-blind.

The roster publishes no lower than admitted. Admission is a compatibility gate, not a trust decision: it checks the build constants and the mod id, admits automatically, and never asks the host who the peer is. A peer below it has proven only that it can reach the socket, so it receives no roster frame — not even a seat count. Admitted and participating peers receive a status-only frame encoded separately with their own seat.

## Game-logic-owned apply invariant

The net crate emits typed snapshots and **never mutates the registry.** All registry-touching replication lives in `crate::netcode`, which owns the two halves of the data path:

- **Host serialize:** walk the authoritative replicable set, stamp each `EntityId` to its stable `NetworkId`, convert to wire mirrors, and build per-client baseline/delta/despawn records. Borrows the registry **immutably**.
- **Client apply:** apply `FullBaseline`, `Delta`, and `Despawn` through the mapped `NetworkId→EntityId` state machine. Full baselines materialize or refresh entities; deltas mutate only when the referenced baseline is held; despawns remove mapped entities idempotently and drop their mappings.

`NetworkId` is the network-stable identity assigned by the host; the host owns an `EntityId→NetworkId` allocator (monotonic, never recycled, stable for an entity's lifetime) and the client owns the inverse `NetworkId→EntityId` map. Stable ids keep the client's mapping coherent across snapshots. This is the network projection of the entity-model ownership rule (`entity_model.md` §6): game logic owns entities; replication is just another reader (host) and a controlled writer (client).

**Reload endpoint stream.** Reload endpoints cross the fixed-tick/frame boundary
through one bounded stream per weapon. HUD and owner-private projection keep
independent cursors and acknowledge only after sampling. Equal endpoints from one
simulation tick coalesce with an observable count. On overflow, the oldest retained
run is dropped and loss is observable per consumer; retained runs stay FIFO. This
bounds stale playback when authored reload cadence outruns publication.

### Snapshot apply ordering

On every client game-logic frame, apply received snapshots before state-crossing detection. Snapshot apply mints a frame-stamped `SnapshotsApplied` witness; crossing detection consumes it after game logic settles same-frame local slot writes. The witness cannot be forged or reused from a prior frame, so crossings always observe received replicated state before they inspect the slot table.

Current component payloads are `Transform`, `PlayerMovementState`, `MeshAnimationState`, and `KinematicMoverState`, added in `ComponentKind` numeric order. `PlayerMovementState` includes presentation-only `aim_pitch` for remote-avatar pose presentation. `MeshAnimationState` carries the current animation state name; descriptor mesh data stays local. `KinematicMoverState` carries phase only: `mover_id`, segment index, direction, mode, elapsed/wait milliseconds, started/completed/blocked flags, velocity, optional target segment for move-and-hold, and rotating phase (`spin_angle_rad`, pre-tick spin angle, active-at-tick-start provenance, current spin rate, target spin rate). Tick provenance lets replay reconstruct the motion that actually produced the authoritative pose when completion or a later command changed the post-tick gate. Static path, collision geometry, and spin authoring (axis, acceleration, `carry_yaw`) stay in PRL `KinematicGeometry`; the level content digest proves cross-peer parity before this phase is trusted.

Player movement grounding is a widened ground reference (`Airborne`, `World`, or `Mover(mover_id)`) rather than a bare boolean. The net crate validates enum shape, finite numeric fields, and movement-state-local numeric invariants before typed apply. A sliding floor normal must be absent or bounded and unit-length within a small squared-length tolerance. Resolving a mover id to a loaded local mover is engine-owned client apply.

Three distinct metadata validity gates apply:

- **Movement-authority metadata** (`local_player`, `last_processed_client_tick`): valid only on records carrying `PlayerMovementState`. No other record type may carry these fields.
- **Active-weapon metadata** (`active_weapon_archetype`): valid only on records carrying `PlayerMovementState`. `None` means no weapon is equipped.
- **Descriptor `entity_class`**: valid on any non-despawn entity record (`FullBaseline` or `Delta`) that carries at least one finite `Transform` payload — it no longer requires `PlayerMovementState`. On despawn records, `entity_class` (and all metadata) remains invalid.

Despawn records carry tombstone metadata only, never component payloads.

## Role model

Role is selected once at startup from CLI flags; default is **single-player with net fully inert** — no endpoint is constructed, serialize/apply never run.

| Flag | Role |
|------|------|
| *(none)* | Single-player. Net inert. |
| `--host [port]` | Listen server. Bare `--host` uses the default port. |
| `--connect <ip:port>` | Client connecting to an explicit address. |

**No endpoint, no gate.** Single-player constructs no endpoint, so no compatibility value is computed, no gate runs, and a level change announces nothing. Nothing branches on player count — the absent endpoint *is* the branch, which is why the compatibility digests are computed inside that check rather than unconditionally and discarded.

`--host` and `--connect` are mutually exclusive. **Direct connect only** — no discovery, no matchmaking, no relay. Network setup can fail (socket bind, transport init, hosted session-id entropy); a failure is logged and **degrades to single-player** rather than blocking boot — a netcode setup error never stops the engine from running. The local seat ledger remains available for single-player carry, but its fallback session id is never published. Clients receive host-authoritative replication, predict their own pawn locally, and reconcile against host acks.

## Testing the conditioned link

Two complementary paths exercise the netcode under loss and latency:

**In-memory harness (deterministic, unit-test path).** A dev-only packet conditioner (gated on `dev-tools`, always built under `test`) sits between an already-connected server/client pair, conditioning the *connection-level packet buffers* — bypassing the UDP transport entirely. It applies one-way delay, bounded jitter, and loss on a **virtual clock the caller advances** (it never reads wall-clock time), driven by a seeded PRNG. Same seed ⇒ same drops and arrival times, every run, every platform. This is deliberately not turmoil: turmoil conditions tokio sockets, and this path has no socket and no async runtime. It is the deterministic, reproducible unit-test path.

**`tc netem` (manual, real-socket soak path).** To shape the *real* renet_netcode UDP loopback path — the in-memory harness's real-socket complement — use Linux `tc netem` on the loopback device. Run the host and client locally over `lo`, then apply impairment:

```sh
# 80ms one-way delay, ±20ms jitter, 2% packet loss on loopback
sudo tc qdisc add dev lo root netem delay 80ms 20ms loss 2%

# Inspect the active qdisc
tc qdisc show dev lo

# Tear down — restores normal loopback
sudo tc qdisc del dev lo root netem
```

`tc netem` shapes every packet over `lo`, so it affects all local loopback traffic for the duration — apply it only for a soak session and always tear it down afterward. The in-memory harness is the deterministic automated gate; `tc netem` is the manual end-to-end soak over the real encrypted UDP path.

### Manual loopback recipe — movement prediction (host + client over `lo`)

The deterministic in-memory harness (`netcode::predict_reconcile_harness`) is the automated gate; this is its manual real-socket complement, for eyeballing the *feel* of prediction/reconciliation that automated tests cannot judge. Use a map with a descriptor-backed player pawn — `content/dev/maps/campaign-test.prl` (a `player_spawn` placement resolves to the `"player"` descriptor) — so the host materializes a real movement pawn on accept.

Run two processes locally over `lo`:

```sh
# Terminal 1 — listen host on the campaign-test map.
RUST_LOG=info cargo run -p xtask -- run content/dev/maps/campaign-test.prl --host

# Terminal 2 — client connecting back to the host's default port over loopback.
RUST_LOG=info cargo run -p xtask -- run content/dev/maps/campaign-test.prl --connect 127.0.0.1:<port>
```

Then shape the loopback link to the harness profile (45..105 ms one-way, ~5% loss) before driving the client, so the manual session matches its `LinkConfig { delay: 45, jitter: 60, loss_probability: 0.05, .. }`:

```sh
# ~75ms mean one-way delay, ±30ms jitter, 5% loss on loopback (both directions).
sudo tc qdisc add dev lo root netem delay 75ms 30ms loss 5%
# ... drive the client, observe, then ALWAYS tear down:
sudo tc qdisc del dev lo root netem
```

Verify, on the **client**:

1. **One `local_player` baseline.** The log shows the client arming prediction exactly once for its own pawn (`[Net] client <id> accepted` on the host; the client marks one pawn local). No record for any other pawn carries `local_player`.
2. **One camera-followed pawn.** The camera follows a single pawn — the marked local pawn — and never a remote one.
3. **No second local-player marker after join/disconnect.** Disconnect and rejoin the client; the host issues a fresh `NetworkId` and the client arms exactly one local pawn again. There is never a moment with two `local_player`-marked pawns.
4. **Immediate local input.** Under the shaped link, the camera-followed pawn responds to WASD/dash on the *same* fixed tick the input is sampled — it does not wait a full RTT. This is prediction working: the local pawn moves locally before the host's authoritative snapshot returns.
5. **Remote interpolation still active.** A *second* client (or the host's own pawn, viewed from the first client) moves smoothly through the interpolation buffer, not prediction — a remote pawn lags behind by the interpolation delay and is never predicted.
6. **No duplicate local pawn.** Exactly one descriptor-backed pawn exists per client. There is no provisional client-spawned pawn alongside the host-authoritative one; the local pawn is the host's pawn, mapped by `NetworkId` and reconciled in place.

Tear down the `tc netem` qdisc when finished. The shaped link affects all loopback traffic for its duration.

## Time sync

A client clock-sync exchange (`postretro-net` `timesync` module) keeps the client's estimate of the server tick. The client periodically sends a probe on the reliable Input channel; the server echoes its current tick. The client measures round-trip against **its own** monotonic clock — the server's echoed time is telemetry, never compared cross-clock, because the two origins are unrelated. A pure estimator smooths a server-tick offset and a link-jitter estimate behind an injected clock, so tests drive it on the harness's virtual clock. The interpolation buffer reads the offset and jitter to size remote-pawn interpolation delay. Registry-blind and scalar-only, like the rest of the net crate.

## Host input command queue — gap policy and bounded playout

The host holds a per-client queue of sanitized inbound `InputCommand`s and resolves
exactly one command per owned pawn per 60 Hz fixed tick, advancing a per-pawn resolved
cursor (`last_processed_client_tick`, stamped into snapshot authority metadata). Two
policies govern resolution:

- **Hold-then-neutral gap policy, frontier-gated.** When the exact next tick is missing, the
  host holds the last resolved command for up to `INPUT_HOLD_TICKS` (rides out a brief gap
  of dropped or late packets), then synthesizes neutral input (a disconnected-but-not-yet-closed client
  cannot coast on stale intent). Whether a held command **advances the resolved cursor**
  depends on whether newer data is buffered behind the hole. A held command freezes the
  cursor (no advance) **only while the buffer is EMPTY** (`pending.is_empty()` — the awaited
  tick is at the buffer *frontier*): nothing newer has arrived, so this is a genuine
  near-term late arrival (the clean-link sub-tick phase offset) worth waiting for, and the
  real command arriving within the hold grace resolves `Real` rather than being drop-staled.
  If **any** command is buffered past the hole (`!pending.is_empty()`), the stream already
  continued — the missing tick was lost or reordered — so the host **advances** instead (the
  "deep-buffer yield"): it repeats the last intent (`Held`) and moves the cursor +1, so the
  cursor tracks the backlog rather than stalling the ack behind a lost tick (which would
  drive the client's reconcile lead unbounded, and on a deep buffer overflow into the
  catch-up trim). The deep-yield **counts toward the grace**, so a *sustained* non-empty gap
  (a lone far-future command, a multi-tick hole) gives up after `INPUT_HOLD_TICKS` and
  neutral-walks rather than `Held`-walking unbounded toward a distant command; an isolated
  loss is followed by a `Real` that resets the count, so isolated losses never accumulate.
  The give-up after the grace and the post-give-up neutral-walk (the coast toward a stream
  that resumed at a far-future tick) advance the cursor with synthesized neutral input.
  Neutralization clears movement, use, and fire but retains the latest finite aim pitch and
  facing yaw for remote-avatar presentation. A client that has never sent a command resolves
  to nothing — its pawn holds its authoritative pose. Clean loopback reaches the empty
  frontier at its buffer-empty edge and freezes exactly as before; a lost tick with newer
  data buffered yields. (The frontier test is exact: at the gap decision, the prior advance's
  stale-drop and intake's stale-check guarantee `pending` holds only commands newer than the
  awaited tick, so `pending.is_empty()` is precisely "nothing buffered ahead of the hole".)

- **Bounded playout buffer: standing floor + depth-keyed catch-up.** The two 60 Hz
  clocks free-run ~1 tick out of phase, so on a clean link the awaited command is usually
  not-yet-arrived when its tick resolves. A **one-shot buildup latch** establishes a
  standing playout floor proactively: armed at stream begin and after any give-up that
  empties the pending queue, it withholds the first `Real` — holding without consuming or
  advancing — until pending depth first reaches `INPUT_BUFFER_TARGET` (~2 ticks ≈ 33 ms),
  then disarms. After the first consume drops one command, the resolved cursor trails the
  newest received tick by ~`INPUT_BUFFER_TARGET − 1` ticks (≈ 1 tick / 16 ms in steady
  state — the signed `cursor_lead` diagnostic reads a small **negative** value, not the
  pre-fix 0). This margin absorbs the sub-tick phase offset that otherwise drop-staled the
  majority of a client's input. The latch is depth-keyed on pending count alone — a client
  that went silent then resumed at a far-future tick holds a single command far ahead
  (depth 1), which keeps the latch armed rather than reading as "buffer full", so the
  resume path stays intact. The standing invariant `INPUT_BUFFER_TARGET < INPUT_HOLD_TICKS`
  guarantees a normal buildup completes before the hold grace can give up on it (and a
  client that sends one command then goes silent still neutralizes — the latch cannot pin
  the pawn armed forever).

  A separate **catch-up** path handles deep backlogs, which would become *permanent*
  latency because drain-rate equals produce-rate. Two backlogs arise: a client streams
  input on connect before the host can drain its pawn (the accept/spawn handshake window),
  and a mid-session host frame hitch stalls the drain while commands keep arriving. When
  the pending queue's depth exceeds `INPUT_BUFFER_MAX` (~8 ticks ≈ 133 ms), the host
  fast-forwards: it keeps only the serially-newest `INPUT_BUFFER_TARGET` commands and
  reseats the cursor one serial tick behind the serially-oldest survivor, correct across
  the `u32` wrap. The trigger is **pending-queue depth (count of buffered commands), not
  tick-distance to the newest command** — the same depth-keying the buildup latch uses, and
  for the same reason. `INPUT_BUFFER_MAX > INPUT_BUFFER_TARGET` gives hysteresis so catch-up
  does not thrash.

  **Freeze and trim reconcile on depth.** The gap-policy freeze and the catch-up trim are
  ordered so they never fight: the freeze fires only when `pending.is_empty()` (the frontier),
  the trim only when `pending > INPUT_BUFFER_MAX`. A freeze therefore cannot grow the buffer
  into the trim a fortiori — it fires at depth 0, and in the gap-resolution phase every
  *non-empty* missing tick advances (the deep-buffer yield) rather than freezing, so a lossy
  backlog drains toward the frontier instead of piling into the trim; only genuine backlogs
  (handshake window, host hitch) reach it. The buildup-withhold above is a separate armed
  phase: it holds without advancing even though `pending` is non-empty, until depth first
  reaches `INPUT_BUFFER_TARGET`. This is what closes the divergence a count-blind freeze
  introduced: keying the freeze on a *count* (`pending.len() <= INPUT_BUFFER_TARGET`) still
  froze a lost tick whenever `pending` dipped to that count under jitter, stalling the ack
  and driving the reconcile error up; the **frontier** gate distinguishes a genuine late
  arrival (buffer empty → wait) from a lost tick with the stream continued (buffer non-empty
  → advance), so the ack never stalls behind buffered data.
  `INPUT_BUFFER_TARGET < INPUT_BUFFER_MAX` still bounds the buildup latch's
  standing depth well below the trim.

Reload uses a reliable edge lane beside command playout. Host intake observes reload
rising edges before stale-drop and backlog trimming, then delivers each due edge once on
an authoritative resolution. Duplicate or stale retransmits cannot create another edge.
If the previously emitted reload level is still high, recovery emits a low tick before
the preserved press so weapon-side level dedup sees a genuine rising edge. Movement,
look, and fire keep the ordinary gap and catch-up behavior.

A catch-up jump advances `last_processed_client_tick` by more than one tick. This is
safe for client reconciliation: the client prunes predicted history monotonically up to
the acked tick, so a forward jump simply discards a larger span of settled predictions
at once.

Before a command has resolved, intake anchors the stream at its first accepted tick.
Later input must remain less than `2^31` ticks forward of that anchor; input at or beyond
that distance is rejected before queue or reload-edge observation. This establishes the
serial-number half-range invariant before ordering reads the unresolved queue. Once a
resolved cursor exists, ordinary stale admission applies. The guard is not an arbitrary
smaller future-tick cap: normal `u32` tick wrapping remains valid.

All tick ordering is wrap-aware under the serial-number half-range invariant.
Stale-drop, duplicate-collapse, and first-resolution tick selection use
`client_tick_le`; catch-up finds its serial-newest anchor with that predicate, then
ranks commands by wrap-aware serial distance to select survivors and reseat the cursor.
This remains correct across the u32 `client_tick` wrap.

## Host-side remote-pawn presentation

The host presents each connected-client pawn through the **same** delay-buffered playout
the client uses for remotes, closing the presentation asymmetry where the host saw a
client's motion less smoothly than the client saw the host's. Each fixed tick the host
records the client pawn's authoritative `Transform` into a `RemoteInterpolationBuffer`
keyed by `NetworkId` (the client's key); each render frame it samples a **delayed
fractional** target — `newest_recorded_tick − INPUT_BUFFER_TARGET + alpha`, where `alpha`
is the render sub-tick accumulator — and writes the position-lerp/rotation-slerp result
through `EntityRegistry::set_presentation_transform`. The clock is the host's own
authoritative tick (the host *is* the clock — no `ClientTimeSync` estimate), and the
fractional target is load-bearing: sampling at an integer tick would step the pose once
per 60 Hz tick and reproduce the choppiness at the host's much higher render rate.

Authority is untouched. The host *simulates* the client's pawn, so `run_host_movement_tick`
and snapshot serialization must read the authoritative pose, never the delayed one. The
buffer holds the authoritative history; the registry `Transform` carries the delayed pose
only during the render-collect window. Per frame: **record** after each tick's movement,
**restore** the authoritative pose from the buffer before the tick loop (and thus before
serialization), and **present** the delayed pose after serialization. The path runs only
on `NetEndpoint::Host` and only for pawns in `MovementOwners` — the host's own pawn is not
an owner, so it keeps its live single-tick presentation. Engine glue lives in
`netcode::host_presentation`; the buffer is owned by the `Host` endpoint.

## Weapon placement is content, not client-local

First-person viewmodel placement — where a weapon sits in view — is authored
weapon-archetype content (a per-weapon placement descriptor plus a mod-global default),
resolved by the host into each occupied wieldable row of the existing opaque tuning
payload. This follows the entity-descriptor contract: small host-resolvable values are
replicated, not hashed. The transport wire vocabulary and mod compatibility digest stay
unchanged. Initial participation sends the effective placement; a live per-weapon or
mod-default edit changes the payload and sends a replacement. A connected client reads
only that host value and has no local placement fallback. The host can therefore
reproduce the shooter's authoritative fire origin from the same placement (see below).
Placement never reads client-local view-feel state.

The third-person avatar weapon mount does not read placement. Observers see the weapon
posed by the avatar hand socket; the FP viewmodel is a screen-space presentation. The
two vantages legitimately diverge — the shooter's authored FP placement versus
observers' socket pose — and the TP mount carries no placement offset. Art owns its
placement in the prop or socket. Placement is the base position; render-rate view-feel
sway/bob is a separate overlay composed on top (owned by movement), excluded from
authority.

**Fire origin composes on placement (decided, not yet built).** The authoritative
projectile origin is the weapon's model-local muzzle composed through the authored
placement — eye ∘ placement ∘ muzzle_local, steady placement, no view-feel. The muzzle
point is per-weapon content like a hit zone, replicated beside placement in the tuning
payload; a connected client predicts from that host value, never from the client-local
viewmodel mesh (the host holds no remote viewmodel). The spawned projectile origin equals
the validated fire origin — they never diverge. The observer's third-person muzzle, posed
by the avatar socket rather than placement, is a separate presentation vantage, deferred.
Today projectiles still spawn at the camera eye.

## Combat authority: FIRE vs HIT

Client-authoritative combat splits weapon fire into two independently-owned halves, both
riding the prediction/reconciliation contract above — no server rewind, no
lag-compensation history window (see *Non-goals*).

**FIRE is host-authoritative; cooldown is client-predicted.** Cooldown and ammo — how
often and how many shots — are the damage-integrity surface. The host validates fire
legitimacy, consumes the magazine, owns timed reload progression and reserve transfer,
advances cooldown, and mints an authorized shot; it never casts a ray. The firing client
predicts its own cooldown and reconciles against an owner-private cooldown fact, the same
pattern movement prediction uses. Client-side ammo and reload prediction/reconciliation
remain out of scope. Owner-private state-slot projection supplies each owner with the
host's authoritative magazine, reserve, reload progress, and reload-active state.

**HIT is client-authoritative declaration.** The client casts its own ray against the
world it renders and declares the result; the host validates cheaply and applies damage.
This is sound only because co-op PvE is a trust-with-cheap-validation model — PvP is a
non-goal. Declaring hits against the rendered world is also what keeps hitscan, pellet
spreads, and future projectiles the same shape: they differ only in ray count and arrival
timing, not in authority model.

### `shot_id`: the security spine

A `shot_id` binds a hit declaration to a specific host-authorized fire. The host mints and
records an open authorized shot on the FIRE path, keyed by `shot_id` and owned by the
firing connection. A declaration is accepted only when its `shot_id` matches a still-open
shot owned by the declaring client — ownership is checked, not assumed, because `shot_id`
derives from public inputs (pawn network id + tick) and is therefore guessable. Accepting
a declaration retires its shot, so one authorized fire accepts at most one declaration. A
fire the host rejected because it is cooling, reloading, or lacks enough magazine ammo
mints no authorized shot, so no declaration can bind to it — free damage is structurally
unreachable, not merely discouraged by a check. This binding is validated first, before
any geometry check.

### World-LOS-only validation

The host validates a declared hit point against **static world geometry only** — never
against the live pose of the target enemy, and never against other dynamic occluders. The
client aims at the interpolated (past) enemy pose it renders; the host is in the present.
Re-checking LOS against the live pose would false-reject legitimate shots on moving
enemies — the same staleness problem lag-compensating rewind exists to paper over, which
this design avoids outright by not needing rewind at all. The attacker eye origin for
validation is the live, crouch-aware eye height, never the standing reference — a
standing-eye ray would false-reject a legitimate crouched shot near cover.

### Ownership and identity maps

- **Pawn `Inventory`** (`postretro_entities`): the single source of truth for a pawn's
  active wieldable instance on every role. Host fire legitimacy, credit, cooldown,
  snapshot archetypes, owner-private projections, HUD feedback, and presentation all
  resolve through this component. `WeaponOwners` is only a host-side dirty attachment
  queue; it contains no pawn -> weapon mapping. A pawn with no active inventory entry
  cannot fire host-side.
- **`NetworkId <-> EntityId` reverse maps, one per peer role.** The client keeps
  `EntityId -> NetworkId` (to name a locally-hit remote enemy on the wire); the host keeps
  `NetworkId -> EntityId` (to resolve a declared target back to a live entity). Both are
  maintained beside their existing forward maps and kept in lockstep on spawn/despawn.
  `NetworkId` is never recycled, so a declaration naming a just-despawned target simply
  misses the lookup instead of resolving to the wrong entity.

### Message family

- **`HitDeclaration`** (client -> server, reliable Input channel): a `shot_id` plus 0..N
  hit records. Standalone rather than folded into the input command, because a hit can
  arrive on a later tick than its fire (projectile-ready). An empty record list is valid —
  it declares a shot that hit nothing.
- **Projectile contact marker:** projectile declarations use the existing hit-record
  shape and reserve target `u32::MAX` when a world contact or no-longer-nameable entity
  contact has no damage target. The finite, in-range point may retire presentation as
  contact even when entity lookup or damage validation fails. Empty projectile
  declarations remain normal travel/range expiry. This changes no wire layout or
  version constant.
- **`ShotVerdict`** (server -> client, owner-private): the per-shot accept/reject fact,
  scoped to the declaring client only and never broadcast. Owner-private state slots
  carry the firing pawn's cooldown, magazine, reserve, reload progress, and reload-active
  state, following the same per-owner projection pattern as `player.health`. The firing
  client reconciles predicted fire and hitmarker state against the verdict and cooldown;
  ammo and reload remain authoritative projections rather than predicted state.

### Version gates

Combat's message and field additions ride the existing two-gate handshake (see *Two-gate
handshake* above): a new message variant bumps the app-protocol (vocabulary) constant, and
any changed message layout — including a later, independent field addition to an
already-shipped message — bumps the wire-version (layout) constant again, independently of
any vocabulary change. `SNAPSHOT_VERSION` is untouched by anything that rides
`ClientMessage`/`ServerMessage` on the Input channel; it bumps only when a change lands on
the snapshot record itself. Rotating-mover phase fields use `SNAPSHOT_VERSION` 11;
mover replay provenance advances it to 12, and E17's replicated mover `blocked`
phase advances it to 13. Slide advances it to 14. The static-kinematic handshake field uses `WIRE_VERSION`
12; mover replay provenance advances it to 13, E15's tagged Control layout advances
it to 14, and participation-framed traffic advances it to 15. E16's `drop_pressed`
input edge advances it to 16, and E17's `blocked` phase advances it to 17. E16's
`JoinSeed` variant on `ClientControlMessage` advances it to 18. E16's dedicated
unreliable Presentation channel and `ServerPresentationMessage` family advance it to
19. Slide advances it to 20. Earlier peers are refused by both handshake gates.

## Current contract

Authoritative client-server co-op provides entity baseline, delta, and despawn replication; state-slot replication; snapshot interpolation; client input streaming; prediction; and reconciliation.

Replicable-set policy is gameplay-authoritative first. Player pawns, AI/enemies, movers, and other networked gameplay objects go on the wire. Deterministic client-local or baked data — particles, sprite visuals, lights, fog volumes, and shared `.prl` map data — stays off the wire unless gameplay authority requires otherwise.

Mover prediction is phase-seeded and separate from the pawn command-ring predictor. The host replicates authoritative mover phase; clients re-run the deterministic mover driver from that phase and reconcile in place, mapped by `NetworkId`. Rotating movers seed angle plus current and target spin rates; clients combine that phase with local PRL axis, acceleration, path, collision geometry, and carry policy, all covered by the level content digest. There is no provisional client-created mover copy.

A mover's **block reaction** (reverse/stop/crush on contact) is the exception to this pure re-simulation: it depends on entity positions the client does not simulate for remote pawns and enemies, so the host decides and clients reconcile only the resulting stop-hold as replicated phase. Block policy, auto-close timers, and per-victim crush cadence stay host-only — off the wire and off the content digest.

Trigger volumes are shared baked map data, not replicated state. Clients send a `use_pressed` input bit with movement input; only the host evaluates touch/use overlap and fires trigger commands. A fired command mutates replicated mover phase, including its optional target segment, so clients reconcile the resulting motion without ever evaluating the trigger locally.

Trap-pool arming follows the same host-only shape: at level install a seeded pass arms a subset of each tagged trigger pool. The roll never crosses the wire and clients never re-run it — client trigger armed-state stays as authored; only a host-armed trap's consequences (mover phase, spawned enemies) reach clients through replication. General posture for engine randomness: host-only, load-time, consequences-only — never per-tick or client-side, never shared-seed re-sim (the per-tick evaluator forbids RNG outright, `scripting.md` §12). One carved exception: weapon pellet-spread sampling runs deterministic per-tick RNG on whichever machine casts the rays. No roll crosses the wire or is re-run by another machine; each casts only its own pawn's rays. Its seed is a pure function of replay-stable weapon state, so the determinism gate can replay it exactly.

**Connected-client AI-enemy spawn suppression.** A connected client does not spawn local authoritative copies of AI enemies, whether map-placed or runtime-spawned (e.g. via a `spawnFromSpawner` reaction fired through the client's own trigger/named-reaction drain paths). Both are host-authoritative: the client receives them solely as host snapshots, runtime spawns arriving `RuntimeSpawn`-classified. A `SpawnContext` runtime-spawn authority flag, set false for a connected client, enforces suppression for the runtime-spawn path (see `spawner.rs`, `session/mod.rs`). Client-side materialization attaches only the descriptor's mesh presentation; `Brain`, `Agent`, `Health`, and `Weapon` components are never attached on the client for a remote enemy. Remote enemies are presentation-only — they carry no local simulation state.

## Not netcode: the live introspection channel

A separate localhost TCP channel (`observe-live` feature, planned — not built) lets an agent or CI attach to a running windowed session and read world state back over a socket. It shares no code with this netcode transport: a different socket, a length-prefixed-JSON wire (not the bitcode codec), and its own background thread — so the "no spawned threads" rule of the transport contract binds the *netcode* transport, not the engine. Read-only: it borrows the registry immutably at one main-thread frame boundary and never mutates, staying off the game-logic-owned apply path. It reuses the batch `observability` dump vocabulary — one vocabulary, two entry points, not a fork.

## Non-goals

- Deterministic lockstep / rollback, competitive PvP, matchmaking, anti-cheat, peer-to-peer, full server-rewind lag compensation (see `index.md` §4).
- bitcode as a persistence format — wire-only, gated on the handshake, never stored.
- An async runtime in the net path — the transport is polled and synchronous by contract.
