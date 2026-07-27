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
  with no recovery path. But a manifest lane that is not atomic-replaced already ships:
  **`fonts`** is absent from `StagedManifest` entirely and is therefore never re-committed.
  (It is the only one — a staged reload re-commits far more than `mod-map-catalog`'s note
  suggests, including `entities`, `maps`, `reactions`, `crossings`, `trigger_events`,
  `trigger_pools`, `events`, the `render` profile, `ui_trees`, `theme`, and `frontend`. An
  earlier draft of this spec cited *theme and fonts* as the non-re-committed pair; theme is
  re-committed, through `commit_mod_ui_theme`.) Mod identity joins an existing minority of
  one rather than becoming the first exception. Worth a comment at the commit site as a second
  instance, not as a warning about a unique one. The mod **digest** does *not* diverge: it
  re-hashes on every staged commit, because a staged reload re-commits the entity registrations
  it reads, and freezing it would leave a live connection gated on a value that no longer
  describes the content the host is running.
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
  constants and the mod id and **false of the digest**: a staged reload re-commits the entity
  registrations the digest reads, so the value it describes changes while the
  connection lives. The first shape made its premise true by declining to look — freezing
  the digest at first commit — which bought the premise at the cost of gating live
  connections on a stale value, in debug builds, which is exactly where co-op is developed
  and playtested. It reasoned about the problem as freeze-versus-rehash-and-close and never
  reached the third option its own demotion edge supplies: **rehash and demote**. Rejected
  in favor of that. The dev-loop objection that killed a whole-mod hash does not transfer:
  demotion is not disconnection, and the domain is two fields per named entity type rather
  than every byte, so an ordinary script edit does not move it at all.
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
- Everything else is net-crate-local behind two hand-bumped constants: the three messages,
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
  state to demote them to. The digest is the opposite case in every respect: a staged reload
  re-commits the entity registrations it reads, it lives in the parity lane, and parity *has*
  a recovery path. So a staged commit that moves a hashed field recomputes the digest,
  reinstalls it, and demotes any participating slot whose declared digest no longer matches —
  with a diagnostic naming the divergence. That is loud and correct where freezing was silent
  and stale. The dev-loop objection that rejected a whole-mod content hash does not reach
  this: that hash moved on every byte of every script and its consequence was a *closed*
  connection; this one moves only when a hashed field changes — a much smaller set than a
  lane, per Task 6's table — and its consequence is a hold that resolves the moment the peers
  agree again.
  Identity's freeze stays a documented divergence (Divergence 2); the digest is no longer
  part of it.
- **The mod compatibility digest covers two named types, not two manifest lanes.** Per
  registered entity type: the `canonical_name` a client materializes by, and the
  `PlayerMovementDescriptor` it predicts with. Nothing else. The full per-field disposition
  is in Task 6, and it has **three** categories rather than two — hashed, skipped because
  presentation, skipped because host-authoritative. That third category is the correction: an
  earlier draft said "hash the `entities` lane exhaustively," which would have hashed `ai`,
  `health`, `weapon`, and `default_weapon`. Those are Tier 2 in
  `research/coop-content-compatibility.md` — host-owned, safe to change freely — so the rule
  as written would have demoted every peer on an enemy retune. That is the exact false
  refusal content-derived compatibility exists to prevent, arrived at from the other
  direction.
- **State-slot parity is not this digest's job.** `ReplicatedSlotSchema` already hashes every
  replicated slot's name, type, range, and scope under its own stream version, and both peers
  already compare it. A second mechanism over the same data is duplication, and it is what
  pulled private fields and a 30-variant IR enum into a domain meant to be destructured. The
  shipped fingerprint owns this; Task 7 fixes its one real defect, which is that it is
  process-cached with no reset and goes stale across a staged reload.
