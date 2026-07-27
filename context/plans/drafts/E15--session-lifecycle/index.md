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

- **Two-stage gate.** Admission (protocol constants + mod id) and content parity (mod
  digest + level identity + level digest), as separate control messages evaluated at
  separate times. The split is by *mutability*, not by subject: admission carries only
  facts that are immutable for a connection's lifetime, so a mismatch there is terminal.
  Every content-derived value sits in parity, because every one of them can change under
  a live connection and must be re-evaluated when it does.
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
- **A typed diagnostic delivered to the client**, distinguishing protocol, mod-identity,
  and content divergence. Protocol and mod-id causes refuse and close; every content cause
  — mod digest, level identity, level digest — is informational and the slot holds. See
  Decisions.
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
- **Divergence 2, named — and narrowed to identity alone.** `mod-map-catalog`
  establishes the manifest's hot-reload discipline as *atomic replace at the staged-commit
  boundary*, degrade-never-abort. Mod **id and version** are **first-commit-wins across
  reloads** instead. Deliberate: mid-session identity churn would silently invalidate
  admission decisions already made for live connections, and admission is the one stage
  with no recovery path. But the same plan records that only `entities` and
  `store_declarations` are re-committed on a staged reload — *theme and fonts are not* — so
  a manifest lane that is not atomic-replaced already ships. Mod identity joins an existing
  minority rather than becoming the first exception. Worth a comment at the commit site as a
  second instance, not as a warning about a unique one. The mod **digest** does *not*
  diverge: it re-hashes on every staged commit, because its sources are exactly the two
  lanes a staged reload re-commits, and freezing it would leave a live connection gated on a
  value that no longer describes the content the host is running.
- **Divergence 3, named — and it is this spec's own headline decision.**
  `coop-session-lobby.md` §4 as written before this spec read: "the manifest declares an id
  and a version; the client sends them at admission; the host compares. This catches honest
  drift (wrong mod, stale version), which is the actual failure mode among friends." That is
  the position the content-digest decision overturns, and the reasoning for overturning it is
  in `research/coop-content-compatibility.md`: a declared version does not track the breaking
  surface in either direction. Recorded as a divergence because the two documents that now
  agree with this spec — `coop-session-lobby.md` §4 and the roadmap's Phase 3.75 sub-bullet —
  were **rewritten in the same commit that made the decision**. They are restatements, not
  corroboration, and a later reader should not read them as independent support.

**Placement.** The gate stays in `postretro-net`: pure comparison over opaque values,
where both existing gates live, unit-testable without a socket. The compared values are
engine-supplied and installed after construction, exactly as the fingerprint already is —
mod identity is not available when `Session::build` constructs the endpoint, because mod
init runs later in boot. The *level-transition* half is engine-side by necessity: it
drives the shipped `LevelRequest` path and touches the registry, both of which the net
crate must not see.

**Compatibility policy is engine-owned, and that is an exception worth claiming.** The
digest domains — which manifest lanes and which descriptor fields decide whether two peers
can play together — are the compatibility policy, and this spec puts them in engine Rust
with no authoring surface. That runs against a thesis this repo has applied twice:
`E18--coop-activation-policy` put co-op activation in script rather than a KVP, and
`E16--impact-policy-substrate` split engine-emitted *facts* from mod-authored *policy*. The
distinction that makes engine ownership right here is that those are preferences with no
correct answer — how a game wants co-op to behave is the author's to decide — whereas
"do these two peers compute the same thing" has exactly one correct answer, determined by
what the engine's own prediction and hit-declaration code reads. Handing it to an author
does not give them expressive room; it gives them a way to be wrong silently, which is the
whole argument for content-derived compatibility. Stated rather than left for a reader to
notice, because this is the first time the answer is "engine."

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
- *Gating admission on the mod digest — closing on a digest mismatch.* This spec's own
  first shape, dropped on direction review. It was justified by "admission facts are
  connection-scoped and can never become true later," which is true of the protocol
  constants and the mod id and **false of the digest**: a staged reload re-commits
  `entities` and `store_declarations`, so the value the digest describes changes while the
  connection lives. The first shape made its premise true by declining to look — freezing
  the digest at first commit — which bought the premise at the cost of gating live
  connections on a stale value, in debug builds, which is exactly where co-op is developed
  and playtested. It reasoned about the problem as freeze-versus-rehash-and-close and never
  reached the third option its own demotion edge supplies: **rehash and demote**. Rejected
  in favor of that. The dev-loop objection that killed a whole-mod hash does not transfer:
  demotion is not disconnection, and the domain is two lanes rather than every byte, so an
  ordinary script edit does not move it at all.
