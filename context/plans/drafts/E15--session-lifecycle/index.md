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

- **Two-stage gate.** Admission (protocol constants + mod id + mod digest) and content
  parity (level identity + level digest), as separate control messages evaluated at
  separate times.
- **A slot state between pending and participating.** An admitted slot holds a live
  connection and receives no entity state.
- **Demotion instead of closure** on a host level install, running the same per-slot
  cleanup a close runs today.
- **Mod identity in the manifest** — a stable id that gates admission, and a version that
  rides the wire for display only.
- **Two content-derived compatibility digests** — one over the mod's simulated surface,
  one over the level's — replacing author-declared version equality as what actually
  gates. The versioning policy, in one sentence: compatibility is decided by content.
- **A relevel message** — server→client, naming the next map's catalog id — and the
  client-side follow that enqueues the load through the shipped request path.
- **Host-side net reset on level unload**, which today early-returns for the host role.
- **Transport polling across the unload→install window**, so a load longer than the
  netcode timeout does not drop every peer.
- **A typed diagnostic delivered to the client**, distinguishing protocol, mod, and
  content divergence. Protocol and mod causes refuse and close; a content cause is
  informational and the slot holds — see Decisions.
- **Level identity as its own compared value**, and **the fingerprint widened to cover
  static world collision geometry** — closing both of the fail-opens a client can hit.

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
- **`loadLevel`'s co-op semantics.** The shipped system reaction still loads locally on
  every peer, a connected client included. `coop-session-lobby.md` §6 records that it must
  eventually become a request the server may refuse, inert on a non-authoritative client —
  a semantic change to a published primitive (`index.md` §2). Deferred to spec 3 with the
  rest of the authoring surface, and named here rather than left to silence. Under this
  spec's hold-on-parity-mismatch rule the interim behavior is benign: a client whose mod
  loads a different level stops participating, is told why, and re-participates at the
  host's next relevel. It is not a disconnect.
- **Shipping mod content to a client that lacks it.** Networked mod sync is a stated
  non-goal (`boot_sequence.md` §8), and it stays one on its own merits, not by inheritance.
  Scripts are the cheap part — 160K against 337M of art in the dev mod, and small by
  construction since the IR cannot iterate — but sending them fixes only the script-side
  third of the breaking surface and leaves static collision, which is the expensive part and
  the part with the worst failure mode. It also inverts boot ordering (mod init runs after
  `Session::build`), requires re-committing manifest lanes the staged path does not cover,
  and feeds peer-controlled input to a C interpreter. Matching is in scope; distribution is
  not, and if it ever lands it belongs out-of-band, not on a reliable game channel. Reasoned
  through in `research/coop-content-compatibility.md`.
- **Tamper resistance.** Mod identity is declared, not proven — see Decisions.
- **Hashing the `.prl` bytes wholesale.** Decided against, not deferred by omission — it
  makes a cross-platform bake difference a hard connection failure, which is a
  bake-determinism question this spec has no standing to answer. Widening the fingerprint to
  the *static collision* it already should have covered is in scope and is a different
  thing; see Decisions.
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
**live defects** are also folded in. The fingerprint fails open **twice**: it covers mover
data only, so two mover-less maps hash identically, and — the larger hole — it never covers
the static world collision that client movement prediction and client-authoritative hit
declaration both run against, so two maps with different brushwork can pass the gate and
leave clients fighting reconciliation and having legitimate shots false-rejected. And the
transport goes unpolled across every load.

**Prior commitments.**

- *Two gates catch different failures at different layers* (`networking.md`). Extended
  along the same reasoning: "compatible build and mod" and "same map" are also different
  failures, and they become true at different times.
- *No entity state reaches a client that has not passed the gate.* Preserved. The send
  path gates on participation, strictly narrower than today's accepted.
