# Networking

> **Read this when:** working on multiplayer, replication, the wire/codec format, the netcode transport, or the host/client role model.
> **Key invariant:** the transport crate is registry-blind — it moves typed snapshots and never mutates entity state. `postretro-netcode` is the sole replication path that touches the `EntityRegistry`.
> **Related:** [Architecture Index](./index.md) · [Entity Model](./entity_model.md) §6 · [Development Guide](./development_guide.md) §4.2 · [Scripting](./scripting.md) §11

---

Authoritative client-server co-op uses client-side prediction and reconciliation. General-purpose multiplayer is a non-goal (see `index.md` §4).

## Crate boundary and ownership

`postretro-net` (`crates/net/`) is transport-only: wire codec, polled transport, time sync, and the dev-only latency harness. `postretro-netcode` is the registry-owning gameplay replication layer. It owns wire-side replication and state-slot tracking. The dependency direction is `postretro-netcode → postretro-net`; neither crate depends on the engine.

`postretro-net` is **glam-free and postretro-free by construction.** Wire types use plain `[f32; N]` / `f32` / `bool` — never glam or engine types. The crate is never handed an `EntityRegistry` and has no notion of entities, components, or game state. It moves opaque, typed messages.

`postretro-netcode` owns the other half of the contract. That crate is the *only* code that touches the registry on behalf of replication, and it owns everything that must know both sides: the role model, the `NetworkId↔EntityId` maps, and the wire↔engine type conversions (the glam-aware `Transform`↔`WireTransform` translation, the `ComponentKind→u16` mapping). The split is deliberate: the transport crate stays reusable and engine-agnostic, while registry mutation stays in gameplay logic.

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
| Input | reliable-ordered | client input/activation commands, HIT declarations, repair/acks and time sync; host activation outcomes, shot verdicts and observer weapon cues |
| Presentation | unreliable | host-addressed passive presentation events (damage numbers and future cosmetic facts) |

Reliability is matched to the data: control state and client→server repair/ack traffic must arrive ordered; snapshots are disposable because missing entity or state baselines are repaired by explicit refresh requests.
Presentation is a separate event lane: it is addressed to one client, fire-and-forget,
and never participates in ack, resend, reconciliation, or participation-epoch
bookkeeping. A lost packet simply produces no cosmetic, and a late joiner receives no
buffered event.
The host's frame bridge is bounded and keeps the oldest events in a burst. Overflow
drops newest events and emits one count-bearing warning at drain; it never grows memory
without limit. Clients reject non-finite presentation anchors before local intake.

## Wire/codec invariants

The wire codec is **bitcode**, pinned to an exact version. bitcode owns endianness and bit-packing — wire types do no manual byte layout. Two hard rules follow from bitcode's unstable byte format across majors:

1. **Never persist bitcode bytes.** The format is not a storage format. It exists only between two live, version-matched peers.
2. **Every connection is gated on the handshake** before any bitcode payload is decoded (see *Handshake* below). A version-mismatched peer is refused before a single message is interpreted.

**No serde-internally-tagged enum crosses the wire.** The engine's `ComponentValue` is a `#[serde(tag = "kind")]` enum, which bitcode cannot round-trip (`DeserializeAnyNotSupported`). So replication does not send engine types: it sends dedicated **wire-mirror** types that derive bitcode's native `Encode`/`Decode`. The component payload carries an explicit `u16` discriminant **numeric-equal to the engine `ComponentKind`**. Current entity payloads cover `Transform`, `PlayerMovementState`, `MeshAnimationState`, and `KinematicMoverState`. `MeshAnimationState` carries only current state name; descriptor mesh data stays local. The engine↔wire conversion lives in `postretro-netcode`; the mirror types know nothing about glam component order or serde tags.

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

Faction declarations and directional relationships are parity-gated mod content, refreshed with the mod digest after content commits. Declaration order remains meaningful because it establishes shared faction identities. Equivalent relationship encodings, including omitted or no-op overrides, canonicalize before comparison. The parity-gated content is the authored *baseline*; runtime-mutable live sentiment is host-authoritative **state**, not content — it replicates like mover phase and stays out of the content-parity digest, so a live-sentiment divergence never gates participation. Its writes are host-only by design: the host mutates and replicates, and a connected client converges on the host's values rather than writing its own. The live overlay admits only faction indices representable as `u16` and at most 4,096 diverged pairs. A write outside that transport envelope fails before changing host state. Snapshot production caches the lowered sparse set by overlay mutation generation, so unchanged frames do not rebuild it.
When a content refresh changes a relationship baseline without changing faction identity, the preserved live overlay rebases before replication resets: values equal to the refreshed baseline leave the sparse set, while genuinely diverged values remain live.

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