- *Shipping mod id and the level digest only, deferring the mod digest.* A smaller first
  step. Rejected by this spec's own research rather than on taste: `PlayerMovementDescriptor`
  tuning lives in `entities` and is a client-local prediction input
  (`coop-content-compatibility.md` Tier 3), so deferring the mod digest leaves a hole of
  precisely the class the static-collision widening exists to close — and leaves it open
  while the spec claims to have closed that class.
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

- **Two control messages, not one, split by mutability.** Admission carries the two
  protocol constants plus the mod id and version; parity carries the mod compatibility
  digest, level identity, and the level content digest. They become true at different times,
  which is why they are separate messages — and they *stop* being true under different
  conditions, which is why they are separate lanes. Admission holds only values that cannot
  change for a live connection; everything content-derived is parity, and parity is
  re-evaluated whenever any of its sources is reinstalled.
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
- **First-commit-wins for mod id and version under hot reload; the mod digest re-hashes.**
  Identity is frozen because admission is terminal — a mid-session id change would invalidate
  admission decisions already made for connections with no recovery path, and there is no
  state to demote them to. The digest is the opposite case in every respect: its sources are
  the two lanes a staged reload actually re-commits, it lives in the parity lane, and parity
  *has* a recovery path. So a staged commit that changes `entities` or `store_declarations`
  recomputes the digest, reinstalls it, and demotes any participating slot whose declared
  digest no longer matches — with a diagnostic naming the divergence. That is loud and
  correct where freezing was silent and stale. The dev-loop objection that rejected a
  whole-mod content hash does not reach this: that hash moved on every byte of every script
  and its consequence was a *closed* connection; this one moves only when a simulated lane
  changes and its consequence is a hold that resolves the moment the peers agree again.
  Identity's freeze stays a documented divergence (Divergence 2); the digest is no longer
  part of it.
- **The mod compatibility digest covers `entities` and `store_declarations` only.** Those
  are the manifest lanes a client simulates against: the entity class names it materializes
  by, the `PlayerMovementDescriptor` tuning it predicts with, and the state-slot schema it
  applies replicated records into. `render`, `theme`, `fonts`, `ui_trees`, and `frontend`
  are deliberately excluded — they are presentation, and hashing them would break co-op on
  every UI tweak. `maps` is excluded too: a catalog divergence is the *recoverable* case
  this spec already handles by name.
- **Within those lanes the digest is a denylist, not an allowlist.** Hash every field of
  every descriptor it reaches, and name the specific fields skipped — not the reverse. An
  allowlist ("hash the fields a client simulates against") is the exact mechanism that
  produced the static-collision fail-open this spec exists to fix: a field added later by
  someone who never reads the recipe defaults to *unhashed*, and no test catches a field you
  forgot. A denylist makes the same omission fail loud — an unskipped presentation field
  demotes peers on a cosmetic edit, which is visible in a minute — instead of silent, which
  is prediction fighting nobody traces back. The rule in
  `research/coop-content-compatibility.md` §6 states the intent; the denylist is what
  enforces it, and enforcement is Task 6's exhaustive destructuring.
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
- **Parity compares all three values and reports which diverged.** Identity mismatch names
  both maps; level-digest mismatch means same map, different content; mod-digest mismatch
  means the peers' simulated surfaces disagree independently of the map.
- **A parity mismatch holds the slot at admitted; it never closes it.** The test for which
  lane a value belongs in is whether a mismatch on it can ever become a match later.
  Admission holds only values for which it cannot: the protocol constants are compiled in,
  and the mod id is frozen at first commit by the rule above. Every parity value is
  *designed* to become true later — a level digest at the next install, a mod digest at the
  next staged commit — so closing on one is a category error, and it would reintroduce for
  the client-initiated case exactly the disconnect this spec removes for the host-initiated
  one. It would also race the spec's own criteria: a client's parity message for level A can
  still be in flight when the host installs level B, so a host that closed on mismatch would
  tear down a client it demoted one frame earlier. Every content cause is therefore a
  *diagnostic* delivered to a still-connected client, and the deferred-disconnect mechanism
  serves admission rejects only.