- **Within each named type the digest is a denylist, not an allowlist.** Bind every field by
  exhaustive destructuring and name the specific skips — not the reverse. An allowlist ("hash
  the fields a client simulates against") is the exact mechanism that produced the
  static-collision fail-open this spec exists to fix: a field added later by someone who never
  reads the recipe defaults to *unhashed*, and no test catches a field you forgot. A denylist
  makes the same omission fail loud instead of silent. Scope the claim honestly, though: it is
  a guarantee about fields *inside* `EntityTypeDescriptor` and `PlayerMovementDescriptor`, not
  about manifest lanes. A new **lane** still escapes it, which is why the uncovered set below
  is named rather than assumed empty.
- **Five simulated manifest lanes are knowingly uncovered, and named.** `reactions`,
  `crossings`, `events`, `trigger_events`, and `trigger_pools` are committed into the data
  registry and the impact-policy runtime; none is presentation, and no digest covers them.
  Tier 3 item 5 in `research/coop-content-compatibility.md` — world gravity set from a
  reaction, which feeds local prediction on both peers — lives in exactly this set. The
  consequence, stated rather than discovered later: two mods whose reaction lanes differ can
  pass both gates and diverge on locally-simulated gravity. Hashing the reaction lanes is its
  own problem, with its own IR-encoding question, and it is not smuggled into this spec. It is
  named here because a spec whose central argument is that silent fail-opens are the danger
  does not get to leave its own uncovered set implicit.
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
- **A demotion runs the same cleanup as a close.** The general rule is that a demoted slot's
  ids are no longer *proven*, whichever trigger fired — so its pawn, replication, ownership,
  command, state-slot, and combat state clear exactly as a close clears them. The connection is
  what survives, not the state. Note the rationale had to generalize: an earlier draft argued
  it from "level unload invalidates every id the per-slot tables hold," which is true of the
  level trigger and false of the mod-digest one, where the level stays loaded and the cleanup
  runs anyway. The client-side counterpart of that asymmetry is an open question below.
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
      produce different **level content digests** while carrying the **same level identity**,
      and a client on one does not participate on the other. Identity must not be what
      discriminates, or the criterion passes with the epoch untouched.
- [ ] A catalogued level and the same `.prl` loaded by raw path produce **different level
      identities**, and the mismatch is reported by name rather than as a hash diff.
- [ ] Two mods differing only in a **hashed** field — an entity's `canonical_name`, or any
      `PlayerMovementDescriptor` field other than `view_feel` — produce different mod digests.
- [ ] Two mods differing only in `ai`, `health`, `weapon`, `default_weapon`, or `behavior`
      produce the **same** mod digest and interoperate. So do two differing only in `light`,
      `emitter`, `mesh`, `view_feel`, or any presentation lane.
- [ ] Declaring the same entity types in a different source order produces the same mod
      digest, and a descriptor with no `canonical_name` does not affect it at all.
- [ ] The same content hashes to the same mod digest **in two separate processes**, including
      a descriptor carrying a populated `zone_multipliers` map.
- [ ] Adding a field to `EntityTypeDescriptor` or `PlayerMovementDescriptor` without touching
      the digest recipe **fails to compile** — the recipe destructures exhaustively, so a new
      field cannot default to unhashed. Verified by the pattern being present at review, not
      by a runtime test.
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
- [ ] A staged hot reload that changes a hashed entity field **recomputes and reinstalls** the
      mod digest; participating slots whose declared digest no longer matches drop to admitted
      with a mod-digest diagnostic, none of them is closed, and each demoted slot's pawn,
      replication, ownership, command, state-slot, and combat state are cleared exactly as a
      level-change demotion clears them.
- [ ] A staged hot reload that changes only presentation or host-authoritative fields moves no
      digest and demotes nobody.
- [ ] After a host level change, no host-side level-scoped table retains an entry keyed by an
      id the unload invalidated, and the replicated-slot schema is rebuilt rather than served
      from the process cache.
- [ ] A redundant relevel naming the level already active or already in flight does not
      restart the load.
- [ ] A client joining a host that **already has a level installed** receives a relevel for
      the current map on admission, without waiting for the next transition.
- [ ] A relevel naming a catalog id absent from the client's catalog logs a diagnostic naming
      the id and leaves the client admitted rather than closing it.
- [ ] Single-player boot constructs no endpoint and reaches Running unchanged.
- [ ] A peer built before this change is refused at the transport gate, before any app
      message is decoded.

## Tasks

### Task 1: Extract the handshake gate

`crates/net/src/transport.rs` is ~740 non-test lines and this spec adds a second gate
stage plus a message family to it. Split first, behavior-preserving: move the pure gate
surface into a new `crates/net/src/handshake.rs` beside `slots.rs` — the validation
function, the reject reason with its `Display` and `Error` impls, the protocol-constant
accessors (`PROTOCOL_ID`, `WIRE_VERSION`, `transport_protocol_id`, `protocol_version`), and
the malformed/hex helpers. What moves is the **comparison** surface, not the wire surface:
`ProtocolVersion` is a bitcode type and stays in `wire.rs` with the others, and `transport.rs`
contains no wire-serialized type at all today. `transport.rs` keeps the renet plumbing:
socket, channels, `NetServer`/`NetClient`, the poll loop, and send gating. Re-export **from
`transport.rs`** (`pub use crate::handshake::*;`), not from `lib.rs`: downstream imports are
module-qualified — `use postretro_net::transport::{NetClient, NetServer}` in
`crates/postretro/src/netcode/mod.rs`, `transport::HandshakeOutcome` in `main.rs` — and
`lib.rs` carries no re-exports today, so a `pub use` there mints a new path instead of
preserving the existing one. Relocate the existing gate unit tests with their
subject. No behavior change.

### Task 2: Mod identity in the manifest

Add a required stable id and a required version to the mod manifest. Declare both on the
`ModManifest` type at its **registration site** — the `registry.register_type("ModManifest")`
call in `crates/postretro/src/scripting/primitives/manifest.rs::register_sdk_type`, which is
where the doc strings live. `sdk/types/postretro.d.ts` and the Luau typedef are **generated
artifacts**; both carry a "do not edit by hand" banner and
`committed_sdk_types_match_current_registry` fails CI on a hand edit. Regenerate them with
`gen-script-types`, along with the `expected.d.ts` / `expected.d.luau` fixtures under
`crates/postretro/src/scripting/typedef/tests/fixtures/`. Note the parity guard in the same
file — `mod_manifest_registered_type_matches_mod_manifest_result` pins the registered field
list to `ModManifestResult`'s, so it fails the moment the Rust fields land without the
registration, which makes the two edits a single change rather than two.
Document precisely what each field is for: peers must declare the **same id** to connect, the
**version is displayed and never compared**, and neither is a security mechanism.
Parse both at **four** sites — the JS and Luau manifest readers
in `crates/scripting-core/src/runtime/mod_init_exec.rs` and their staged-reload
counterparts in `crates/scripting-core/src/staged_manifest.rs` — each following the
existing required-`name` shape, so a missing field is an `InvalidArgument` naming the
field and the source path. Validate the id against `[A-Za-z0-9_.:-]` at parse, **anchored over
the whole string**, minimum length 1, compared case-sensitively at admission; the version is
any non-empty string. Carry both on `ModManifestResult` beside `name` **and on
`StagedManifest`** (`crates/scripting-core/src/staged_manifest/transfer.rs`) — a fifth type,
and the one the staged path actually hands to the main thread. Without it the main-thread
commit cannot see the staged values and the first-wins warning has nothing to compare. Commit
them with the rest of the manifest at mod init, but make the commit **first-wins across
reloads**: a staged manifest whose id or version differs from the installed one logs a warning
and leaves the installed value alone. This applies to **identity only** — the mod
compatibility digest Task 7 installs follows the opposite rule and is re-hashed on every
staged commit, because a staged reload re-commits the entity registrations it reads and it
sits in the recoverable parity lane rather than the terminal admission one. Comment both rules at the commit site
together, so the asymmetry reads as deliberate: identity is frozen because admission has no
recovery path, the digest is refreshed because parity does. The identity freeze diverges from
the atomic-replace discipline most manifest lanes follow — note at the same site that theme
and fonts are already non-re-committed, so it is a second instance and not a unique
exception. Update every shipped manifest under `content/` to declare both fields.

### Task 3: Two-stage gate and slot demotion

Replace the single app-gate message with two. **Admission** carries the
two protocol constants, the mod id, and the mod version; **content parity** carries the mod
compatibility digest, a level identity string, and the level content digest. Both compare
opaque values the crate never interprets.

**Control needs a tagged envelope first.** `NetServer::process_control_messages` decodes
Control as a bare `let received: ProtocolVersion = wire::decode(&bytes)`. bitcode is not
self-describing, so a second client→server Control message would decode as the first without
erroring. The crate already solved this on `Channel::Input`, and the comment above
`ClientMessage` in `wire.rs` states the rule: the receiver decodes one enum and matches on
the variant rather than guessing an untagged payload's type. Add the Control equivalents —
a client→server envelope carrying admission and parity, a server→client envelope carrying the
divergence diagnostic and Task 4's relevel — as **appended** variants in the same style, and
replace the untagged decode with a match. The messages are bitcode wire types, so they are
defined in `wire.rs` beside `ClientMessage`/`ServerMessage` and *compared* in Task 1's
`handshake.rs`; `ProtocolVersion` stays in `wire.rs` too. Note that the admission message
carries `String` fields, so it cannot be `Copy` and its constructor cannot stay a `const fn`
the way `protocol_version` is today.

**Slots must retain what they declared.** `SlotTable` is `HashMap<ClientId, SlotState>` —
state only, no payload — and `process_control_messages` drops `received` after comparing. The
shipped fingerprint setter sidesteps this by closing *every* client unconditionally rather
than comparing per-slot, which is exactly what this spec replaces. Per-slot demotion needs
each slot's last-declared parity triple retained, so widen the slot record to carry it (or add
a parallel map on `NetServer` beside `pending_lifecycle`) and say which. `SlotState` loses
`Copy` as a consequence, the same way `RejectReason` does. The
version field is carried for diagnostics and **must not be compared** — comment it at the
comparison site so a later reader does not "fix" the omission.