**Not built yet: the revealed term** (`ready/sh-streaming--reveal-gate-and-warm-horizon`). The predicate gains one term after content parity: both peers have revealed the host's installed level — finished Settling (`boot_sequence.md` §9). The client declares the revealed level identity on Control; the host records its own reveal and clears it at unload and suspend. A parity-matched slot that has not revealed stays admitted under a revealed holding cause, so the four stages and the entry rule below stand. Parity stays content identity, published at install. The name is "revealed", never "ready", which mods own.

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

The client predicts with the host's numbers, never its own, and the sites that resolve tuning keep **no fallback to the local registry** for a replicated value. A fallback fires only on the peers whose content differs, which is precisely the case replication exists to fix. This is a behavior semantic, not just a mechanism: a modder testing a movement change in co-op sees the host's values, not their own. First-person weapon placement also rides this payload because later fire authority consumes it. View feel and movement/weapon sound keys are stripped from tuning, so player view-feel settings survive a join and owners retain local sounds. Activation programs, scaling bases, and action aliases come from the host. Observer cues separately carry frozen host fire/impact keys, resolved against local sound assets.

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

**Impact bursts follow the same rule: one per contact on every peer, spawned locally.** The built-in spark burst never crosses the wire. The firing peer's own simulation spawns it: the host's local fire, or the client's predicted hitscan and projectile contact, gated like its `impact` event, so a dry or silent pull shows none and a later rejection does not retract it. Every other client spawns it from the reliable observer impact cue's contacts, using each contact's normal, and the host spawns it for a client's shot at HIT ingestion from the validated contacts. One route per peer is the invariant: splash sends no Presentation-channel burst, since it would double the cue's. An impact that publishes no cue (no `shot_id`, or more contacts than a cue carries) shows no burst to observers.

Damaged-enemy overlays are private per recipient. The host renderer owns only
host-local feedback; each remote recipient has an independent cap and linger
lifecycle. Equal-time cap decisions use the stable non-recycled `NetworkId`, so
unordered fact arrival cannot select a different retained target set.

**The player's damage bearing is the declared exception (decided, not yet built).** It is transient feedback, yet it rides owner-private replicated slots, never the Presentation channel, and adds no message kind. The presentation layer is world-anchored only, so a screen-space indicator has no anchor there, while a per-owner slot binds directly to a HUD widget. The host computes, at the damage chokepoint, a per-owner latest bearing in a frame the client can re-project, a source-known flag (damage need not have a spatial source — `entity_model.md` §7c), and a recency signal. Slots reach the client at snapshot cadence carrying only the latest value, so recency changes on every hit and holds until the next — a count or elapsed time, never a one-tick pulse a snapshot can straddle. Within one tick the last-applied hit wins. The owning client re-projects the bearing against its current view every frame into a client-local, unreplicated, readonly slot. A non-owning client never receives it. The mod HUD draws the indicator; the engine publishes the fact, not the policy.

### Mod identity

The manifest declares a stable id and a version. **The id gates** — it is the namespace that makes a catalog id resolvable, so peers must declare the same one. **The version never gates.** It rides the same message and serves display and diagnostics only; exact-version equality would refuse a friend on the previous build over a change no client simulates. Because a gating and a non-gating value share one message, the no-compare rule is commented at the comparison site rather than left to inference.

Both are **frozen at first commit** and do not move across a hot reload, diverging from the atomic-replace discipline most manifest lanes follow. Admission is terminal, so a mid-session id change would invalidate decisions already made, with no state to demote those connections to. The compatibility digest is the opposite case and re-hashes on every reload, because parity has a recovery path.

Identity is declared, not proven — tamper resistance is a non-goal, and neither field is a security mechanism.

## Session-state ledger

State that survives a level change, enumerated rather than accreted:

- **The connection** — its id, lifecycle stage, and last parity declaration. Not built yet: its last revealed declaration, which a host level change does not clear.
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

The transport crate emits typed snapshots and **never mutates the registry.** All registry-touching replication lives in `postretro-netcode`, which owns the two halves of the data path:

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