- **The held slot is bounded by the transport, not by the gate.** A peer that never reaches
  parity holds an `Admitted` slot indefinitely, which is a connection resource with no path
  to participation — the one real cost of preferring hold to close. Accepted, because the
  case it covers is a peer on the *right* mod whose content diverged, which is the
  recoverable case by construction, and because renet's own timeout already reclaims a slot
  whose peer stops talking. A genuinely wrong mod still closes, on the id, at admission.
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
- **Divergence reasons become a typed enum over two closing causes and three holding
  ones** — protocol and mod id close; mod digest, level identity, and level digest hold —
  each carrying expected and received. The type is one enum rather than two so a single
  `Display` serves every diagnostic, but the closing and holding sets are distinguishable at
  the type level rather than by convention, so a later cause cannot be added to the wrong
  lane by omission. `RejectReason` and `HandshakeOutcome` lose `Copy` and gain `Clone`; call
  sites update in the same pass.
- **Both protocol constants bump.** New message vocabulary bumps the app protocol id; the
  changed layout bumps the wire version. Independent bumps, both apply.
- **Single-player is untouched.** No endpoint is constructed, so no gate runs, no relevel
  is sent, and no path branches on player count.

## Acceptance criteria

- [ ] A client connecting to a host with no level installed is admitted, holds the
      connection open, and receives no entity records.
- [ ] It begins receiving entity records once — and only once — its mod digest matches the
      host's **and** host and client have installed levels whose identity and level digest
      both match.
- [ ] A client whose declared mod **id** differs is refused, is told the id diverged, and
      receives no entity records.
- [ ] A client whose mod **compatibility digest** differs but whose id matches **keeps its
      connection**, holds at admitted, receives no entity records, and is told the mod
      digest — not the id and not the level — is what diverged.
- [ ] A client whose mod **version** differs but whose id and digest match **participates
      normally**; the version difference appears in a host-side log and nowhere else.
- [ ] A client whose protocol constants diverge is refused with a protocol cause, not a
      mod or content cause.
- [ ] A client refused at admission — protocol or mod id — observes the typed reason before
      its connection closes.
- [ ] Two maps that differ only in ways the shipped fingerprint ignored — two maps with no
      movers at all, and two maps whose only difference is static world collision geometry —
      produce different parity values, and a client on one does not participate on the other.
- [ ] Two mods differing only in `entities` or `store_declarations` content produce
      different mod digests; two differing only in `theme`, `fonts`, `ui_trees`, `render`,
      `frontend`, or `maps` produce the **same** digest and interoperate.
- [ ] Declaring the same descriptors and store slots in a different source order produces
      the same mod digest.
- [ ] Adding a field to a descriptor the mod digest reaches, without touching the digest
      recipe, **fails to compile** — the recipe destructures exhaustively, so a new field
      cannot default to unhashed.
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
- [ ] A staged hot reload that changes the mod **id or version** warns and leaves the
      installed value unchanged; no slot changes state.
- [ ] A staged hot reload that changes `entities` or `store_declarations` **recomputes and
      reinstalls** the mod digest; participating slots whose declared digest no longer
      matches drop to admitted with a mod-digest diagnostic, and none of them is closed.
- [ ] A staged hot reload that changes only presentation lanes moves no digest and demotes
      nobody.
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
staged manifest whose id or version differs from the installed one logs a warning and leaves
the installed value alone. This applies to **identity only** — the mod compatibility digest
Task 7 installs follows the opposite rule and is re-hashed on every staged commit, because
its sources are the two lanes a staged reload re-commits and it sits in the recoverable
parity lane rather than the terminal admission one. Comment both rules at the commit site
together, so the asymmetry reads as deliberate: identity is frozen because admission has no
recovery path, the digest is refreshed because parity does. The identity freeze diverges from
the atomic-replace discipline most manifest lanes follow — note at the same site that theme
and fonts are already non-re-committed, so it is a second instance and not a unique
exception. Update every shipped manifest under `content/` to declare both fields.

### Task 3: Two-stage gate and slot demotion

