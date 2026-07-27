# Session Lifecycle (E15 Phase 3.75)

## Goal

A client joins a session before it knows which map to load, and the session survives
the host changing levels. Today the app-level handshake carries the loaded map's
fingerprint and both peers refuse to evaluate it until a level installs, so "connected"
and "on the host's map" are the same state; a fingerprint change closes every
connection; no message tells a client what to load; and the transport is not polled
across the unload→install window, so a slow load times peers out regardless.

Split the gate into **admission** (protocol compatibility and matching mod identity, no
map involved) and **content parity** (the level, revalidated at every install). A host
level change then **demotes** its clients rather than closing them, names the next map
over the wire, and clients follow. The connection outlives the map it joined on.

## Prerequisites

- **Epic 15 Phase 1** (shipped) — the two-gate handshake, the reliable Control channel,
  the typed reject reason, and the protocol/wire constants this restages.
- **Epic 15 Phase 2** (shipped) — the slot lifecycle and its close events, and the
  per-slot cleanup a demotion reuses.
- **`mod-map-catalog`** (shipped) — the catalog id as the stable logical handle for a
  map, and `LevelSource::Catalog` resolving against the engine-global catalog. The
  relevel message names a catalog id, so the catalog is what makes one string enough.
- **`runtime-level-lifecycle`** (shipped) — the queued load/unload request path a
  followed relevel enqueues into. It scoped "networked or mid-level hot-swap" out; this
  spec is where that returns.

## Scope

### In scope

- **Two-stage gate.** Admission (protocol + mod identity) and content parity (level
  identity + fingerprint), as separate control messages evaluated at separate times.
- **A slot state between pending and participating.** An admitted slot holds a live
  connection and receives no entity state.
- **Demotion instead of closure** on a host level install, running the same per-slot
  cleanup a close runs today.
- **Mod identity in the manifest** — a stable id and a version, compared for exact
  equality at admission.
- **A relevel message** — server→client, naming the next map's catalog id — and the
  client-side follow that enqueues the load through the shipped request path.
- **Host-side net reset on level unload**, which today early-returns for the host role.
- **Transport polling across the unload→install window**, so a load longer than the
  netcode timeout does not drop every peer.
- **A typed reject reason delivered to the client**, distinguishing protocol, mod, and
  content divergence.
- **Level identity as its own compared value**, closing the fingerprint's fail-open.

### Out of scope

- **Player identity, seats, and the roster.** Spec 2 of the band. This spec decides
  whether a *connection* may participate, never who is behind it.
- **Authored join policy.** Spec 3 of the band. Admission is engine mechanism; the
  predicate that gates it has nothing to bind against until a roster exists.
- **Reconnect after a close.** A closed connection stays closed. Demotion is what keeps
  a connection alive; a dropped peer relaunches.
- **Host migration and graceful host-leave.** Recorded on the roadmap as a later
  aspiration. This spec opens the session-state ledger (see Decisions) but does not
  serialize it.
- **A client asking the host to change level.** Map authority is server-owned. The
  authorized-requester concept arrives with host-as-client packaging (Phase 4).
- **Shipping mod content to a client that lacks it.** Networked mod sync is a stated
  non-goal (`boot_sequence.md` §8). Matching is in scope; distribution is not.
- **Tamper resistance.** Mod identity is declared, not proven — see Decisions.
- **Hashing the `.prl` bytes** and **widening the fingerprint's domain.** Both decided
  against, not deferred by omission — see Decisions.
- **Frontend/lobby presentation.** A demoted client with no level renders the ordinary
  world-less Frontend state; menus and roster UI are spec 3.

## Direction

**Problem.** One structural coupling in a shipped message causes all of it: gate 2
carries protocol constants and a map fingerprint in a single payload, and both peers
early-return until a level installs. That makes "connected" and "on the host's map" the
same state, so no state exists for a client that has connected but has not yet been told
what to load — and with no such state, there is nowhere for a relevel protocol to live.
Every remaining capability in the band sits behind it.

**Evidence, with its status stated.** Mostly **anticipation**: no join flow exists at
all, and today both peers are launched with the same map on the command line, so nobody
has hit the inversion because nobody can reach it. That is legitimate here because the
band's other two specs are blocked on it and the roadmap has committed to them. Two
**live defects** are also folded in: the fingerprint fails open (two mover-less maps
hash identically, so the gate passes and clients stay attached to a host where their
pawns no longer exist), and the transport goes unpolled across every load.