Knockback is host-authoritative combat state. Snapshots carry total player velocity and its protected knockback portion; reconciliation restores both before replay, without reapplying the hit. The host freezes direct-hit tuning at fire time and derives impulse from accepted hit geometry or projectile travel direction. Clients never submit impulse magnitude. Predicted projectiles remain presentation-only until authoritative movement arrives. Response tuning travels with the host movement descriptor. The wire and tuning payload versions reject older peers that cannot represent these fields.

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
| `--connect <ip:port>` | Client connecting to an explicit IPv4 address. |

**No endpoint, no gate.** Single-player constructs no endpoint, so no compatibility value is computed, no gate runs, and a level change announces nothing. Nothing branches on player count — the absent endpoint *is* the branch, which is why the compatibility digests are computed inside that check rather than unconditionally and discarded.

`--host` and `--connect` are mutually exclusive. **Direct connect only** — no discovery, no matchmaking, no relay. A host binds `0.0.0.0`, so it publishes the address to dial in `session.hostAddress`, with `session.hosting` set (`ui.md` §3): the interface its OS would route outbound traffic through, found by connecting an unsent UDP probe to a documentation address, which a same-machine client can dial too; with no routable interface it names loopback. Under a full-tunnel VPN that interface is the tunnel, which LAN peers cannot reach. `--host` refuses port 0: players must be told the port in advance. Network setup can fail (socket bind, transport init, hosted session-id entropy); a failure is logged and **degrades to single-player** rather than blocking boot — a netcode setup error never stops the engine from running. The local seat ledger remains available for single-player carry, but its fallback session id is never published. Clients receive host-authoritative replication, predict their own pawn locally, and reconcile against host acks.

**IPv4 only:** the host binds IPv4, and `--connect` refuses an IPv6 address at parse time with a clear message. Joining loses nothing by this — players on IPv6 networks almost always keep an outbound IPv4 path (dual-stack, or NAT64/464XLAT, DS-Lite, MAP-E translation). The reachability limit that matters is inbound IPv4 to a host behind carrier-grade NAT, DS-Lite, or MAP-E; IPv6 alone does not fix it, since home routers block unsolicited inbound IPv6 by default and the joiner needs IPv6 too, so a relay outranks dual-stack when host reachability becomes the complaint. MAP-E hosts can only forward ports in their assigned block, which `--host <port>` covers.

**Client connect lifecycle.** The client endpoint is built on the boot's logo frame, but the main thread then runs full renderer init and mod init before the next poll, so the connect clock starts at the **first poll**, not at construction; that first poll's `dt` is not counted. netcode times an unanswered attempt out after 15 s; a client that has **never connected** retries with a fresh socket, token, and client id, for a bounded number of attempts, which covers a host started after its client. A denial or an expired token ends the connect without a retry, and a connection that was established is never silently rejoined. Windows reports a send to an unbound port as a `ConnectionReset` on the next receive; the client skips those, as renet_netcode's server transport does, and lets the timeout decide. The transport logs the ending of a connection once, naming the host address (renetcode also logs each attempt's timeout). Nothing above the transport reacts to that ending: the endpoint stays a client and keeps polling, with no player-facing connection state.

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

The deterministic in-memory harness (`postretro-netcode::predict_reconcile_harness`) is the automated gate; this is its manual real-socket complement, for eyeballing the *feel* of prediction/reconciliation that automated tests cannot judge. Use a map with a descriptor-backed player pawn — `content/dev/maps/campaign-test.prl` (a `player_spawn` placement resolves to the `"player"` descriptor) — so the host materializes a real movement pawn on accept.

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
the preserved press so weapon-side level dedup sees a genuine rising edge. Use and drop
presses ride the same kind of lane: rising edges observed at intake, each delivered once
on an advancing resolution, so a trim cannot lose a door press or a weapon drop.
Activation starts and their release/cancel edges have their own lane (§Combat authority).
Movement, look, and held fire keep the ordinary gap and catch-up behavior; a trimmed jump
is still lost.

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
`postretro-netcode::host_presentation`; the buffer is owned by the `Host` endpoint.

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