**The divergence reason is a two-level enum**, not a flat one:
`DivergenceReason::{Closing(ClosingCause), Holding(HoldingCause)}`, with `Display` on the
outer so one impl serves every diagnostic. A flat five-variant enum would be distinguishable
only by matching on it — which is the convention this is meant to replace, so the nesting is
the point rather than decoration. `ClosingCause` covers protocol and mod id; `HoldingCause`
covers mod digest, level identity, and level digest. Each carries expected and received, with
the mod-id cause quoting both peers' declared versions and the level causes distinguishing an
identity mismatch from a same-identity digest mismatch. `RejectReason` today is a **struct**
with an `impl std::error::Error`; say whether it becomes the outer wrapper and keeps that impl
or is replaced. `RejectReason` and `HandshakeOutcome` **lose `Copy`; they already derive
`Clone`.** The real work is two reuse-after-move sites in `process_control_messages` (the
malformed-decode and validate-failure branches each move `reason` into `reject` and then reuse
it in the outcome) plus three `Some(*reason)` derefs in the loopback tests.

**Slot machine.** Add an `Admitted` **variant to `SlotState`** between `Pending` and
`Accepted`, rename `Accepted` to `Participating`, and add the demotion transition
`Participating → Admitted` emitting a new lifecycle event beside the existing close event.
Keep `Closed { cause }` terminal and every existing idempotence property — with two
clarifications the shipped shape forces:

- **`on_close` continues to emit only from `Participating`.** `Admitted → Closed` is silent,
  because either the slot never participated (no pawn was created) or it was demoted and the
  demotion edge already ran the cleanup. Emitting there would double-run it.