**Prior commitments.**

- *Two gates catch different failures at different layers* (`networking.md`). Extended
  along the same reasoning: "compatible build and mod" and "same map" are also different
  failures, and they become true at different times.
- *No entity state reaches a client that has not passed the gate.* Preserved. The send
  path gates on participation, strictly narrower than today's accepted.
- *Exact-match validation, refuse rather than migrate* (`networking.md`, mirroring the
  `BakedIr` version-epoch discipline). Mod version is exact string equality.
- *Explicit author-assigned id, not a content-derived hash* (`E16--impact-policy-substrate`,
  which spells out why: a derived id changes when content is edited and silently orphans
  references). This is the project rule that makes declared mod identity correct rather
  than merely convenient.
- *The catalog `id` is the stable logical handle every reference uses, decoupled from
  `path`* (`mod-map-catalog`). The relevel message and the parity message both name that
  handle rather than a path.
- *The manifest is the foundation for mod identity* (`M7--mod-script-layer`, which
  anticipated exactly this field). Redeemed here.
- *Session state must be enumerable, not scattered* (roadmap Phase 3.75,
  `coop-session-lobby.md` §6). This spec opens that ledger — see Decisions.
- *Design against host-as-client from day one* (roadmap Epic 15). Honored: every path
  runs through slots, and map authority is server-owned with no client-issued load
  request, so a future loopback host-client demotes and follows like any other peer.
- *The net crate is registry-blind and postretro-free.* Preserved — mod identity and
  level identity cross as opaque strings the crate compares and never interprets.
- **Divergence 1, named.** `networking.md`: "a connection is bound to that fingerprint
  for its lifetime. Installing different static mover content closes it." Overturned
  deliberately. That rule encodes a *missing feature*, not a safety property — closing
  is the only safe response to unvalidatable content when no protocol exists for reaching
  agreement. Demotion preserves the actual guarantee (nothing replicates while content is
  unproven) and drops only the disconnection.
- **Divergence 2, named.** `mod-map-catalog` establishes the manifest's hot-reload
  discipline as *atomic replace at the staged-commit boundary*, degrade-never-abort.
  Mod identity is **first-commit-wins across reloads** instead, so after this spec every
  manifest lane is atomic-replace except these two fields. Deliberate: mid-session
  identity churn would silently invalidate admission decisions already made for live
  connections. This is a divergence from the manifest's commit discipline, not an
  instance of it — the `boot_sequence.md` persistence-overlay rule it resembles is a
  different lane.

**Placement.** The gate stays in `postretro-net`: pure comparison over opaque values,
where both existing gates live, unit-testable without a socket. The compared values are
engine-supplied and installed after construction, exactly as the fingerprint already is —
mod identity is not available when `Session::build` constructs the endpoint, because mod
init runs later in boot. The *level-transition* half is engine-side by necessity: it
drives the shipped `LevelRequest` path and touches the registry, both of which the net
crate must not see.

**Alternatives rejected.**

- *Keeping admission and transitions as two specs.* The original decomposition, reopened
  after review. Rejected on three signals, all generated while drafting the smaller
  version: the admission spec had to pull the fail-open fix forward because its own
  invariant failed silently without it; its two headline acceptance criteria were claims
  about surviving a window the transition spec owned; and the transition spec was already
  described as likely to split along a different seam. The work divides by *layer* (gate,
  wire, engine lifecycle), not by *capability*.
- *One message, evaluated twice.* Keep the shipped handshake and have the client resend
  it on every level install. The cheapest rival, in two variants. With the fingerprint
  field required, a client with no level cannot send one — but that is an argument about
  today's type, so the honest variant is an *optional* fingerprint field. Rejected on
  timing rather than typing: admission becomes true at mod init and parity at every level
  install, so one message either blocks on the later fact or re-sends the earlier one
  redundantly, and the reject reason cannot say which half diverged.
- *Admit on protocol alone; check the mod at first level install.* Fewer moving parts.
  Rejected because it reports the wrong cause — a player on the wrong mod would be told
  the map does not match — and because a relevel naming a catalog id is only sound if the
  mods match, since the catalog is mod-supplied.