**Fire origin composes on placement.** When a projectile weapon supplies a
model-local `muzzleOffset`, its authoritative origin is the weapon's muzzle
composed through the authored placement — eye ∘ placement ∘ muzzle_local,
steady placement, no view-feel. The muzzle point is per-weapon content like a
hit zone, replicated beside placement in the tuning payload; a connected client
predicts from that host value, never from the client-local viewmodel mesh (the
host holds no remote viewmodel). Within each peer's path, the spawned projectile
origin equals its validated fire origin. If the projectile's exact radius contacts
static world along the eye-to-muzzle sweep, or the eye ray contacts world at or before
the muzzle's forward plane, that peer uses the eye origin so the projectile cannot
spawn through nearby geometry. The eye-ray query reaches through the muzzle plane
even when projectile range is shorter. Otherwise its direction converges on the
crosshair target. An omitted offset preserves the historical camera-eye origin. The
observer's third-person muzzle, posed by the avatar socket rather than placement,
is a separate presentation vantage, deferred.

## Combat authority: FIRE vs HIT

Client-authoritative combat splits weapon fire into two independently-owned halves, both
riding the prediction/reconciliation contract above — no server rewind, no
lag-compensation history window (see *Non-goals*).

**FIRE is host-authoritative; execution and recovery are client-predicted.** The host
admits activations, resolves charge and shot statistics, debits each shot's resource,
owns timed reload progression and reserve transfer, and mints authorized shots. It
never applies a client's target or damage from this path. Projectile FIRE resolves
the eye ray against static world and live targetable entities to reconstruct an
obstruction-safe origin and crosshair-converged direction. Every authorized shot
retains its resolved combat and presentation tuning through later switching or
descriptor replacement. Hit declarations cannot select those statistics.

Each execution, including a hold restart, requires a client-named initiation on an
admitted real input command. The request names its client tick and primary/secondary
lane and binds to the live weapon instance. The host never invents remote restarts
from held input. Committed waits advance once per host simulation tick, independently
of movement cursor holds or jumps, with at most one shot per execution per tick.

**Starts survive playout.** A reliable-ordered Input stream stalls for a whole resend
interval when one packet is lost or the host hitches, and the catch-up trim then keeps
only the newest commands. A start dropped there loses its shot, and a surviving start
judged by host spacing alone is refused as cooling; either deletes a lagging client's
predicted bolt mid-flight. Intake therefore retains each start before stale-drop and
trim, up to 64 per client, and delivers the oldest once its own command tick has
resolved and no execution is live. A start that arrives during a live execution waits
rather than being refused. A retained start expires two seconds after it first becomes
due and is reported as an initiation rejection. A start dropped at intake, because 64
are already retained or because it replays a settled start, mints nothing and is not
reported; the client's predicted shot stands until its HIT is denied. Reaching 64 takes
seconds of total stall, so the gap is accepted rather than given its own refusal slot.

**Cadence is judged in client ticks, capped by host time.** Two halves gate a start.
The client half: its client tick must be at least the weapon's recovery,
`ceil(recovery_ms / tick_ms)`, after the client tick at which the previous execution's
recovery began; failing it refuses the start and mints nothing. The host half: the
start waits in its lane until host time since that recovery began, plus the 150 ms
charge tolerance, covers the recovery. Each execution is credited at the client's
claimed time clamped to at most the tolerance past host time, so an early start carries
its lead into the next rather than earning a fresh tolerance per shot. Over any W host
ticks, executions admitted stay at or below ⌊(W + tolerance) / R⌋ + 1 whatever ticks a
client stamps. Measuring in host ticks alone is what refused on-time starts after a
trim; measuring in client ticks alone would let a client fire as fast as it stamps. A
charged action's recovery runs from its client release tick, moved forward only; other
executions ignore releases for cadence, as the weapon machine does. The client half
applies only while the recorded recovery names the same weapon in the same slot;
otherwise the host's own cooldown applies. After a long stall the host half may hold
later shots of the same hold by up to the tolerance; it delays them, never refuses
them.

Explicit release/cancel names the initiating activation. Intake retains edges before
stale-drop or backlog trimming, deduplicates them, and delivers them once after that
start is admitted. Early edges wait for admission; terminal edges are inert. Synthetic
held/neutral commands contain no edges and cannot release charge. Cancellation wins
over release in the same command. Focus/menu suspension sends reliable cancellation
even on a render-only frame; intent does not advance execution, and the host applies
it on its next tick. Switch, drop, death, disconnect, level change, and descriptor
replacement also cancel future work without refunding authorized shots.

