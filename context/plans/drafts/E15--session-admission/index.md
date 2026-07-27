# Session Admission (E15 Phase 3.75)

## Goal

A client can join a session before it knows which map to load. Today the app-level
handshake carries the loaded map's fingerprint, and the host queues every handshake
until its own level installs — so acceptance requires a loaded map on both sides, and
a client cannot connect to a host that will *tell* it what to load. Split the gate:
**admission** proves protocol compatibility and matching mod identity with no map
involved, and **content parity** proves the map matches, revalidated at every level
install rather than bound to the connection for its lifetime.

Consequence, and the reason this ships alone: after this spec a host changing levels
**demotes** its clients instead of disconnecting them. The connection outlives the map
it joined on. Learning *which* map to load next is the next spec.

## Prerequisites

- **Epic 15 Phase 1** (shipped) — the two-gate handshake, the control channel, the
  typed reject reason, and the protocol/wire constants this restages.
- **Epic 15 Phase 2** (shipped) — the slot lifecycle (`Pending`/`Accepted`/`Closed`),
  its close events, and the per-slot cleanup a demotion reuses.

## Scope

### In scope

- **Two-stage gate.** Admission (protocol + mod identity, no map) and content parity
  (level fingerprint), as separate control messages evaluated at separate times.
- **A slot state between pending and participating.** An admitted slot holds a live
  connection and receives no entity state.
- **Demotion instead of closure.** A host level install returns every participating
  slot to admitted and runs the same per-slot cleanup a close runs today.
- **Mod identity in the manifest** — a stable id and a version, declared by the mod,
  compared for exact equality at admission.
- **A typed reject reason delivered to the client**, so a refused player learns
  whether the protocol, the mod, or the map diverged.
- **Fingerprint domain widened to include level identity**, because per-level
  revalidation against a hash that cannot discriminate maps is a no-op.

### Out of scope

- **Telling a client which map to load.** The relevel message, host-side net reset on
  unload, and transport polling across the unload→install window are spec 2. After
  this spec an admitted client waits until it happens to install a matching level.
- **Client-side reconnect.** A closed connection stays closed; only demotion is added.
- **Player identity, seats, and the roster.** Spec 3. Admission decides whether a
  connection is allowed, not who is behind it.
- **Authored join policy.** Spec 4. Admission is engine-owned mechanism only; the
  predicate that gates it has nothing to bind against until the roster exists.
- **Shipping mod content to a client that lacks it.** Networked mod sync is a stated
  non-goal (`boot_sequence.md` §8). Matching is in scope; distribution is not.
- **Tamper resistance.** Mod identity is declared, not proven. See Decisions.
- **Hashing the `.prl` bytes.** Decided, not deferred by omission — see Decisions.
- **A graceful host-leave message.** Named by the roadmap as a host-migration
  prerequisite; nothing in this spec needs it.

## Direction

**Problem.** The app gate is one message carrying both a protocol version and a map
fingerprint, and the host refuses to evaluate it until a level installs. That single
coupling is the cause: it makes "connected" and "on the host's map" the same state, so
there is no state a client can occupy while waiting to be told what to load. Every
capability the session band wants — a lobby, server-chosen maps, a session that
survives a level change — is blocked behind it.

**Prior commitments.**

- *Two gates catch different failures at different layers* (`networking.md`). Honored
  and extended: this splits the second gate along the same reasoning, because
  "compatible build and mod" and "same map content" are also different failures that
  become true at different times.
- *No entity state reaches a client that has not passed the gate.* Preserved exactly.
  The send path gates on participation, which is strictly narrower than today's
  accepted, so nothing that was refused becomes permitted.
- *Exact-match validation, refuse rather than migrate* (`networking.md`, mirroring the
  `BakedIr` version-epoch discipline). Honored: mod version is exact string equality.
- *The net crate is registry-blind and postretro-free.* Preserved — mod identity
  crosses as two opaque strings the crate compares and never interprets.
- *First successful commit wins* (`boot_sequence.md`, the persistence-overlay rule).
  Reused for mod identity under hot reload.
- **Divergence, named.** `networking.md` states "a connection is bound to that
  fingerprint for its lifetime. Installing different static mover content closes it."
  This spec overturns that sentence deliberately. The rule exists because closing is
  the only safe response to content a peer cannot validate when no protocol exists for
  reaching agreement — it encodes a missing feature, not a safety property. Demotion
  preserves the actual guarantee (nothing replicates while content is unproven) and
  drops only the disconnection. `networking.md` wants this rewritten at promotion.

**Placement.** The gate stays in `postretro-net`. It is pure comparison over opaque
values, it is where both existing gates live, and the crate's registry-blindness is
what keeps it unit-testable without a socket. The *sources* of the compared values
stay engine-side and are installed after construction, exactly as the fingerprint
already is — mod identity is not available when `Session::build` constructs the
endpoint, because mod init runs later in boot.