- *Close and reconnect on level change.* Preserves the shipped invariant verbatim and
  reaches the same end state. Rejected on three counts, none of which is prediction
  safety: no reconnect path exists (the client's handshake flag never resets), the
  unload→install window is unpolled so the reconnect would race a timeout, and a
  reconnect re-mints the client id, which is wall-clock nanos per connection. The last
  matters most downstream — `E16--per-player-currency`'s third shape died on precisely
  this, its "value survives a level transition" criterion contradicting its
  seat-released-on-disconnect rule because a level change closed every connection.
- *Folding level identity into the fingerprint hash.* Considered and rejected after
  review. The fail-open is an *identity* failure ("different maps"), not a *content
  parity* failure ("prediction inputs diverge"); making a content hash accidentally
  identity-sensitive mixes the categories, makes the most common mismatch an opaque
  32-byte diff instead of a readable map name, and costs a rename, an epoch bump, and a
  `networking.md` domain rewrite that carrying one more field does not.

**Foreclosures and one-way doors.**

- **One mod id/version pair on the wire forecloses mod stacking** without a protocol
  change. `M7--mod-script-layer` anticipates mod inheritance; a stack needs an ordered
  set and set-comparison semantics, not a wider field. Cheap to accept today because the
  active mod root is singular and fixed at init, but it is a foreclosure, not an accident.
- **The one-way door is the *requiredness* of mod identity, not its wire visibility.**
  A wire-visible-but-optional identity (required to pass admission, warn-and-continue at
  mod init) would be reversible. Required costs a protocol bump plus a manifest migration
  for every mod, and a breaking edit to a published SDK type. Accepted: it is consistent
  with how the manifest already fails on a missing `name`, and an optional identity is
  meaningless for exactly the mods most likely to drift.
- **Level identity distinguishes addressing modes, not just maps.** A host on the
  raw-path dev bypass and a client on the catalog id for the same `.prl` will not match.
  Mitigated rather than accepted: identity falls back to the content-root-relative path
  for uncatalogued levels, so the documented two-process loopback recipe (both peers
  launched with the same path) still matches, and the mismatch that remains is reported
  by name rather than as a hash diff.
- Everything else is net-crate-local behind two hand-bumped constants: the two messages,
  the new slot state, the rename of accepted to participating, the demotion transition,
  the deferred disconnect. Backing any of them out is a normal change.

## Decisions

- **Two control messages, not one.** Admission carries the two protocol constants plus
  mod id and version; parity carries level identity and the content fingerprint. They
  become true at different times, which is the whole reason they are separate.
- **Mod identity is declared, not derived.** Author-chosen id and version, exact string
  equality. A compatibility check: it catches the wrong mod and a stale version, which is
  the real failure among friends. It does not catch tampering and must not be documented
  as if it does — anti-cheat is a stated non-goal.
- **Exact version equality, no ranges.** A semver range needs a compatibility policy the
  project cannot test. The author bumps the version when the shared contract changes.
- **Mod id charset validated at parse** — `[A-Za-z0-9_.:-]`, matching the catalog id's
  role as a stable logical handle. Version is any non-empty string; compared, never
  ordered.
- **Both fields required**, in every shipped manifest. See Foreclosures for the cost.
- **First-commit-wins for mod identity under hot reload** — a divergence from the
  manifest's atomic-replace discipline, argued above.
- **Level identity is the catalog id, falling back to the content-root-relative path**
  for an uncatalogued level. It is opaque to the net crate. The fingerprint keeps its
  shipped domain — mover authoring and collision — unchanged, unrenamed, and unbumped.
- **Parity compares both values and reports which diverged.** Identity mismatch names
  both maps; fingerprint mismatch means same map, different content.
- **A relevel names a catalog id, so a host on a raw-path level sends none.** Its clients
  stay admitted until they install a matching level themselves. This keeps the documented
  loopback dev recipe working and keeps the relevel message honest — the catalog is the
  only shared namespace in which one string resolves on both peers.
- **A demotion runs the same cleanup as a close.** Level unload invalidates every id the
  per-slot tables hold, so a demoted slot's pawn, replication, ownership, command,
  state-slot, and combat state clear exactly as a close clears them. The connection is
  what survives, not the state.
- **This spec opens the session-state ledger with one entry: the connection.** The
  roadmap requires session-surviving state be enumerated rather than accreted. Today the
  enumeration is short and defined by subtraction, and that is stated deliberately so the
  next spec's seat and roster are added to a named list rather than discovered.
- **The transport is polled during Loading, for transport advance and keepalive only.**
  No snapshot apply, no game logic — there is no world during a load. Without it the
  netcode timeout, not the design, bounds how long a level may take to install.
- **A reject sends its reason before disconnecting.** The slot closes immediately so no
  further traffic is honored; only the socket teardown defers one poll, letting the
  reliable message flush. Without it a player on the wrong mod cannot distinguish a
  version mismatch from an unreachable host, which is most of what mod matching is for.
- **Reject reasons become a typed enum over three causes** — protocol, mod, content —
  each carrying expected and received. `RejectReason` and `HandshakeOutcome` lose `Copy`
  and gain `Clone`; call sites update in the same pass.
- **Both protocol constants bump.** New message vocabulary bumps the app protocol id; the
  changed layout bumps the wire version. Independent bumps, both apply.
- **Single-player is untouched.** No endpoint is constructed, so no gate runs, no relevel
  is sent, and no path branches on player count.

## Acceptance criteria

- [ ] A client connecting to a host with no level installed is admitted, holds the
      connection open, and receives no entity records.
- [ ] It begins receiving entity records once — and only once — host and client have
      installed levels whose identity and fingerprint both match.
- [ ] A client whose declared mod id or version differs is refused, is told which of the
      two diverged, and receives no entity records.
- [ ] A client whose protocol constants diverge is refused with a protocol cause, not a
      mod or content cause.
- [ ] A refused client observes the typed reason before its connection closes.
- [ ] Two maps that differ only in ways the fingerprint ignores — including two maps with
      no movers at all — produce different parity values, and a client on one does not
      participate on the other.
- [ ] A content mismatch names the host's map identity in the reject reason; a
      same-identity fingerprint mismatch is reported as a content divergence rather than
      an identity one.
- [ ] A host installing a different catalog level **keeps** its clients connected: each
      drops to admitted, stops receiving entity records, and its pawn, replication,
      ownership, command, state-slot, and combat state are cleared exactly as a
      disconnect clears them.
- [ ] Those clients load the host's new map without being told out of band, then
      re-participate — with their client ids unchanged across the whole transition.
- [ ] A level install that takes longer than the netcode timeout does not drop any peer;
      connections established before the load are still connected after it.
- [ ] A host on a raw-path level sends no relevel and does not disconnect its clients;
      the documented two-process loopback recipe — both peers launched with the same map
      path — still reaches participation.
- [ ] A manifest missing the mod id or version, or whose id violates the charset, fails
      mod init with a diagnostic naming the field.
- [ ] A staged hot reload that changes the mod id or version warns and leaves the
      installed identity unchanged; admitted clients stay admitted.
- [ ] Single-player boot constructs no endpoint and reaches Running unchanged.
- [ ] A peer built before this change is refused at the transport gate, before any app
      message is decoded.

## Tasks

### Task 1: Extract the handshake gate

`crates/net/src/transport.rs` is ~740 non-test lines and this spec adds a second gate
stage plus a message family to it. Split first, behavior-preserving: move the pure gate
surface into a new `crates/net/src/handshake.rs` beside `slots.rs` — the wire-comparison
types, the validation function, the reject reason and its `Display`, the protocol-constant
accessors, and the malformed/hex helpers. `transport.rs` keeps the renet plumbing: socket,
channels, `NetServer`/`NetClient`, the poll loop, and send gating. Re-export from `lib.rs`
so no downstream import path changes. Relocate the existing gate unit tests with their
subject. No behavior change.

### Task 2: Mod identity in the manifest

Add a required stable id and a required version to the mod manifest. Declare both on the
`ModManifest` type in `sdk/types/postretro.d.ts` and mirror them in the Luau typedef,
documenting that peers compare them for exact equality and that this is a compatibility
check, not a security one. Parse both at **four** sites — the JS and Luau manifest readers
in `crates/scripting-core/src/runtime/mod_init_exec.rs` and their staged-reload
counterparts in `crates/scripting-core/src/staged_manifest.rs` — each following the
existing required-`name` shape, so a missing field is an `InvalidArgument` naming the
field and the source path. Validate the id against `[A-Za-z0-9_.:-]` at parse; the version
is any non-empty string. Carry both on `ModManifestResult` beside `name`. Commit them with
the rest of the manifest at mod init, but make the commit **first-wins across reloads**: a
staged manifest whose identity differs from the installed one logs a warning and leaves
the installed value alone. This deliberately diverges from the atomic-replace discipline
every other manifest lane follows — comment the divergence at the commit site, because a
future reader will otherwise read it as a bug. Update every shipped manifest under
`content/` to declare both fields.

### Task 3: Two-stage gate and slot demotion

Replace the single app-gate message with two, in Task 1's module. **Admission** carries the
two protocol constants plus mod id and version; **content parity** carries a level identity
string and the existing fingerprint. Both ride the reliable Control channel; both compare
opaque values the crate never interprets. Make the reject reason a typed enum over three
causes — protocol, mod, content — each carrying expected and received, with the content
cause distinguishing an identity mismatch from a same-identity fingerprint mismatch;
`RejectReason` and `HandshakeOutcome` lose `Copy` and gain `Clone`, and their call sites in
this crate update in the same pass. Add an `Admitted` state to `SlotTable` between
`Pending` and `Accepted`, rename `Accepted` to `Participating`, and add the demotion
transition `Participating → Admitted` emitting a new lifecycle event beside the existing
close event. Keep `Closed` terminal and every existing idempotence property. `NetServer`
holds mod identity and the level identity/fingerprint pair as separate `Option`s installed
after construction: admission evaluates once identity is present, parity once the level
pair is present, each queuing until then — extending the shipped early-return rather than
adding a second waiting mechanism. Installing a different level pair **demotes**
participating slots instead of closing them. Gate `send_snapshot` and the accepted-client
accessor on participating only. On reject, enqueue the typed reason on Control and close
the slot immediately, but defer the socket disconnect to the next poll via a small
pending-disconnect list on `NetServer`, so the reliable message flushes first. On the
client, split `handshake_sent` into an admission flag sent once on connect and a parity
flag re-armed whenever the level pair changes, replacing today's self-disconnect. Bump
`PROTOCOL_ID` and `WIRE_VERSION`. Unit-test the gate and the slot machine without sockets.

### Task 4: Relevel message and client-follow

Give the host a way to name the next map and the client a way to follow it. Add a
server→client relevel message on the reliable Control channel carrying one map catalog id,
sent to every admitted and participating slot when the host installs a catalogued level and
on admission for a client joining a host that already has one — so a late joiner is told
the current map without waiting for the next transition. A host whose active level has no
catalog id sends nothing. On the client, surface received relevel messages out of the
endpoint poll as a typed value the engine drains; the engine enqueues
`LevelRequest::Load(LevelSource::Catalog(id))` through the shipped request path, which
already unloads any active level first. Ignore a relevel naming the level already active or
already in flight, so a redundant send does not restart a load. An id absent from the local
catalog logs a diagnostic naming the id and leaves the client admitted — it is the
recoverable case, since the mods matched but the catalogs diverged.

### Task 5: Poll the transport across the load window

`net_poll_and_apply` is reached only from the Running gameplay block, so neither peer
exchanges packets between level unload and install and the netcode timeout alone bounds how
long a load may take. Advance the net endpoint from the loading frame in
`crates/postretro/src/startup/lifecycle.rs` — transport advance, handshake processing, and
keepalive only. Do **not** apply snapshots or run any game logic there: there is no world
during a load, and the snapshot-apply ordering contract (apply before state-crossing
detection, within the Game-logic stage) has no meaning outside Running. A client in Loading
must still deliver received relevel messages, since a relevel can arrive while an earlier
load is in flight. Cover the window with a test that holds a load open past the timeout and
asserts the connection survives.

### Task 6: Engine lifecycle wiring

Wire the engine's halves to the new gate. **Mod identity:** after mod init commits, install
the committed id and version on the net endpoint through a setter mirroring the
fingerprint's, reached through the same `session.net_endpoint` borrow `install_level_payload`
uses; single-player has no endpoint and skips. **Level identity:** derive it in
`install_level_payload` from `App.active_level_source`, which `retain_active_level_tags_for_install`
populates on the line immediately before the fingerprint is computed — the catalog id when
present, otherwise the content-root-relative path — and install it alongside the unchanged
fingerprint. **Accept seam:** the pawn spawn currently keyed off the accept outcome in
`main.rs` moves to the participation transition; a pawn needs a level, which is what parity
now proves. **Demotion:** route the new demotion event into `host_handle_lifecycle` so it
runs the same per-slot cleanup a close runs — do not duplicate that cleanup. **Host unload
reset:** `reset_level_scoped_client_state` early-returns for the host role today; give it a
host arm that clears the level-scoped host tables (movement owners, slot pawns, replicable
set, weapon owners, open shots, command queues) whose entries the unload has invalidated,
and confirm the host state-replication schema is rebuilt per level rather than cached for
the process. Keep the `main.rs` edit to redirecting the two triggers — splitting that file
is out of scope and explicitly deferred by `runtime-level-lifecycle`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the split every net-crate edit lands in.
**Phase 2 (concurrent):** Task 2, Task 3, Task 5 — scripting-core, the net crate, and the
boot loading frame are disjoint; Task 5's polling is safe against both the old and new gate.
**Phase 3 (sequential):** Task 4 — consumes Task 3's slot states and message family.
**Phase 4 (sequential):** Task 6 — consumes the setters from Task 3, the committed identity
from Task 2, and the client-follow drain from Task 4.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No entity state reaches a slot below participating | Task 3 (send gating) | Every new send path must gate on participation, not admission or connection | AC 1, 3, 8 |
| Content parity is proven for the *current* level, not the joining one | Task 3 (demotion on level change), Task 6 (per-install identity + fingerprint) | A demotion that failed to clear state would leave stale ids addressable | AC 8, 9 |
| A demotion clears exactly what a close clears | Task 3 (event), Task 6 (routed into the existing cleanup) | Shared with the close path — a second cleanup implementation would drift | AC 8 |
| Level identity discriminates any two distinct levels | Task 6 (catalog id, path fallback) | The per-level gate is a no-op if two levels can collide | AC 6, 11 |
| A connection's id survives a level change | Task 3 (demote, never close), Task 5 (window stays polled) | Later specs key player identity off a connection that must not be re-minted | AC 9, 10 |
| Admission and parity queue independently until their source installs | Task 3 (two `Option`s, two early returns) | Coupling them re-creates the ordering inversion this spec removes | AC 1, 2 |
| A refused peer learns the cause before teardown | Task 3 (deferred disconnect) | A future reject path that disconnects inline drops the message | AC 3, 4, 5, 7 |
| A relevel never restarts the load it names | Task 4 (active/in-flight suppression) | Late-join and transition both send; a third sender must suppress too | AC 9 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod id | `ModManifestResult` field | admission message field | `id: string` | `id: string` |
| mod version | `ModManifestResult` field | admission message field | `version: string` | `version: string` |
| level identity | engine-derived `String` | parity + relevel message field | catalog `id` (existing) | same |
| content fingerprint | `[u8; 32]`, unchanged domain | parity message field | n/a | n/a |
| admission message | net-crate handshake type | Control, client→server | n/a | n/a |
| parity message | net-crate handshake type | Control, client→server | n/a | n/a |
| relevel message | net-crate handshake type | Control, server→client | n/a | n/a |
| reject reason | typed enum, three causes | Control, server→client | n/a | n/a |
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
  // Unchanged, and now load-bearing for co-op: the host names one of these ids when
  // it changes level, and clients resolve it locally.
  maps: defineMapCatalog([
    { id: "combat-demo", path: "maps/combat-demo.prl", name: "Combat Demo" },
  ]),
});
```

## Open questions

- **Version-mismatch strictness (owner call).** Exact equality means a cosmetic-only mod
  update blocks a friend on the previous version. The alternative is an author-declared
  compatibility key distinct from the display version — one more field, moving the
  judgement to the author who can actually make it. Cheap to add later; noted in case the
  friction bites early.
- **Reject-reason delivery is the trimmable part.** The deferred disconnect is one list
  and one poll, and the rest of the spec works without it. If it fights renet's teardown,
  the fallback is a host-side log only, at the cost of a player who cannot tell a wrong
  mod from an unreachable host.
- **Mid-load relevel.** Task 4 suppresses a relevel naming the in-flight level, but a
  relevel naming a *different* map while a load is in flight is left to the shipped request
  path's ordering. If that path cannot preempt an in-flight load cleanly, the honest v1 is
  to queue the newer request and apply it on completion.
- **`networking.md` update at promotion.** The fingerprint-binds-the-connection sentence is
  overturned, the slot lifecycle gains a state and a transition, the handshake section
  describes one app message where there will be three, and the crate boundary gains a
  server→client control message family.