Charge uses wrap-safe release tick minus start tick, capped by host elapsed time
since admission plus 150 ms and then by authored full duration. Out-of-lifetime spans
reject. Missing intermediate samples never imply release or weaken a valid hold;
late arrival cannot add charge beyond the input timestamps. Severe backlog compression
can clamp charge. Release before the minimum cancels without debit; full charge never
auto-fires. Two seconds without an admitted real command cancels pending work; charge
also expires without firing 60 seconds after its full duration.

Connected prediction advances once per real fixed command after an equip-only
switch/timer pass. It freezes every due shot's action/program, scaling bases,
ordered shell/bloom reservation, effective fire/impact sounds, and projectile tuning.
All due shots in a rendered frame resolve against that frame's displayed aim/target
pose. Render-only frames advance no charge or steps.

Reliable owner outcomes report initiation accept/reject, execution acceptance with
resolved charge, cancellation, completion, and per-shot verdicts. Execution acceptance
is sent before awaiting HIT. Outcomes bind the token's captured local instance to the
host weapon id. Matching installed tuning corrects future snapshots and still-live
predicted projectile size, speed, radius, and remaining travel budget. Correction never
respawns, rewinds a transform, replays damage, or resurrects a contacted projectile.
Recovery corrections name an activation and bound instance; older results cannot
rewind newer execution. An admitted execution's outcome may only shorten the client's
predicted recovery, never lengthen it: the host admits on the client's own shot tick,
so the local countdown is already what admission requires; the host's remainder is
one transit stale and would make a client fire slower than the host player. An
initiation rejection still adopts the host's value. Slot-only cooldown projection can
seed an instance before prediction starts, but cannot roll back its active prediction.
No full weapon rollback.

Client-side ammo, heat, cell, and reload prediction/reconciliation remain out of scope;
connected clients never run the heat or cell update. Owner-private state-slot projection
supplies each owner with the host's authoritative magazine, reserve, reload progress,
reload-active, heat, overheat threshold, overheated latch, cell charge, and cell capacity,
each read from that owner's own pawn and beside the host wieldable slot it describes.

Each owner-private weapon value — cooldown, magazine, reserve, reload progress,
reload-active, heat, overheat threshold, overheated, cell charge, cell capacity — travels as a
`[host wieldable slot, value]` sample; the store keeps a plain
number or boolean, so HUD and script readers never see the slot. The slot names the weapon a
value describes: the host projects its own active weapon, which lags a local switch by a
round trip, and state records arrive per slot rather than atomically. An active weapon
without ammo sends its magazine and reserve as the `[slot]` absence instead, which the client
applies as the same store clear the host HUD makes; presentation reads it as no magazine to run
dry (a fire) and reload edges as no reload-capable weapon. Heat and cell numbers follow the
same rule for a weapon of another kind; the overheated latch has no absence and reads false
there. Which resource the active weapon runs is not replicated: every role publishes it from
its own active weapon, as it does the weapon name, so it leads the host-correlated values by up
to a round trip after a local switch. A pawn with no inventory sends the
HUD's reload defaults (no progress, not reloading) attributed to slot 0. `Unset` skips the
write for plain and correlated slots alike, and a correlated slot is sent only from its
projection, never from a plain table value. **Client fire
prediction is presentation-gated only.** Every due shot predicts, resolves, and declares
its contacts, including every shot in a multi-tick frame. Stale resource samples cannot
stop semantic attempts or declarations. The projection only chooses what a shot presents,
trusting each value it reads only when that value names the client's own active slot;
otherwise it presents a fire. An idle weapon whose magazine cannot pay the resolved shot cost
presents a dry fire: the dry-fire sound, with no fire sound, muzzle FX or impact. So does a cell
weapon whose charge cannot pay it. An overheated heat weapon presents nothing; the shot that
crosses the threshold presents a fire, since its latch arrives a round trip later. A magazine
reload in progress, or a per-shell reload whose magazine cannot pay the cost, presents
nothing; a per-shell reload the magazine covers presents a fire, since the shot cancels it. A
reload flag held at full progress is the replayed Completed endpoint, so the weapon reads
idle. A dry or silent shot suppresses cosmetics, but its predicted projectile still
simulates contact and declares normally. Only authoritative denial stops resource-dependent
future execution. A dry or silent shot shows neither muzzle FX nor a hitmarker, even when
its declaration carries an entity hit; verdicts retract presentation/flight without changing
newer recovery. Hitscan prediction keeps
world contacts and each contact's normal, so predicted impact presentation matches the
host's contact data.
Reload presentation edges (start, shell, complete) derive from the projected reload-active,
reload-progress, magazine, and reserve samples, one round trip late, attributed to the weapon
the client holds in the host slot the reload flag names. Every value read must name that
slot; a frame whose values name different slots is held unread. Complete is the flag falling
after the last held sample showed completion (magazine full, reserve empty, or a magazine
reload at full progress), or a fall in which ammo rose by exactly what the reserve fell while
the client wields that weapon. A rise that projects full progress replays a completion
endpoint and is no start. A local switch the host refuses keeps the reload tracked; one it
performs names another slot and presents nothing — including a switch away and back, when
any sample of the other slot arrives in between. Any other fall is a cancel and presents
nothing (`audio.md` §4). The overheat cue is the projected latch rising on the weapon the
client holds in the named slot while that slot is its active one. Heat and threshold must
name the same slot or the frame is held; a change of projected weapon resets the baseline, so
a switch back to a weapon still locked out plays nothing. Presentation only; ammo, heat, and
cell stay unpredicted.