- *Exact-match validation, refuse rather than migrate* (`networking.md`, mirroring the
  `BakedIr` version-epoch discipline). Preserved in substance and moved to a better carrier:
  what is matched exactly is a content digest, not an author-typed version string. Refuse,
  never migrate, still holds.
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
- **Divergence 2, named — and smaller than it first looked.** `mod-map-catalog`
  establishes the manifest's hot-reload discipline as *atomic replace at the staged-commit
  boundary*, degrade-never-abort. Mod identity is **first-commit-wins across reloads**
  instead. Deliberate: mid-session identity churn would silently invalidate admission
  decisions already made for live connections. But the same plan records that only
  `entities` and `store_declarations` are re-committed on a staged reload — *theme and
  fonts are not* — so a manifest lane that is not atomic-replaced already ships. Mod
  identity joins an existing minority rather than becoming the first exception. Worth a
  comment at the commit site as a second instance, not as a warning about a unique one.

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
  reaches the same end state. Two obvious objections do not survive scrutiny, and are named
  so they are not reached for again: the unpolled window is fixed by this spec's own Task 5,
  so it cannot also be a reason to reject the rival; and the client-id re-mint (wall-clock
  nanos per connection) is precisely what spec 2's seat exists to stop keying off. What does
  reject it: no reconnect path exists at all — the client's handshake flag never resets and
  the endpoint is constructed once in `Session::build` — so the rival is *more* work, not
  less; and it turns every level change into a fresh renet_netcode teardown and rebuild,
  which on a direct-connect transport with no relay is a fresh NAT, firewall, and handshake
  failure opportunity. A session that plays four maps runs that gauntlet four times.
  Downstream it is also what `E16--per-player-currency`'s third shape died on: its "value
  survives a level transition" criterion contradicted its seat-released-on-disconnect rule,
  because a level change closed every connection.
- *No new slot state — keep `Pending`/`Accepted`/`Closed` and make participation
  `accepted && parity_ok` in a side map.* The cheapest rival to the slot-machine change.
  Rejected because a demotion needs a transition *edge* to fire the per-slot cleanup on, and
  a boolean flipping in a side table gives you nothing to hang that event on — the cleanup
  is the half that matters. A side map is also exactly the "second waiting mechanism" the
  gate design in Task 3 refuses on its own terms.
- *Folding level identity into the fingerprint hash.* Considered and rejected after
  review. The fail-open is an *identity* failure ("different maps"), not a *content
  parity* failure ("prediction inputs diverge"); making a content hash accidentally
  identity-sensitive mixes the categories and makes the most common mismatch an opaque
  32-byte diff instead of a readable map name. This is *not* an argument against widening the
  hash to static collision, which the spec does: collision geometry is a deterministic
  prediction input, the same category as the mover collision already hashed. The rule is that
  the digest answers "is the content the same" and identity answers "which map" — widening
  respects it, folding identity in would not.

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

- **Two control messages, not one.** Admission carries the two protocol constants plus mod
  id and the mod compatibility digest; parity carries level identity and the level content
  digest. They become true at different times, which is the whole reason they are separate.
- **Compatibility is a property of content, not a promise by the author.** Two
  content-derived digests gate, one per stage; no hand-maintained version string does. The
  reasoning is in `research/coop-content-compatibility.md`: what can break a co-op session
  is exactly what a client computes locally and the host never corrects, and that set is
  small, enumerable, and mechanically hashable. An author-declared compatibility key moves
  the judgement to a human who will get it wrong silently, and its failure mode is
  prediction fighting rather than a clean refusal.
- **Mod id is declared and gates; mod version is declared and does not.** The id is the
  namespace that makes a catalog id resolvable, so it must match. The version is required in
  the manifest, rides the admission message, and is used for display and diagnostics only —
  it never refuses a connection. Exact-version equality was the first draft's rule and is
  dropped: it blocks a friend on the previous build over a lighting tweak, which is a change
  no client ever simulates.
- **Mod id charset validated at parse** — `[A-Za-z0-9_.:-]`, matching the catalog id's
  role as a stable logical handle. Version is any non-empty string; displayed, never
  compared for admission and never ordered.