- **Promotion into `Participating` must re-emit on every entry.** `SlotTable::on_accept`
  returns `None` when the slot is already accepted — once-only per `ClientId` — which is
  correct today and wrong the moment a demoted slot re-participates, since Task 7 hangs the
  pawn spawn on that transition. Say so explicitly; it is the single easiest thing to miss.

**Verdict lanes.** `ServerPoll` today carries `handshakes` and `lifecycle`, and
`HandshakeOutcome` is exactly `Accepted`/`Rejected`. A parity *pass* and a parity *hold* map
to neither. Name which vector carries each of the three new verdicts — admission pass,
participation transition, parity hold — adding a third field or new outcome variants rather
than leaving an implementer to invent the vocabulary Task 7 then consumes. Note that the
accept signal deliberately rides `handshakes` and not `lifecycle` today, and that
`SlotEvent::Accepted` is matched-but-ignored in `crates/postretro/src/netcode/mod.rs` under a
comment saying the arm is kept exhaustive so a new variant is a compile error — so the new
demotion variant breaks that file by design.

**Installed values.** `NetServer` holds the mod id as one `Option`, and the parity sources as
two: `Option<mod digest>` and `Option<(level identity, level digest)>`, because they install
on different schedules — the digest at mod init and at every staged commit, the level pair at
every level install. Combine them into a comparable triple only when both are present; a
partial install is not a parity value. Admission evaluates once the id is present, parity once
the triple completes, each queuing until then — extending the shipped early-return rather than
adding a second waiting mechanism. Installing a **different** triple by either route
**demotes** participating slots whose retained declaration no longer matches, instead of
closing them.

**Send gating and rejects.** Gate `send_snapshot` and the accepted-client accessor on
participating only. `NetServer` has no server→client Control send path today — `send_snapshot`
is Snapshot and `send_input` is Input — so name the new one. On an **admission** reject —
protocol or mod id — enqueue the typed reason on Control and close the slot immediately, but
defer the socket disconnect to the next poll via a small pending-disconnect list on
`NetServer`. Be honest about what that buys: `RELIABLE_RESEND` is 300 ms, so at a 16 ms frame
the reason gets exactly one datagram and a single drop loses it. This is best-effort delivery,
not guaranteed — which is consistent with it being the spec's named trimmable part. A
**parity** mismatch is not a reject: enqueue the cause as a diagnostic and leave the slot at
`Admitted`, unclosed — every parity value becomes true again at the next matching install or
commit, so there is nothing to tear down.

**Both poll entry points.** `NetServer::update` and `NetServer::poll_handshakes` each
independently drain `pending_lifecycle`, run the `ServerEvent` loop, and call
`process_control_messages`; only `update` calls `transport.send_packets`, and
`poll_handshakes` is the in-memory relay path `harness.rs` and the loopback tests use. "The
next poll" must mean both, or relay-driven tests wedge on an undrained pending-disconnect list.

**Client side.** Split `handshake_sent` into an admission flag sent once on connect and a
parity flag re-armed whenever either parity source changes — level install **or** staged mod
commit — replacing today's self-disconnect. The flag is duplicated across `NetClient::update`
and `NetClient::update_connections`, so the split lands twice, and its accessor is the
loop-termination condition in `harness.rs`'s `pump_client_to_server`.

Bump `PROTOCOL_ID` and `WIRE_VERSION`, and re-stage the existing both-gates regression test in
`transport.rs` — which hard-asserts their exact values with bump-specific failure messages —
to the new pair, with the previous pair as the refused peer. That test is what satisfies the
last acceptance criterion. Unit-test the gate and the slot machine without sockets, including
a mod-digest change demoting a participating slot with no level involved, and a demoted slot
re-participating and re-emitting its promotion.

### Task 4: Relevel message and client-follow

Give the host a way to name the next map and the client a way to follow it. Add a
server→client relevel variant to Task 3's Control envelope carrying one map catalog id,
sent to every admitted and participating slot when the host installs a catalogued level and
on admission for a client joining a host that already has one — so a late joiner is told
the current map without waiting for the next transition. Sending to admitted slots is also
what recovers a slot held on a parity mismatch: it is the message that tells a diverged
client which map would let it participate.

**The relevel id is a separate installed value, not the parity triple's identity field.** A
host whose active level has no catalog id sends nothing — but the crate cannot evaluate that
condition, because level identity is one opaque string it never interprets and the path
fallback is indistinguishable from a catalog id inside it. So `NetServer` holds an additional
`Option<String>` relevel catalog id, installed by the engine only when the active level is
catalogued and cleared otherwise. Both the send-on-install and send-on-admission paths read
that, not the parity value. Send from both poll entry points, as Task 3 requires of every new
send.