Projectile launch prediction is not rewind-synchronized. The firing client launches from
its rendered local camera and rendered target state; the host later reconstructs from the
live authoritative pawn and target state plus the transmitted aim and shared tuning.
Ordinary latency may therefore produce slightly different origin, direction, or contact.
`ShotVerdict` reconciles fire/hit acceptance, muzzle FX, hitmarker state, and rejected
predicted flight; correlated activation outcomes reconcile recovery and charge. Neither
rewinds the predicted projectile transform. This is the accepted no-rewind co-op
tradeoff. Exact launch-pose reconciliation requires a separate protocol design.

**HIT is client-authoritative declaration.** The client casts its own ray against the
world it renders and declares the result; the host validates cheaply and applies damage.
This is sound only because co-op PvE is a trust-with-cheap-validation model — PvP is a
non-goal. Splash projectile detonation is stricter: the declaration binds the authorized
shot, but the host derives the first contact from frozen fire origin, direction, speed,
radius, range, lifetime, and elapsed host ticks. The client never selects a splash center.

### `shot_id`: the security spine

A `shot_id` binds a hit declaration to a specific host-authorized fire. The host mints and
records an open authorized shot on the FIRE path, keyed by `shot_id` and owned by the
firing connection. A declaration is accepted only when its `shot_id` matches a still-open
shot owned by the declaring client — ownership is checked, not assumed, because `shot_id`
derives from public inputs (pawn network id, initiating client tick, action lane, authored
shot ordinal) and is therefore guessable. Host fire tick remains separate. Ordinals name
authored attempts, including refused ones; movement-cursor progress never names a later shot. Accepting
a declaration retires its shot, so one authorized fire accepts at most one declaration. A
fire the host rejected because it is cooling, reloading, or lacks enough magazine ammo
mints no authorized shot, so no declaration can bind to it — free damage is structurally
unreachable, not merely discouraged by a check. This binding is validated first, before
any geometry check.

Early HIT for a future ordinal waits for its matching decision. Cancellation rejects
unissued ordinals; previously authorized projectiles retain their normal validation and
lifetime. Per client, pending declarations, retained release/cancel records, and terminal
activation records are each bounded to 64. Unknown declarations/edges expire two seconds
from first receipt; duplicates never refresh expiry. Future ordinals expire two seconds
after their scheduled decision. Terminal records expire two seconds after termination;
overflow evicts the oldest, while a monotonic settled-start watermark prevents replay
after eviction. Live future ordinals remain independent of that watermark. Declaration
overflow rejects the newest HIT without undoing FIRE. Expiry does not reject an
initiation that remains eligible for admission. If FIRE is still unknown or
pending at overflow or expiry, a reliable HIT-only refusal retires the client's hit feedback record without
changing recovery or its predicted flight. A later actual FIRE denial still removes
that shot's flight even after its hit feedback record has gone; no refusal queue is
retained. Already-decided FIRE keeps its ordinary accurate verdict. Edge overflow rejects the newest
edge and cancels its affected active execution. Live delivered-edge history stays until
termination so duplicate release cannot revive and a later cancel remains valid.

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
  it declares a shot that hit nothing. Each record carries a target, point, optional hit
  zone, and the contact's surface normal.