Replace the single app-gate message with two, in Task 1's module. **Admission** carries the
two protocol constants, the mod id, and the mod version; **content parity** carries the mod
compatibility digest, a level identity string, and the level content digest. Both ride
the reliable Control channel; both compare opaque values the crate never interprets. The
version field is carried for diagnostics and **must not be compared** — comment it at the
comparison site so a later reader does not "fix" the omission. Make the divergence reason a
typed enum whose closing causes (protocol, mod id) and holding causes (mod digest, level
identity, level digest) are distinguishable at the type level rather than by convention,
each carrying expected and received, with the mod-id cause quoting both peers' declared
versions and the level causes distinguishing an identity mismatch from a same-identity
digest mismatch. `RejectReason` and `HandshakeOutcome` lose `Copy` and gain `Clone`, and
their call sites in this crate update in the same pass. Add an `Admitted` state to
`SlotTable` between `Pending` and `Accepted`, rename `Accepted` to `Participating`, and add
the demotion transition `Participating → Admitted` emitting a new lifecycle event beside the
existing close event. Keep `Closed` terminal and every existing idempotence property.
`NetServer` holds the mod id and a **parity triple** — mod digest, level identity, level
digest — as separate `Option`s installed after construction: admission evaluates once the id
is present, parity once the triple is complete, each queuing until then — extending the
shipped early-return rather than adding a second waiting mechanism. The triple's two halves
install on different schedules (the mod digest at mod init and at every staged commit, the
level pair at every level install), so store it as one value replaced whole and compare on
replacement; installing a **different** triple by any route **demotes** participating slots
whose declared values no longer match, instead of closing them. Gate `send_snapshot` and the
accepted-client accessor on participating only. On an **admission** reject — protocol or mod
id — enqueue the typed reason on Control and close the slot immediately, but defer the socket
disconnect to the next poll via a small pending-disconnect list on `NetServer`, so the
reliable message flushes first. A **parity** mismatch is not a reject: enqueue the cause as a
diagnostic and leave the slot at `Admitted`, unclosed — every parity value becomes true again
at the next matching install or commit, so there is nothing to tear down. On the client,
split `handshake_sent` into an admission flag sent once on connect and a parity flag re-armed
whenever the parity triple changes — level install **or** staged mod commit — replacing
today's self-disconnect. Bump `PROTOCOL_ID` and `WIRE_VERSION`. Unit-test the gate and the
slot machine without sockets, including a mod-digest change demoting a participating slot
with no level involved.

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
still match.

Within those lanes the recipe is a **denylist**, and the enforcement is structural rather
than documentary: destructure each descriptor exhaustively by name — `let Descriptor { a, b,
c, .. }` is forbidden, no `..` rest pattern — bind every field, hash all of them, and route
the deliberate exclusions through a single `// not hashed: presentation only` block naming
each one and why. Adding a field to a descriptor then fails to compile until someone decides
which side it belongs on. That compile error *is* the mechanism; an allowlist plus a comment
asking future authors to remember is the mechanism that produced the static-collision
fail-open this spec is fixing. Where a descriptor legitimately carries presentation data
(a mesh handle, a material name), skip it by name in that block, not by omission.

Place the recipe engine-side (a sibling of the level recipe), not in `scripting-core`: the
manifest lane stays unaware of netcode, exactly as the mover recipe keeps its byte layout out
of the net crate. Test order-insensitivity, content-sensitivity per lane, and that a
presentation-only manifest edit leaves the digest unmoved.

### Task 7: Engine lifecycle wiring