On the client, surface received relevel messages out of the endpoint poll as a typed value the
engine drains — say whether that changes `NetClient::update`'s return type to a `ClientPoll`
(there is no such type today; `update` returns `Result<(), _>` and `update_connections` returns
`()`) or types the existing `NetClient::drain_control`, which returns `Vec<Vec<u8>>` and has no
callers. The drain must run from the world-less frames Task 5 opens, not only from Running,
since a relevel can arrive while an earlier load is in flight. The engine enqueues
`LevelRequest::Load(LevelSource::Catalog(id))` through the shipped request path, which
already unloads any active level first. Ignore a relevel naming the level already active or
already in flight, so a redundant send does not restart a load. An id absent from the local
catalog logs a diagnostic naming the id and leaves the client admitted — it is the
recoverable case, since the mods matched but the catalogs diverged.

**Mid-load relevel is settled, not open.** `App::enqueue_level_request` already queues a newer
`Load` over a queued one and applies it when the in-flight load completes, so the v1 rule is
the shipped behavior — with one exception to name: it early-returns with a warning during the
**boot** load specifically. Say that a relevel arriving during boot load is dropped, and that
this is acceptable because a client in boot load has not yet been admitted.

### Task 5: Poll the transport across every world-less frame

`net_poll_and_apply` is reached only from the Running gameplay block, via
`frame_order::run_snapshot_apply_stage`. That is a bigger hole than "the load window," and
naming it as one was this spec's own error: the redraw arm **returns early on
`BootState::Frontend`** before the snapshot-apply stage, and neither `run_frontend_ui_logic`
nor `render_frontend_frame` touches `session.net_endpoint`. The endpoint is constructed in
`Session::build` at boot, so a peer launched with no map argument sits in Frontend and never
advances renet at all — it cannot complete the transport connect, let alone be admitted.
**The spec's headline case is unreachable without this task covering Frontend**, not just
Loading. Frontend is also where `finish_level_failure` and `unload_level` land.

So: advance the net endpoint from **every world-less frame** — the loading frame in
`crates/postretro/src/startup/lifecycle.rs` and the Frontend path in
`crates/postretro/src/main.rs`. Transport advance, handshake processing, and keepalive only.
Do **not** apply snapshots or run any game logic there: there is no world outside Running,
and the snapshot-apply ordering contract (apply before state-crossing detection, within the
Game-logic stage) has no meaning there. Splash is out — it is two frames and precedes
`Session::build`; say so rather than leaving the reader to wonder.

Relevel delivery is **not** this task — it belongs to Task 4, which owns the message, and
Task 4 must state that its client-side drain runs from these frames too, since a relevel can
arrive while an earlier load is in flight.

Cover the window with a test that holds a load open past the timeout and asserts the
connection survives, and a second that admits a client while the host sits in Frontend with
no level installed — that one is AC 1's real regression test.

### Task 6: The two compatibility digest recipes

Pure functions plus their tests; Task 7 installs them. Both live engine-side beside the
existing fingerprint recipe, so the net crate keeps treating every compared value as opaque.

**Level content digest.** Widen `kinematic_static_fingerprint` (`runtime_movers.rs`) to hash
the level's static world collision alongside the mover data it already covers: the
`LevelWorld` vertex positions and index buffer that `CollisionWorld::populate_from_level`
builds the client's trimesh from. Hash them with the existing `hash_len`/`hash_f32` helpers
and in the same shape the per-mover vertex/index loop already uses, so a zero-triangle level
is distinguishable from another zero-triangle level by nothing *in this field* — which is
correct, because level identity carries that. Bump `FINGERPRINT_EPOCH` to 2; note it is a
function-local `const` inside the recipe, not a shared constant. Take `&LevelWorld` (or the
two slices) rather than `&KinematicGeometry`; the caller in `install_level_payload` already
holds `world`. Rename to `level_content_digest` — it is no longer kinematic-only — and carry
the rename through its engine-side call chain, which reaches outside this file:
`NetEndpoint::set_kinematic_static_fingerprint` (`crates/postretro/src/netcode/mod.rs`) and
its caller in `install_level_payload`. Test that two levels differing **only** in static
collision produce different digests, that identical geometry with differing entity placements
produces the same one, and that two mover-less levels with different brushwork no longer
collide.

**Mod compatibility digest.** Two named types, not two manifest lanes.

*Source.* Read from `ScriptCtx::data_registry` after mod init commits, **not** from
`ModManifestResult`: `ScriptingCore::drain_manifest_registrations` does
`std::mem::take(&mut manifest.entities)`, so the manifest's own `entities` is an empty `Vec`
by the time anything could hash it. The registry is the same read `App::net_poll_and_apply`
already uses to build `net_descriptors`, so this adds no new access path.

*Domain.* Per entity type in the registry, hash exactly two things:

| Field of `EntityTypeDescriptor` | Disposition |
|---|---|
| `canonical_name` | **hashed** — the wire's entity class |
| `movement` (`PlayerMovementDescriptor`) | **hashed**, minus `view_feel` |
| `default_weapon`, `weapon`, `health`, `ai`, `behavior` | skipped — **host-authoritative** |
| `light`, `emitter`, `mesh` | skipped — **presentation** |

Three categories, not two. A field can be excluded because a client never sees it *or*
because the host owns it, and collapsing those into one "presentation" bucket is what an
earlier draft got wrong: `ai`, `health`, and `weapon` are Tier 2 in
`research/coop-content-compatibility.md` — safe to change freely — so hashing them would
demote every peer on an enemy retune, which is the false refusal this whole policy exists to
prevent. Inside `PlayerMovementDescriptor` the nine remaining fields (`capsule`, `ground`,
`air`, `fall`, `stuck_stop_enabled`, `stuck_stop_threshold`, `dash`, `forgiveness`, `crouch`)
are all prediction inputs; `view_feel` is documented render-only and is the one skip.