- **Presentation-contact marker:** target `u32::MAX` marks a record with no damage
  target — a world contact, or an entity contact no longer nameable. Projectile and
  hitscan declarations share it. For direct projectiles, the finite in-range point may
  retire presentation even when entity lookup or damage validation fails. For splash,
  the marker only reports contact; the host-replayed first contact supplies damage,
  occlusion, and presentation position. Empty projectile declarations remain normal
  travel/range expiry. Hitscan declarations carry every world contact under the marker,
  so the host holds a remote shot's full contact set. The host validates a hitscan world
  contact with the checks an entity hit gets — range from the live eye, and eye line of
  sight to a point pulled 1 cm back from the contact, so the struck surface never blocks
  its own validation. It applies no damage from one.
- **Declared normals are contact data only.** A non-finite or non-unit normal drops that
  record's contact data and never affects damage validation. Validated normals reach the
  host's impact presentation; a splash impact keeps its host-resolved normal. The host
  raises one `impact` per remote shot carrying every validated contact, as a local
  shot does (`audio.md` §4).
- **`ActivationOutcome`** (server -> client, reliable owner-private Input channel):
  initiation/execution and terminal facts naming the token, bound host instance where
  accepted, resolved charge at execution acceptance, and authoritative recovery.
- **`ShotVerdict`** (server -> client, owner-private): the per-shot accept/reject fact,
  scoped to the declaring client only and never broadcast. Owner-private state slots
  carry the firing pawn's cooldown, magazine, reserve, reload progress, reload-active
  state, and heat or cell values, each beside the host wieldable slot it describes, following the same per-owner projection pattern as `player.health`. The firing
  client reconciles predicted fire, flight, and hitmarker state against the verdict;
  activation outcomes own recovery correction. Ammo and reload remain authoritative
  projections rather than predicted state.
- **Observer weapon cues** (server -> participating clients, reliable ordered Input
  channel): frozen fire/impact sound keys, action aliases, shot identity, and captured
  entity/contact anchors, independently of snapshot cadence. The firing owner is
  excluded. Receivers resolve local assets without re-reading weapon descriptors.
  Queues clear on demotion, level changes, and world-less polling. Explicit projectile
  sprite size/model scale and shot identity survive snapshot seeds, deltas, and late
  joins; local projectile assets keep the established local-resolution policy.

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
19. Slide advances it to 20. The sparse faction-sentiment snapshot record advances
`SNAPSHOT_VERSION` to 15 and `WIRE_VERSION` to 21; it changes no Input-channel
`ClientMessage` or `ServerMessage` variant. Protected player knockback velocity
advances `SNAPSHOT_VERSION` to 16 and `WIRE_VERSION` to 22. The hit record's contact
normal advances `WIRE_VERSION` to 23; `SNAPSHOT_VERSION` is unchanged. Weapon
activation commands/outcomes advance the application protocol to PRL8 and wire to
24. Explicit projectile body size/model scale/shot identity and reliable frozen
observer weapon cues advance the application protocol to PRL9, `WIRE_VERSION` to
25, and `SNAPSHOT_VERSION` to 17. Incompatible peers fail the handshake; snapshot
17 independently rejects older snapshot envelopes. The PRL level-file format is
unchanged. The host movement descriptor's knockback response advances the tuning
epoch to 9; host-resolved activation programs/scaling bases advance it to 10.
Slot-correlated owner-private weapon samples change only the state-schema fingerprint,
through per-slot wire-shape tags: `[slot, number]` is tag 1, `[slot, flag]` tag 2, and
`[slot, number]` or `[slot]` (magazine and reserve) tag 3. They ride the existing array
value, so `WIRE_VERSION` and `SNAPSHOT_VERSION` are unchanged. A mixed-build peer is not
refused: the handshake admits it, and its client rejects every state batch at the
fingerprint check for the whole session, so no replicated state (health included) reaches
it until both peers run the same build.

## Current contract

Authoritative client-server co-op provides entity baseline, delta, and despawn replication; state-slot replication; snapshot interpolation; client input streaming; prediction; and reconciliation.

Replicable-set policy is gameplay-authoritative first. Player pawns, AI/enemies, movers, and other networked gameplay objects go on the wire. Deterministic client-local or baked data — particles, sprite visuals, lights, fog volumes, and shared `.prl` map data — stays off the wire unless gameplay authority requires otherwise.