Wire the engine's halves to the new gate. **Mod identity:** after mod init commits, install
the id and version on the net endpoint through a setter mirroring the fingerprint's, reached
through the same `session.net_endpoint` borrow `install_level_payload` uses; single-player
has no endpoint and skips. Install once, first-commit-wins, and on a staged reload that
changes either value log the warning Task 2 specifies. **Mod digest:** compute Task 6's
recipe over the committed manifest and install it into the parity triple — at mod init
**and again after every staged commit**, since a staged reload re-commits both lanes it
hashes. Reinstalling a changed digest demotes non-matching participating slots through
Task 3's existing replacement path; there is no separate demotion trigger to write here.
**Level identity and digest:** derive
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
| No entity state reaches a slot below participating | Task 3 (send gating) | Every new send path must gate on participation, not admission or connection | AC 1, 3, 4, 13 |
| **Admission carries only values that cannot change for a live connection** | Task 3 (lane assignment), Task 2 (identity frozen at first commit) | The lane is chosen by convenience, not by mutability — a future compared value put in admission "because it is known early" becomes an unrecoverable close the moment it can be reinstalled. The mod digest was exactly that mistake, caught on review | AC 4, 19 |
| Content parity is proven for the *current* content, not the joining one | Task 3 (demotion on triple replacement), Task 7 (per-install level pair, per-commit mod digest) | A demotion that failed to clear state would leave stale ids addressable; a parity value that stopped being reinstalled would gate on history | AC 13, 14, 19 |
| A demotion clears exactly what a close clears | Task 3 (event), Task 7 (routed into the existing cleanup) | Shared with the close path — a second cleanup implementation would drift. Both demotion triggers, level and mod digest, run the one path | AC 13, 19 |
| Level identity discriminates any two distinct levels | Task 7 (catalog id, path fallback) | The per-level gate is a no-op if two levels can collide | AC 8, 16 |
| Every input a client simulates against is covered by a digest | Task 6 (both recipes, denylist form) | A new client-local simulation input added later is silently ungated unless its recipe is widened too — static collision was exactly that omission. Enforced by exhaustive destructuring, not by the rule in `coop-content-compatibility.md` §6, because a rule does not fail a build | AC 8, 9, 11 |
| The mod digest describes the content the host is running now | Task 7 (re-hash on every staged commit) | Freezing it — the first draft's rule — gates live connections on a value the reload already replaced, silently, in the builds where co-op is developed | AC 19, 20 |
| Mod version is carried and never compared | Task 2 (SDK docs), Task 3 (commented at the comparison site) | It rides the same message as a gating value; a later reader "completing" the comparison silently reinstates exact-version equality and its false refusals | AC 5 |
| Presentation-only manifest lanes never affect compatibility | Task 6 (digest domain, exclusions named in one block) | Widening the mod digest to theme, fonts, UI trees, render, or frontend breaks co-op on every cosmetic edit | AC 9, 20 |
| A connection's id survives a level change | Task 3 (demote, never close), Task 5 (window stays polled) | Later specs key player identity off a connection that must not be re-minted | AC 14, 15 |
| Admission and parity queue independently until their source installs | Task 3 (separate `Option`s, separate early returns) | Coupling them re-creates the ordering inversion this spec removes | AC 1, 2 |
| A peer refused at admission learns the cause before teardown | Task 3 (deferred disconnect) | A future reject path that disconnects inline drops the message | AC 3, 6, 7 |
| No content divergence ever closes a connection | Task 3 (hold at admitted, closing and holding causes separated at the type level) | Any later content check that rejects instead of holding re-creates the disconnect this spec removes, and races an in-flight parity message against a just-installed level | AC 4, 12, 13, 19 |
| A relevel never restarts the load it names | Task 4 (active/in-flight suppression) | Late-join and transition both send; a third sender must suppress too | AC 14 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod id | `ModManifestResult` field | admission message field | `id: string` | `id: string` |
| mod version | `ModManifestResult` field | admission message field, carried not compared | `version: string` | `version: string` |
| mod compatibility digest | `[u8; 32]`, engine-derived from the committed manifest, re-derived per staged commit | **parity** message field | n/a (derived) | n/a (derived) |
| level identity | engine-derived `String` | parity + relevel message field | catalog `id` (existing) | same |
| level content digest | `[u8; 32]`, widened domain, epoch 2 | parity message field | n/a | n/a |
| admission message | net-crate handshake type | Control, client→server | n/a | n/a |
| parity message | net-crate handshake type | Control, client→server | n/a | n/a |
| relevel message | net-crate handshake type | Control, server→client | n/a | n/a |
| divergence reason | typed enum: 2 closing causes (protocol, mod id), 3 holding (mod digest, level identity, level digest) | Control, server→client | n/a | n/a |
| slot state | `SlotState::{Pending, Admitted, Participating, Closed}` | not replicated | n/a | n/a |

## Script syntax examples

```ts
// Proposed design — two new required fields; everything else is unchanged.
export default defineMod({
  // Stable machine identity. Charset [A-Za-z0-9_.:-]. Peers must declare the same id, and
  // this is the only *declared* field that can refuse a join. Whether two peers with the
  // same id can play is then decided by hashing what each simulates against.
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
- **Reject-reason delivery is the trimmable part, and it shrank again.** It covers
  admission rejects only — every content divergence now holds the connection open, so its
  diagnostic rides an ordinary reliable message with no deferral — which leaves one list and
  one poll on the protocol/mod-id path. The rest of the spec works without it. If it fights
  renet's teardown, the fallback is a host-side log only, at the cost of a player who cannot
  tell a wrong mod from an unreachable host. Worth noting the trim got cheaper: with the mod
  digest moved to parity, the *common* mismatch among friends on slightly different builds
  no longer travels this path at all.
- **Whether a mod-digest demotion should suppress the relevel message.** Task 4 sends a
  relevel to admitted slots so a diverged client learns which map would let it participate.
  For a client demoted on the **mod** digest, the map is not the problem and reloading it
  changes nothing — the send is harmless but misleading. Probably wants the diagnostic to
  carry the cause clearly enough that the relevel is not read as a fix. Detail review.
- **Mid-load relevel.** Task 4 suppresses a relevel naming the in-flight level, but a
  relevel naming a *different* map while a load is in flight is left to the shipped request
  path's ordering. If that path cannot preempt an in-flight load cleanly, the honest v1 is
  to queue the newer request and apply it on completion.
- **`networking.md` update at promotion.** The fingerprint-binds-the-connection sentence is
  overturned, the slot lifecycle gains a state and a transition, the handshake section
  describes one app message where there will be three, and the crate boundary gains a
  server→client control message family.