Descriptors whose `canonical_name` is `None` are **excluded entirely**. A client materializes
remote entities by `entity_class` matched against `canonical_name`, so an unnamed descriptor
cannot cross the wire — which also supplies the total order the sort needs, since the
remaining set is keyed by a present name.

*State slots are not in this digest.* `ReplicatedSlotSchema`
(`crates/postretro/src/netcode/state_slots.rs`) already hashes every replicated slot's dotted
name, type, range, and scope under its own `FINGERPRINT_STREAM_VERSION`, and both peers
already compare it. Do not build a second mechanism over the same data — that duplication is
what dragged `SlotRecord`'s private `write_generation`, `StoreDeclarationSet`'s private
`BTreeMap`, and `SlotSchema::accumulate`'s ~30-variant `IrNode` into a domain that cannot be
destructured. State-slot parity is that fingerprint's job; this task's only slot-related work
is the cache defect Task 7 fixes.

*Determinism rules, all three load-bearing.* Every map-valued field the recipe reaches is
hashed in **key-sorted order** — `HealthDescriptor::zone_multipliers` is a
`std::collections::HashMap` with `RandomState`, whose iteration order differs *per process*,
so without this two peers on byte-identical content compute different digests. Enums are
hashed through a `match` with **no wildcard arm** — struct destructuring gives no
exhaustiveness over enums, so that is a separate rule and not a consequence of the first.
`Option` writes a presence byte, and every string and sequence is length-prefixed via
`hash_str`/`hash_len`, so two distinct descriptor sets cannot concatenate to the same stream.

*Enforcement.* Within each named type, destructure exhaustively — `let Descriptor { a, b, ..
}` is forbidden, no rest pattern — bind every field, and route skips through two labelled
blocks, `// not hashed: host-authoritative` and `// not hashed: presentation`. Adding a field
then fails to compile until someone classifies it. That compile error is the mechanism. Note
what it does and does not buy: it is a claim about fields **inside the types named above**,
not about manifest lanes. Applying it at lane granularity is exactly the overreach the
domain table above corrects.

*Epoch.* The mod digest carries its own epoch constant, bumped whenever the recipe changes —
the level digest has `FINGERPRINT_EPOCH` and this needs the equivalent.

Place the recipe engine-side (a sibling of the level recipe), not in `scripting-core`: the
manifest lane stays unaware of netcode, exactly as the mover recipe keeps its byte layout out
of the net crate. Test: order-insensitivity; that a `movement` edit moves the digest; that an
`ai`, `health`, or `weapon` edit does **not**; that a presentation edit does not; and that a
descriptor carrying a populated `zone_multipliers` map hashes identically across two
processes.

### Task 7: Engine lifecycle wiring

Wire the engine's halves to the new gate. **Mod identity:** after mod init commits, install
the id and version on the net endpoint through a setter mirroring the fingerprint's, reached
through the same `session.net_endpoint` borrow `install_level_payload` uses; single-player
has no endpoint and skips. Install once, first-commit-wins, and on a staged reload that
changes either value log the warning Task 2 specifies.

**Mod digest:** compute Task 6's recipe over `ScriptCtx::data_registry` and install it as the
parity lane's first source — at mod init **and again after every staged commit**, since a
staged reload re-commits the entity registrations the recipe reads. The staged seam is
`App::poll_staged_manifest_results`
(`crates/postretro/src/startup/staged_manifest_lifecycle.rs`), which the spec previously left
unnamed. Three things it forces: install only on `StagedManifestCommitOutcome::Committed`;
place the install inside a `self.session` borrow scope compatible with the one that function
already scopes around `commit_staged_manifest_result` so App methods can re-borrow; and define
the digest for the `NoStartScript` committed case, which clears lanes — the empty-registry
digest, not a skip, since skipping would leave a stale value installed. Reinstalling a changed
digest demotes non-matching participating slots through Task 3's replacement path; there is no
separate demotion trigger to write here.

**Level identity and digest:** derive identity in `install_level_payload` from
`App.active_level_source`, which `retain_active_level_tags_for_install` populates on the line
immediately before the digest is computed. The catalog id when present — but the path fallback
needs real work rather than a phrase: `resolve_level_source`'s `Path` arm stores
`map_path.to_string_lossy()`, the raw CLI argument exactly as typed, absolute or CWD-relative.
Two peers launched from different working directories would diverge on identity while running
the same file. Relativize against `App::content_root`, emit forward slashes, case-sensitive, no
`.`/`..` segments, and state the behavior for a path outside the content root (use the
normalized absolute path and accept that it only matches a peer launched identically). Install
it alongside Task 6's widened level digest, computed from the same `world` already in scope.
Also install the relevel catalog id Task 4 needs — set when the source is `Catalog`, cleared
otherwise. **Accept seam:** the pawn spawn currently keyed off the accept outcome in
`main.rs` moves to the participation transition; a pawn needs a level, which is what parity
now proves. **Demotion:** route the new demotion event into `host_handle_lifecycle` so it
runs the same per-slot cleanup a close runs — do not duplicate that cleanup. **Host unload
reset:** `reset_level_scoped_client_state` early-returns for the host role today; give it a
host arm that clears the level-scoped host tables (movement owners, slot pawns, replicable
set, weapon owners, open shots, command queues) whose entries the unload has invalidated.