Mover prediction is phase-seeded and separate from the pawn command-ring predictor. The host replicates authoritative mover phase; clients re-run the deterministic mover driver from that phase and reconcile in place, mapped by `NetworkId`. Rotating movers seed angle plus current and target spin rates; clients combine that phase with local PRL axis, acceleration, path, collision geometry, and carry policy, all covered by the level content digest. There is no provisional client-created mover copy.

A mover's **block reaction** (reverse/stop/crush on contact) is the exception to this pure re-simulation: it depends on entity positions the client does not simulate for remote pawns and enemies, so the host decides and clients reconcile only the resulting stop-hold as replicated phase. Block policy, auto-close timers, and per-victim crush cadence stay host-only — off the wire and off the content digest.

Trigger volumes are shared baked map data, not replicated state. Clients send a `use_pressed` input bit with movement input; only the host evaluates touch/use overlap and fires trigger commands. A fired command mutates replicated mover phase, including its optional target segment, so clients reconcile the resulting motion without ever evaluating the trigger locally.

Trap-pool arming follows the same host-only shape: at level install a seeded pass arms a subset of each tagged trigger pool. The roll never crosses the wire and clients never re-run it — client trigger armed-state stays as authored; only a host-armed trap's consequences (mover phase, spawned enemies) reach clients through replication. General posture for engine randomness: host-only, load-time, consequences-only — never per-tick or client-side, never shared-seed re-sim (the per-tick evaluator forbids RNG outright, `scripting.md` §12). One carved exception: weapon pellet-spread sampling runs deterministic per-tick RNG on whichever machine casts the rays. No roll crosses the wire or is re-run by another machine; each casts only its own pawn's rays. Its seed is a pure function of replay-stable weapon state, so the determinism gate can replay it exactly. The cone half-angle that sampler receives is replay-stable too: descriptor-tuned bloom, movement, and vertical bias compose it through the shared host/client calculation without touching the seed. The authored tuning replicates; each casting machine derives its own result.

**Connected-client AI-enemy spawn suppression.** A connected client does not spawn local authoritative copies of AI enemies, whether map-placed or runtime-spawned (e.g. via a `spawnFromSpawner` reaction fired through the client's own trigger/named-reaction drain paths). Both are host-authoritative: the client receives them solely as host snapshots, runtime spawns arriving `RuntimeSpawn`-classified. A `SpawnContext` runtime-spawn authority flag, set false for a connected client, enforces suppression for the runtime-spawn path (see `spawner.rs`, `session/mod.rs`). Client-side materialization attaches only the descriptor's mesh presentation; `Brain`, `Agent`, `Health`, and `Weapon` components are never attached on the client for a remote enemy. Remote enemies are presentation-only — they carry no local simulation state.

## Not netcode: the live introspection channel

A separate localhost TCP channel lets an agent or CI attach to a running windowed session and read world state back over a socket. It ships behind the dependency-free `observe-live` feature and `--observe-live <PORT>`; without both, it opens no socket or transport thread. It shares no code with netcode: a distinct TCP socket, 4-byte-length-prefixed JSON (not bitcode), and a background transport thread that passes only opaque request and reply bytes. A single-slot queue bounds transport-to-engine backlog; overload closes the requesting connection. The main thread services at most one request at the Input-stage frame boundary before game logic, reads the registry immutably, and builds the reply. The channel is read-only, so it stays off the game-logic-owned apply path. It reuses the batch `observability` dump vocabulary — one vocabulary, two entry points, not a fork. The exception is a named live-only section for data with no batch meaning: `cpu_timing`, the latest closed CPU stage timing window, requested with `dump.cpu_timing` (`rendering_pipeline.md` §12). It reads without consuming, and reports `not-requested` or `not-yet-windowed` rather than zeros. A live-only section is exempt from the batch dump's byte-identity rule, and a batch runspec that requests one is rejected. The transport contract's no-spawned-threads rule therefore binds netcode, not this engine-local channel.

## Non-goals

- Deterministic lockstep / rollback, competitive PvP, matchmaking, anti-cheat, peer-to-peer, full server-rewind lag compensation (see `index.md` §4).
- bitcode as a persistence format — wire-only, gated on the handshake, never stored.
- An async runtime in the net path — the transport is polled and synchronous by contract.