- **Both fields required**, in every shipped manifest. See Foreclosures for the cost.
- **First-commit-wins for mod identity and the mod digest under hot reload** — a divergence
  from the manifest's atomic-replace discipline, argued above. A staged reload that changes
  either logs a warning naming the live connections whose digest is now stale. Hot reload is
  debug-only, so this is a release guarantee and a debug best-effort — stated rather than
  hidden, and the alternative (re-hashing on reload) closes every connection on every script
  edit, which is the dev-loop breakage that rejected a whole-mod content hash in the first
  place.
- **The mod compatibility digest covers `entities` and `store_declarations` only.** Those
  are the manifest lanes a client simulates against: the entity class names it materializes
  by, the `PlayerMovementDescriptor` tuning it predicts with, and the state-slot schema it
  applies replicated records into. `render`, `theme`, `fonts`, `ui_trees`, and `frontend`
  are deliberately excluded — they are presentation, and hashing them would break co-op on
  every UI tweak. `maps` is excluded too: a catalog divergence is the *recoverable* case
  this spec already handles by name.
- **The level content digest is the existing fingerprint, widened to cover static world
  collision.** `CollisionWorld::populate_from_level` builds the client's local trimesh from
  `LevelWorld::vertices` and `LevelWorld::indices`; client movement prediction runs against
  it, and so does client-authoritative hit declaration, which the host then validates
  against *its* static geometry. Nothing hashes it today. That is the same silent fail-open
  as the mover-less case, in the same place, and it is fixed by the same rule that put mover
  collision in the fingerprint: a deterministic prediction input belongs in the parity hash.
  `FINGERPRINT_EPOCH` bumps.
- **Level identity is the catalog id, falling back to the content-root-relative path**
  for an uncatalogued level. It is opaque to the net crate, and it answers a different
  question from the digest — *which map*, rather than *is the content the same* — which is
  why it stays a field beside the hash instead of being folded into it.
  A catalog id is **mod-scoped**, which is still why identity alone cannot discriminate: two
  mods may each declare a map `id: "combat-demo"` over different `.prl` files, so two peers
  on different mods compare identity equal. An earlier draft argued that the mover-less
  collision made mod matching load-bearing for the fail-open fix; **widening the fingerprint
  retires that specific argument**, since differing brushwork now diverges on content. What
  replaces it is stronger: the two digests are halves of one policy, installed at the two
  moments this spec already installs values. A fork that changes only scripts ships
  identical map bytes and is caught at admission, never at parity; a map edit is caught at
  parity, never at admission. Neither stage covers for the other, which is why they belong
  in one spec.
- **Parity compares both values and reports which diverged.** Identity mismatch names
  both maps; digest mismatch means same map, different content.
- **A parity mismatch holds the slot at admitted; it never closes it.** Admission facts —
  protocol, mod — are connection-scoped and can never become true later, so a mismatch there
  is terminal and closes. Parity is level-scoped and is *designed* to become true one install
  later; closing on a fact scheduled to change is a category error, and it would reintroduce
  for the client-initiated case exactly the disconnect this spec removes for the
  host-initiated one. It would also race the spec's own criteria: a client's parity message
  for level A can still be in flight when the host installs level B, so a host that closed on
  mismatch would tear down a client it demoted one frame earlier. The content cause is
  therefore a *diagnostic* delivered to a still-connected client, and the deferred-disconnect
  mechanism serves admission rejects only.
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
- [ ] A client whose declared mod id differs, or whose mod compatibility digest differs, is
      refused, is told which of the two diverged, and receives no entity records.
- [ ] A client whose mod **version** differs but whose id and digest match **participates
      normally**; the version difference appears in a host-side log and nowhere else.
- [ ] A client whose protocol constants diverge is refused with a protocol cause, not a
      mod or content cause.
- [ ] A client refused at admission — protocol or mod — observes the typed reason before
      its connection closes.
- [ ] Two maps that differ only in ways the shipped fingerprint ignored — two maps with no
      movers at all, and two maps whose only difference is static world collision geometry —
      produce different parity values, and a client on one does not participate on the other.
- [ ] Two mods differing only in `entities` or `store_declarations` content produce
      different mod digests; two differing only in `theme`, `fonts`, `ui_trees`, `render`,
      `frontend`, or `maps` produce the **same** digest and interoperate.
- [ ] Declaring the same descriptors and store slots in a different source order produces
      the same mod digest.