**Replicated-slot schema cache — work, not a check.** An earlier draft said "confirm the host
state-replication schema is rebuilt per level rather than cached for the process." It is not:
`HostStateReplication::schema` is built lazily through `get_or_insert_with` and has no reset
path, and the client-side `schema`/`net_schema` cache identically. So give all of them a reset
on level unload **and** on a staged commit that changes store declarations. This matters
beyond tidiness now that state-slot parity is owned by `ReplicatedSlotSchema` rather than by
the mod digest: a staged reload that adds a namespace otherwise leaves both peers comparing a
fingerprint derived from schema neither is still running.

Keep the `main.rs` edit to redirecting the two triggers — splitting that file
is out of scope and explicitly deferred by `runtime-level-lifecycle`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the split every net-crate edit lands in.

**Phase 2 (concurrent):** Task 2, Task 3, Task 5, Task 6. Disjoint in their primary files —
scripting-core, the net crate, the world-less frames, the two hash recipes — with two crossings
named rather than assumed away:

- **Task 3 is not net-crate-local.** Renaming `Accepted` to `Participating` escapes the crate:
  `NetServer::accepted_clients` is called from `crates/postretro/src/main.rs` and
  `crates/postretro/src/netcode/mod.rs`, `NetServer::is_accepted` from
  `trigger_state_channel_harness_test.rs`, and the exhaustive `SlotEvent::Accepted` arm in
  `netcode/mod.rs` breaks on the new demotion variant by design. Task 3 therefore carries a
  mechanical rename pass over `crates/postretro`, or keeps deprecated aliases until Task 7 —
  pick one, because without either the workspace does not compile at the end of Phase 2.
- **Task 6 has one existing caller.** `kinematic_static_fingerprint` is called today from
  `install_level_payload`, and Task 6 both renames it and changes its parameter, so Task 6
  updates that call site in place. Task 7 later rewrites the surrounding lines. This is a
  two-line overlap, not a conflict, but it is not "no caller until Phase 4" as an earlier
  draft claimed.