**Alternatives rejected.**

- *One message, evaluated twice.* Keep `ProtocolVersion` as-is; have the client resend
  it on every level install and the host re-evaluate. Cheapest possible change, and it
  is the real rival. Rejected because it does not solve the problem: the message
  carries a fingerprint, so a client with no level still cannot send one, and the host
  still cannot admit it. The coupling *is* the bug; re-evaluating a coupled message
  more often does not decouple it.
- *Admit on protocol alone; check the mod at first level install.* Fewer moving parts,
  and mod mismatch would surface through the parity failure anyway. Rejected because
  it reports the wrong cause — a player running the wrong mod would be told the map
  does not match — and because the roster and join policy that follow need a mod-valid
  connection to exist before any level does.
- *Keep closing on fingerprint change; add a reconnect.* Preserves the shipped
  invariant verbatim and reaches the same end state. Rejected: reconnect re-mints the
  client id (wall-clock nanos, per connection), so every per-connection identity the
  next two specs build would be destroyed at every level change — the exact defect the
  currency spec died on three times. Demotion keeps the connection, so identity keeps
  a stable anchor.

**Foreclosures and one-way doors.** The wire-visible mod identity is the one-way door:
once mods declare an id and a version, that pair is a compatibility contract between
peers and cannot be quietly restructured. Undoing it costs a protocol bump and a
manifest migration for every mod. Widening the fingerprint domain is not a one-way
door — it is behind an epoch constant and a protocol bump, both already hand-bumped.
Nothing here forecloses hashing `.prl` bytes later; that would be a further widening
of the same domain.

## Decisions

- **Two control messages, not one widened message.** Admission carries the two
  protocol constants plus mod id and version; parity carries the fingerprint alone.
  They are separate because they become true at different times — admission at mod
  init, parity at every level install. One message would reintroduce the coupling.
- **Mod identity is declared, not derived.** An author-chosen id and version compared
  for exact string equality. This is a compatibility check: it catches the wrong mod
  and a stale version, which is the real failure among friends. It does not catch
  tampering and must not be documented as if it does — anti-cheat is a stated non-goal.
- **Exact version equality, no ranges.** A semver range needs a compatibility policy
  the project cannot test. The author bumps the version when the shared contract
  changes.
- **Mod id charset is validated at manifest parse** — `[A-Za-z0-9_.:-]`, the same
  charset rule shipped ammo-type identifiers use. Version is a free-form non-empty
  string; it is compared, never ordered.
- **Both fields are required.** An optional identity means an unidentified mod, which
  makes the gate meaningless for exactly the mods most likely to drift. Every shipped
  manifest gains both.
- **Mod identity installs after construction and the gate queues until it arrives.**
  The endpoint is built in `Session::build`; mod init runs later. This mirrors the
  fingerprint's existing install-then-unblock shape rather than inventing a second one.
- **First commit wins under hot reload.** A staged manifest whose identity differs from
  the installed one warns and keeps the installed value. Mid-session identity churn
  would silently invalidate a live session's admission decisions.
- **A demotion runs the same cleanup as a close.** Level unload invalidates every id
  the per-slot tables hold, so a demoted slot's pawn, replication, ownership, command,
  state-slot, and combat state are cleared exactly as a close clears them. The
  connection is what survives, not the state.
- **The fingerprint's domain gains the level's identity, and its epoch bumps.** It
  covers mover authoring and collision today, so two mover-less maps hash identically
  — a latent fail-open when bound once per connection, and a silent failure of this
  spec's central invariant when evaluated per level. Level identity is resolved
  immediately before the fingerprint is computed, on the same borrow.
- **Hashing the `.prl` bytes instead is rejected.** Strictly stronger, and it makes any
  cross-platform bake difference a hard connection failure — a bake-determinism
  question this spec has no standing to answer. The domain stays authored-input-shaped.
- **The fingerprint is renamed to match its widened domain.** It stops being about
  kinematics. Mechanical: the wire field, one producer, and the endpoint setter.
- **A reject sends its reason before disconnecting.** The slot closes immediately, so
  no further traffic is honored; only the socket teardown defers one poll, letting the
  reliable message flush. Without this a player running the wrong mod sees an
  unexplained failure to connect, which is most of what this feature is for.
- **Reject reasons become a typed enum over three causes** — protocol, mod, parity —
  each carrying expected and received. `RejectReason` and `HandshakeOutcome` stop being
  `Copy` (the mod id is a `String`) and become `Clone`; their call sites update in the
  same pass.
- **Both protocol constants bump.** A new message vocabulary bumps the app protocol id;
  the changed layout bumps the wire version. Per `networking.md` §Version gates these
  are independent bumps and both apply.