- [ ] A client whose level fails parity **keeps its connection**, receives a content
      diagnostic naming the host's map identity, and re-participates at the host's next
      matching install without reconnecting; a same-identity fingerprint mismatch is
      reported as a content divergence rather than an identity one.
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
- [ ] A staged hot reload that changes the mod id, version, or digest warns — naming the
      live connections whose digest is now stale — and leaves the installed values
      unchanged; admitted clients stay admitted.
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
documenting precisely what each is for: peers must declare the **same id** to connect, the
**version is displayed and never compared**, and neither is a security mechanism.
Parse both at **four** sites — the JS and Luau manifest readers
in `crates/scripting-core/src/runtime/mod_init_exec.rs` and their staged-reload
counterparts in `crates/scripting-core/src/staged_manifest.rs` — each following the
existing required-`name` shape, so a missing field is an `InvalidArgument` naming the
field and the source path. Validate the id against `[A-Za-z0-9_.:-]` at parse; the version
is any non-empty string. Carry both on `ModManifestResult` beside `name`. Commit them with
the rest of the manifest at mod init, but make the commit **first-wins across reloads**: a
staged manifest whose identity differs from the installed one logs a warning and leaves the
installed value alone. The same first-wins rule and the same warning cover the mod
compatibility digest Task 7 installs — and because a staged reload *does* re-commit
`entities` and `store_declarations`, the digest can go stale against live content, so the
warning must say so and name the affected connections rather than merely noting a change.
Hot reload is debug-only, so this is a release guarantee and a debug best-effort; re-hashing
instead would close every connection on every script edit. This first-wins rule diverges from
the atomic-replace discipline most manifest lanes follow — comment the divergence at the
commit site, noting that theme and fonts are already non-re-committed, so it is a second
instance and not a unique exception. Update every shipped manifest under `content/` to
declare both fields.

### Task 3: Two-stage gate and slot demotion

Replace the single app-gate message with two, in Task 1's module. **Admission** carries the
two protocol constants, the mod id, the mod compatibility digest, and the mod version;
**content parity** carries a level identity string and the level content digest. Both ride
the reliable Control channel; both compare opaque values the crate never interprets. The
version field is carried for diagnostics and **must not be compared** — comment it at the
comparison site so a later reader does not "fix" the omission. Make the reject reason a
typed enum over three causes — protocol, mod, content — each carrying expected and received,
with the mod cause distinguishing an id mismatch from a same-id digest mismatch and quoting
both peers' declared versions, and the content cause distinguishing a level identity
mismatch from a same-identity digest mismatch;
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
accessor on participating only. On an **admission** reject — protocol or mod — enqueue the
typed reason on Control and close the slot immediately, but defer the socket disconnect to
the next poll via a small pending-disconnect list on `NetServer`, so the reliable message
flushes first. A **parity** mismatch is not a reject: enqueue the content cause as a
diagnostic and leave the slot at `Admitted`, unclosed — parity is level-scoped and becomes
true at the next matching install, so there is nothing to tear down. On the
client, split `handshake_sent` into an admission flag sent once on connect and a parity
flag re-armed whenever the level pair changes, replacing today's self-disconnect. Bump
`PROTOCOL_ID` and `WIRE_VERSION`. Unit-test the gate and the slot machine without sockets.

### Task 4: Relevel message and client-follow

Give the host a way to name the next map and the client a way to follow it. Add a
server→client relevel message on the reliable Control channel carrying one map catalog id,
sent to every admitted and participating slot when the host installs a catalogued level and
on admission for a client joining a host that already has one — so a late joiner is told
the current map without waiting for the next transition. Sending to admitted slots is also
what recovers a slot held on a parity mismatch: it is the message that tells a diverged
client which map would let it participate. A host whose active level has no catalog id
sends nothing. On the client, surface received relevel messages out of the
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

### Task 6: The two compatibility digest recipes

Pure functions plus their tests; Task 7 installs them. Both live engine-side beside the
existing fingerprint recipe, so the net crate keeps treating every compared value as opaque.