**Phase 3 (sequential):** Task 4 — consumes Task 3's slot states and Control envelope.
**Phase 4 (sequential):** Task 7 — consumes the setters from Task 3, the committed identity
from Task 2, both recipes from Task 6, and the client-follow drain from Task 4.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No entity state reaches a slot below participating | Task 3 (send gating) | Every new send path must gate on participation, not admission or connection | AC 1, 3, 4, 16 |
| **Admission carries only values that cannot change for a live connection** | Task 3 (lane assignment), Task 2 (identity frozen at first commit) | The lane is chosen by convenience, not by mutability — a future compared value put in admission "because it is known early" becomes an unrecoverable close the moment it can be reinstalled. The mod digest was exactly that mistake, caught on review | AC 4, 22 |
| Content parity is proven for the *current* content, not the joining one | Task 3 (demotion on source replacement), Task 7 (per-install level pair, per-commit mod digest) | A demotion that failed to clear state would leave stale ids addressable; a parity value that stopped being reinstalled would gate on history | AC 16, 17, 22 |
| A demotion clears exactly what a close clears | Task 3 (event), Task 7 (routed into the existing cleanup) | Shared with the close path — a second cleanup implementation would drift. Both demotion triggers, level and mod digest, run the one path | AC 16, 22 |
| Level identity discriminates any two distinct levels | Task 7 (catalog id, normalized path fallback) | The per-level gate is a no-op if two levels can collide, and the addressing-mode case is the one that collides in practice | AC 9 |
| The level digest discriminates content the identity cannot | Task 6 (static collision folded in, epoch 2) | Two maps sharing an identity but differing in brushwork must not compare equal — this is the fail-open the spec exists to close, and it is only tested if identity is held constant | AC 8 |
| Every input a client simulates against is either covered by a digest **or named as uncovered** | Task 6 (both recipes), Decisions (the five reaction lanes) | A new client-local simulation input added later is silently ungated unless its recipe is widened too — static collision was exactly that omission. The weaker "or named" form is deliberate: the reaction lanes are uncovered, and a total-coverage claim would be false | AC 8, 10, 14 |
| The mod digest describes the content the host is running now | Task 7 (re-hash on every staged commit) | Freezing it — the first draft's rule — gates live connections on a value the reload already replaced, silently, in the builds where co-op is developed | AC 22, 23 |
| The mod digest is stable across processes | Task 6 (key-sorted map hashing) | `HashMap` iteration order varies per process; without the sort two peers on identical content never agree, and the failure looks like a content mismatch rather than a recipe bug | AC 13 |
| Host-authoritative fields never affect compatibility | Task 6 (three-category disposition table) | Hashing `ai`, `health`, or `weapon` demotes peers on an enemy retune — the false refusal the whole content-derived policy exists to prevent | AC 11 |
| Mod version is carried and never compared | Task 2 (SDK docs), Task 3 (commented at the comparison site) | It rides the same message as a gating value; a later reader "completing" the comparison silently reinstates exact-version equality and its false refusals | AC 5 |
| Presentation never affects compatibility | Task 6 (digest domain, exclusions named in labelled blocks) | Widening the mod digest to meshes, lights, emitters, or `view_feel` breaks co-op on every cosmetic edit | AC 11, 23 |
| A connection's id survives a level change | Task 3 (demote, never close), Task 5 (world-less frames stay polled) | Later specs key player identity off a connection that must not be re-minted | AC 17, 18 |
| Admission and parity queue independently until their source installs | Task 3 (separate `Option`s, separate early returns) | Coupling them re-creates the ordering inversion this spec removes | AC 1, 2 |
| A peer refused at admission learns the cause before teardown | Task 3 (deferred disconnect, best-effort) | A future reject path that disconnects inline drops the message entirely | AC 3, 6, 7 |
| No content divergence ever closes a connection | Task 3 (hold at admitted, closing and holding causes separated at the type level) | Any later content check that rejects instead of holding re-creates the disconnect this spec removes, and races an in-flight parity message against a just-installed level | AC 4, 15, 16, 22, 27 |
| A relevel never restarts the load it names | Task 4 (active/in-flight suppression) | Late-join and transition both send; a third sender must suppress too | AC 25, 26 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod id | `String` on `ModManifestResult` **and `StagedManifest`** | admission variant field | `id: string` | `id: string` |
| mod version | `String` on both, same as above | admission variant field, carried not compared | `version: string` | `version: string` |
| mod compatibility digest | `[u8; 32]`, engine-derived from `ScriptCtx::data_registry`, re-derived per staged commit | **parity** variant field | n/a (derived) | n/a (derived) |
| level identity (parity) | engine-derived `String` — catalog id, else normalized content-root-relative path | parity variant field | catalog `id` (existing) | same |
| relevel catalog id | `Option<String>` on `NetServer`, installed only for a catalogued level | relevel variant field | catalog `id` (existing) | same |
| level content digest | `[u8; 32]`, widened domain, epoch 2 | parity variant field | n/a | n/a |
| client→server Control envelope | tagged enum in `wire.rs`, mirroring `ClientMessage` | Control, client→server; carries admission + parity | n/a | n/a |
| server→client Control envelope | tagged enum in `wire.rs`, mirroring `ServerMessage` | Control, server→client; carries relevel + divergence | n/a | n/a |
| divergence reason | `DivergenceReason::{Closing(ClosingCause), Holding(HoldingCause)}` — 2 closing (protocol, mod id), 3 holding (mod digest, level identity, level digest) | inside the server→client envelope | n/a | n/a |
| slot state | `SlotState::{Pending, Admitted, Participating, Closed { cause }}`, no longer `Copy` | not replicated | n/a | n/a |
| retained slot declaration | the last parity triple a slot declared, held per slot | not replicated | n/a | n/a |

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
- **~~Stable-key sort vs canonical serialization~~ — settled.** Sort by `canonical_name`,
  which is total over the hashed set because descriptors lacking one are excluded from the
  digest entirely. The nested question the detail review surfaced — map-valued fields whose
  iteration order varies per *process* — is not an ordering preference but a correctness
  requirement, and is now a Task 6 rule and an invariant.
- **Reject-reason delivery is the trimmable part, and it shrank again.** It covers
  admission rejects only — every content divergence now holds the connection open, so its
  diagnostic rides an ordinary reliable message with no deferral — which leaves one list and
  one poll on the protocol/mod-id path. The rest of the spec works without it. If it fights
  renet's teardown, the fallback is a host-side log only, at the cost of a player who cannot
  tell a wrong mod from an unreachable host. Worth noting the trim got cheaper: with the mod
  digest moved to parity, the *common* mismatch among friends on slightly different builds
  no longer travels this path at all.
- **~~Whether a mod-digest demotion should suppress the relevel message~~ — settled.** Do not
  suppress. The send is already idempotent under Task 4's active/in-flight rule, and a cause
  filter would be a second policy in the send path. The diagnostic carries the cause, which is
  what stops a client reading the relevel as a fix.
- **~~Mid-load relevel~~ — settled by the shipped path.** `App::enqueue_level_request` already
  replaces a queued `Load` and applies the newer one on completion, so the hoped-for v1 rule is
  the existing behavior. Its one exception — an early return during the **boot** load — is now
  named in Task 4 and is benign, since a client in boot load has not been admitted yet.
- **Whether the client should despawn materialized remotes on a mod-digest demotion.** The
  level-change demotion clears the client's world by unloading it. A mod-digest demotion leaves
  the world loaded, so the client keeps its `NetworkId→EntityId` map and its remote pawns, which
  simply stop updating — a frozen tableau rather than a clean state. Either despawn mapped
  remotes and disarm prediction on demotion, or accept the freeze and say so. The second
  demotion trigger created this case; it did not exist when the demotion rule was written.
- **`networking.md` update at promotion.** The fingerprint-binds-the-connection sentence is
  overturned, the slot lifecycle gains a state and a transition, the handshake section
  describes one app message where there will be three, and the crate boundary gains a
  server→client control message family.