- **Single-player is untouched.** No endpoint is constructed, so no gate runs and no
  path branches on player count.

## Acceptance criteria

- [ ] A client connecting to a host that has no level installed is admitted, holds the
      connection open, and receives no entity records.
- [ ] That client receives entity records once — and only once — the host and client
      have installed levels with matching content fingerprints.
- [ ] A client whose declared mod id or version differs from the host's is refused, is
      told which of the two diverged, and receives no entity records.
- [ ] A client whose protocol constants diverge is refused with a protocol cause, not a
      mod or map cause.
- [ ] A refused client observes the typed reason before its connection closes.
- [ ] A host installing a different level **keeps** its clients connected: each drops
      to admitted, stops receiving entity records, and its remote pawn, replication,
      ownership, command, state-slot, and combat state are cleared exactly as a
      disconnect clears them.
- [ ] A demoted client re-participates without reconnecting once it installs a level
      whose fingerprint matches, and its client id is unchanged across the demotion.
- [ ] Two maps that differ only in ways the old fingerprint ignored — including two
      maps with no movers at all — produce different fingerprints, and a client on one
      does not participate on the other.
- [ ] A manifest missing the mod id or version, or whose id violates the charset, fails
      mod init with a diagnostic naming the field.
- [ ] A staged hot reload that changes the mod id or version warns and leaves the
      installed identity unchanged; the session's admitted clients stay admitted.
- [ ] Single-player boot constructs no endpoint and reaches Running unchanged.
- [ ] A peer built before this change is refused at the transport gate, before any app
      message is decoded.

## Tasks

### Task 1: Extract the handshake gate

`crates/net/src/transport.rs` is ~740 non-test lines and this spec adds a second gate
stage to it. Split first, behavior-preserving: move the pure gate surface into a new
`crates/net/src/handshake.rs` beside `slots.rs` — the wire-comparison types, the
validation function, the reject reason and its `Display`, the protocol-constant
accessors, and the malformed/hex helpers. `transport.rs` keeps the renet plumbing:
socket, channels, `NetServer`/`NetClient`, the poll loop, and the send gating. Re-export
from `lib.rs` so no downstream import path changes. No behavior change, no new tests
beyond relocating the existing gate unit tests with their subject.

### Task 2: Mod identity in the manifest

Add a required stable id and a required version to the mod manifest. Declare both on
the `ModManifest` type in `sdk/types/postretro.d.ts` and mirror them in the Luau
typedef, documenting that they are compared for exact equality between peers and that
this is a compatibility check, not a security one. Parse both at **four** sites — the
JS and Luau manifest readers in `crates/scripting-core/src/runtime/mod_init_exec.rs`
and their staged-reload counterparts in `crates/scripting-core/src/staged_manifest.rs`
— each following the existing required-`name` shape, so a missing field is an
`InvalidArgument` naming the field and the source path. Validate the id against
`[A-Za-z0-9_.:-]` at parse; the version is any non-empty string. Carry both on
`ModManifestResult` beside `name`. Commit them with the rest of the manifest at mod
init, and make the commit **first-wins**: a staged reload whose identity differs from
the installed one logs a warning and leaves the installed value alone, mirroring the
persisted-state overlay rule. Update every shipped manifest under `content/` to declare
both.

### Task 3: Two-stage gate in the net crate

Replace the single app-gate message with two, in Task 1's module. **Admission** carries
the two protocol constants plus mod id and version; **parity** carries the level content
fingerprint. Both ride the existing reliable Control channel. Make the reject reason a
typed enum over three causes — protocol, mod, parity — each carrying expected and
received; `RejectReason` and `HandshakeOutcome` lose `Copy` and gain `Clone`, and their
call sites in this crate update in the same pass. Add an `Admitted` state to
`SlotTable` between `Pending` and `Accepted`, rename `Accepted` to `Participating` to
match what it now means, and add the demotion transition `Participating → Admitted`
that emits a new lifecycle event so the engine glue can clean up. Keep `Closed`
terminal and keep every existing idempotence property. `NetServer` holds mod identity
and the fingerprint as separate `Option`s installed after construction: admission
evaluates once identity is present, parity evaluates once a fingerprint is present, and
each queues until then — extending the shipped early-return rather than adding a second
waiting mechanism. `set_level_content_fingerprint` **demotes** participating slots
instead of closing them. Gate `send_snapshot` and `accepted_clients` on participating
only. On reject, enqueue the typed reason on Control and close the slot immediately, but
defer the socket disconnect to the next poll through a small pending-disconnect list on
`NetServer`, so the reliable message flushes first. On the client, split `handshake_sent`
into an admission flag sent once on connect and a parity flag re-armed on every
fingerprint change, replacing today's self-disconnect. Bump `PROTOCOL_ID` and
`WIRE_VERSION`. Unit-test the gate and the slot machine without sockets, and extend the
loopback integration tests to cover a mod-mismatch reject and a demote-then-re-participate
cycle.