**Level content digest.** Widen `kinematic_static_fingerprint` (`runtime_movers.rs`) to hash
the level's static world collision alongside the mover data it already covers: the
`LevelWorld` vertex positions and index buffer that `CollisionWorld::populate_from_level`
builds the client's trimesh from. Hash them with the existing `hash_len`/`hash_f32` helpers
and in the same shape the per-mover vertex/index loop already uses, so a zero-triangle level
is distinguishable from another zero-triangle level by nothing *in this field* — which is
correct, because level identity carries that. Bump `FINGERPRINT_EPOCH` to 2. Take
`&LevelWorld` (or the two slices) rather than `&KinematicGeometry`; the caller in
`install_level_payload` already holds `world`. Rename to reflect the widened domain — it is
no longer kinematic-only. Test that two levels differing **only** in static collision
produce different digests, and that identical geometry with differing entity placements
produces the same one.

**Mod compatibility digest.** New recipe over the committed `ModManifestResult`, hashing
`entities` and `store_declarations` and nothing else — see Decisions for why those two and
why not the presentation lanes. It must be **order-insensitive to declaration order but
sensitive to content**: sort by a stable key (entity `canonicalName`/class name, slot name)
before hashing, so two peers that declare the same descriptors in a different source order
still match. Hash the descriptor fields a client actually simulates against — class name,
`PlayerMovementDescriptor` tuning, the components a client materializes — and the slot
schema; document at the recipe why any excluded field is excluded, because that list is the
compatibility policy in code form. Place it engine-side (a sibling of the level recipe), not
in `scripting-core`: the manifest lane stays unaware of netcode, exactly as the mover
recipe keeps its byte layout out of the net crate.

### Task 7: Engine lifecycle wiring

Wire the engine's halves to the new gate. **Mod identity and digest:** after mod init
commits, compute Task 6's mod digest over the committed manifest and install it with the id
and version on the net endpoint, through a setter mirroring the fingerprint's and reached
through the same `session.net_endpoint` borrow `install_level_payload` uses; single-player
has no endpoint and skips. Install once, first-commit-wins, and on a staged reload that
changes either value log the warning Task 2 specifies. **Level identity and digest:** derive
identity in `install_level_payload` from `App.active_level_source`, which
`retain_active_level_tags_for_install` populates on the line immediately before the digest is
computed — the catalog id when present, otherwise the content-root-relative path — and
install it alongside Task 6's widened level digest, computed from the same `world` already
in scope. **Accept seam:** the pawn spawn currently keyed off the accept outcome in
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
**Phase 2 (concurrent):** Task 2, Task 3, Task 5, Task 6 — scripting-core, the net crate, the
boot loading frame, and the two hash recipes are disjoint. Task 5's polling is safe against
both the old and new gate; Task 6 is pure functions with no caller until Phase 4.
**Phase 3 (sequential):** Task 4 — consumes Task 3's slot states and message family.
**Phase 4 (sequential):** Task 7 — consumes the setters from Task 3, the committed identity
from Task 2, both recipes from Task 6, and the client-follow drain from Task 4.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No entity state reaches a slot below participating | Task 3 (send gating) | Every new send path must gate on participation, not admission or connection | AC 1, 3, 11 |
| Content parity is proven for the *current* level, not the joining one | Task 3 (demotion on level change), Task 7 (per-install identity + digest) | A demotion that failed to clear state would leave stale ids addressable | AC 11, 12 |
| A demotion clears exactly what a close clears | Task 3 (event), Task 7 (routed into the existing cleanup) | Shared with the close path — a second cleanup implementation would drift | AC 11 |
| Level identity discriminates any two distinct levels | Task 7 (catalog id, path fallback) | The per-level gate is a no-op if two levels can collide | AC 7, 14 |
| Every input a client simulates against is covered by a digest | Task 6 (both recipes) | A new client-local simulation input added later is silently ungated unless its recipe is widened too — static collision was exactly that omission | AC 7, 8, 9 |
| Mod version is carried and never compared | Task 2 (SDK docs), Task 3 (commented at the comparison site) | It rides the same message as two gating values; a later reader "completing" the comparison silently reinstates exact-version equality and its false refusals | AC 4 |
| Presentation-only manifest lanes never affect compatibility | Task 6 (digest domain) | Widening the mod digest to theme, fonts, UI trees, render, or frontend breaks co-op on every cosmetic edit | AC 8 |
| A connection's id survives a level change | Task 3 (demote, never close), Task 5 (window stays polled) | Later specs key player identity off a connection that must not be re-minted | AC 12, 13 |
| Admission and parity queue independently until their source installs | Task 3 (two `Option`s, two early returns) | Coupling them re-creates the ordering inversion this spec removes | AC 1, 2 |
| A peer refused at admission learns the cause before teardown | Task 3 (deferred disconnect) | A future reject path that disconnects inline drops the message | AC 3, 5, 6 |
| A parity mismatch never closes a connection | Task 3 (hold at admitted) | Any later content check that rejects instead of holding re-creates the disconnect this spec removes, and races an in-flight parity message against a just-installed level | AC 10, 11 |
| A relevel never restarts the load it names | Task 4 (active/in-flight suppression) | Late-join and transition both send; a third sender must suppress too | AC 12 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod id | `ModManifestResult` field | admission message field | `id: string` | `id: string` |
| mod version | `ModManifestResult` field | admission message field, carried not compared | `version: string` | `version: string` |
| mod compatibility digest | `[u8; 32]`, engine-derived from the committed manifest | admission message field | n/a (derived) | n/a (derived) |
| level identity | engine-derived `String` | parity + relevel message field | catalog `id` (existing) | same |
| level content digest | `[u8; 32]`, widened domain, epoch 2 | parity message field | n/a | n/a |
| admission message | net-crate handshake type | Control, client→server | n/a | n/a |
| parity message | net-crate handshake type | Control, client→server | n/a | n/a |
| relevel message | net-crate handshake type | Control, server→client | n/a | n/a |
| reject reason | typed enum, three causes | Control, server→client | n/a | n/a |
| slot state | `SlotState::{Pending, Admitted, Participating, Closed}` | not replicated | n/a | n/a |

## Script syntax examples

```ts
// Proposed design — two new required fields; everything else is unchanged.
export default defineMod({
  // Stable machine identity. Charset [A-Za-z0-9_.:-]. Peers must declare the same id —
  // this is the only manifest field that gates a co-op join.
  id: "postretro.dev",
  // Display only. Shown in logs and diagnostics; never compared, never ordered, and it
  // cannot refuse a connection. Whether two builds can play together is decided by
  // hashing what a client actually simulates against, not by this string.
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

- **~~Version-mismatch strictness~~ — settled.** Neither exact equality nor an
  author-declared compatibility key. Compatibility is decided by two content digests and the
  version does not gate at all. Recorded here because the reasoning that got there —
  the tiering of what server authority already absorbs — is the durable part, and it lives in
  `research/coop-content-compatibility.md`.
- **Whether the mod digest wants a stable-key sort or a canonical serialization.** Task 6
  specifies sorting by a stable key before hashing so source order does not matter. If the
  descriptor set later grows fields whose own ordering is semantic, that rule needs
  restating rather than extending. Flagged for the detail review, not the owner.
- **Reject-reason delivery is the trimmable part.** It now covers admission rejects only —
  a parity mismatch holds the connection open, so its diagnostic needs no deferral at all —
  which shrinks it to one list and one poll on the protocol/mod path. The rest of the spec
  works without it. If it fights renet's teardown, the fallback is a host-side log only, at
  the cost of a player who cannot tell a wrong mod from an unreachable host.
- **Mid-load relevel.** Task 4 suppresses a relevel naming the in-flight level, but a
  relevel naming a *different* map while a load is in flight is left to the shipped request
  path's ordering. If that path cannot preempt an in-flight load cleanly, the honest v1 is
  to queue the newer request and apply it on completion.
- **`networking.md` update at promotion.** The fingerprint-binds-the-connection sentence is
  overturned, the slot lifecycle gains a state and a transition, the handshake section
  describes one app message where there will be three, and the crate boundary gains a
  server→client control message family.