### Task 4: Engine wiring

Wire the engine's two halves to the new gate. **Mod identity:** after mod init commits,
install the committed id and version on the net endpoint through a setter mirroring the
fingerprint's, reaching it through the same `session.net_endpoint` borrow
`install_level_payload` uses; single-player has no endpoint and skips. **Fingerprint:**
widen `kinematic_static_fingerprint` in `crates/postretro/src/runtime_movers.rs` to
fold in the level's own identity, taking it from `App.active_level_source`, which
`retain_active_level_tags_for_install` populates on the line immediately before the
fingerprint is computed in `install_level_payload`; bump the function's epoch constant
and rename the function, the wire field, and the endpoint setter to name the widened
domain. **Accept seam:** the pawn spawn currently keyed off `HandshakeOutcome::Accepted`
in `main.rs` moves to the participation transition — a pawn needs a level, which is
exactly what parity now proves. **Demotion:** route the new demotion event into
`host_handle_lifecycle` so it runs the same per-slot cleanup a close runs; do not
duplicate that cleanup. Keep the `main.rs` edit to redirecting these two triggers —
splitting that file is out of scope and explicitly deferred by `runtime-level-lifecycle`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the split every later edit lands in.
**Phase 2 (concurrent):** Task 2, Task 3 — the net crate carries mod identity as opaque
strings, so neither consumes the other; they touch disjoint crates.
**Phase 3 (sequential):** Task 4 — consumes the setters from Task 3 and the committed
identity from Task 2.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No entity state reaches a slot below participating | Task 3 (send gating on participating) | Every new send path must gate on participation, not admission or connection | AC 1, 3, 6 |
| Content parity is proven for the *current* level, not the joining one | Task 3 (demotion on fingerprint change), Task 4 (per-install fingerprint) | A demotion that failed to clear state would leave stale ids addressable | AC 6, 7 |
| A demotion clears exactly what a close clears | Task 3 (event), Task 4 (routed into the existing cleanup) | Shared with the close path — a second cleanup implementation would drift | AC 6 |
| The fingerprint discriminates any two distinct levels | Task 4 (domain widened with level identity) | The whole per-level gate is a no-op if two levels can collide | AC 8 |
| A connection's id survives a level change | Task 3 (demote, never close) | Later specs key player identity off a connection that must not be re-minted | AC 7 |
| Admission and parity queue independently until their source installs | Task 3 (two `Option`s, two early returns) | Coupling them re-creates the ordering inversion this spec removes | AC 1, 2 |
| A refused peer learns the cause before teardown | Task 3 (deferred disconnect) | A future reject path that disconnects inline drops the message | AC 3, 4, 5 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod id | `ModManifestResult` field | admission message field | `id: string` | `id: string` |
| mod version | `ModManifestResult` field | admission message field | `version: string` | `version: string` |
| admission message | net-crate handshake type | Control channel, bitcode `Encode`/`Decode` | n/a | n/a |
| parity message | net-crate handshake type | Control channel, bitcode `Encode`/`Decode` | n/a | n/a |
| reject reason | typed enum, three causes | Control channel, server→client | n/a | n/a |
| level content fingerprint | `[u8; 32]`, engine-computed | parity message field | n/a | n/a |
| slot state | `SlotState::{Pending, Admitted, Participating, Closed}` | not replicated | n/a | n/a |

## Script syntax examples

```ts
// Proposed design — two new required fields; everything else is unchanged.
export default defineMod({
  // Stable machine identity. Charset [A-Za-z0-9_.:-]. Peers must declare the same id.
  id: "postretro.dev",
  // Compared for exact equality, never ordered. Bump it when the shared contract
  // changes — this catches a stale mod, not a tampered one.
  version: "0.4.0",
  name: "Postretro Dev Mod",
  entities: [/* … */],
  maps: [/* … */],
});
```

## Open questions

- **Reject-reason delivery is the trimmable part.** The deferred disconnect is one
  list and one poll, and everything else in the spec works without it. If it proves
  awkward against renet's teardown, the fallback is a host-side log only — at the cost
  of a player who cannot tell a wrong mod from an unreachable host.
- **Version-mismatch strictness (owner call).** Exact equality means a cosmetic-only
  mod update blocks a friend on the previous version. The alternative is an
  author-declared compatibility key distinct from the display version — one more field,
  and it moves the judgement to the author who can actually make it. Cheap to add
  later; noted in case the friction is expected to bite early.
- **`networking.md` update at promotion.** The fingerprint-binds-the-connection
  sentence is overturned, the slot lifecycle gains two states, and the handshake
  section describes one app-level message where there will be two.
